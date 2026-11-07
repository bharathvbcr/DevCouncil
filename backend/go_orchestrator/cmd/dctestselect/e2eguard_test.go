package main

import (
	"errors"
	"go/build"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"testing"
)

var testDecl = regexp.MustCompile(`(?m)^func (Test\w*)\(`)

// e2eOnlyTestFiles are the test files of dir that build with the e2e tag and
// not without it, for the platform the workflow's runner builds on. go/build
// evaluates every constraint shape (block-comment headers, `// +build`, BOM,
// filename suffixes) as the go command does. The platform is pinned: ImportDir
// reads GOOS and GOARCH from the context, so a file tagged `e2e && linux` would
// otherwise be excluded in both runs on a macOS dev box and pass the guard,
// while the ubuntu runner's `-run '^TestE2E'` skips its misnamed test.
func e2eOnlyTestFiles(dir string) ([]string, error) {
	files := func(tags ...string) (map[string]bool, error) {
		ctx := build.Default
		ctx.GOOS, ctx.GOARCH, ctx.CgoEnabled, ctx.BuildTags = "linux", "amd64", true, tags
		pkg, err := ctx.ImportDir(dir, 0)
		var none *build.NoGoError
		if err != nil && !errors.As(err, &none) {
			return nil, err
		}
		set := map[string]bool{}
		for _, f := range append(append([]string{}, pkg.TestGoFiles...), pkg.XTestGoFiles...) {
			set[f] = true
		}
		return set, nil
	}
	with, err := files("e2e")
	if err != nil {
		return nil, err
	}
	without, err := files()
	if err != nil {
		return nil, err
	}
	var only []string
	for f := range with {
		if !without[f] {
			only = append(only, f)
		}
	}
	sort.Strings(only)
	return only, nil
}

// misnamedE2ETests returns the e2e-only test files of dir and the tests in them
// whose name does not start with TestE2E: CI and ci:local run `-run '^TestE2E'`,
// so such a test is compiled out of ./... and filtered out of that run, and
// runs nowhere.
func misnamedE2ETests(dir string) (scanned, misnamed []string, err error) {
	scanned, err = e2eOnlyTestFiles(dir)
	if err != nil {
		return nil, nil, err
	}
	for _, f := range scanned {
		raw, err := os.ReadFile(filepath.Join(dir, f))
		if err != nil {
			return nil, nil, err
		}
		for _, m := range testDecl.FindAllStringSubmatch(string(raw), -1) {
			if !strings.HasPrefix(m[1], "TestE2E") {
				misnamed = append(misnamed, f+": "+m[1])
			}
		}
	}
	return scanned, misnamed, nil
}

func TestTheE2EGuardReadsTheBuildConstraintNotTheFirstLine(t *testing.T) {
	dir := t.TempDir()
	const bad = "\nimport \"testing\"\n\nfunc TestNotPrefixed(t *testing.T) {}\n"
	for name, head := range map[string]string{
		"plain.go":     "//go:build e2e\n\npackage main\n",
		"comment.go":   "// Copyright the authors.\n// Licensed under the terms.\n\n//go:build e2e\n\npackage main\n",
		"and.go":       "//go:build e2e && unix\n\npackage main\n",
		"andfirst.go":  "//go:build linux && e2e\n\npackage main\n",
		"bom.go":       "\ufeff//go:build e2e\n\npackage main\n",
		"parens.go":    "//go:build (e2e && unix) || (e2e && !windows)\n\npackage main\n",
		"untagged.go":  "package main\n",
		"negated.go":   "//go:build !e2e\n\npackage main\n",
		"either.go":    "//go:build e2e || linux\n\npackage main\n",
		"othertag.go":  "//go:build integration\n\npackage main\n",
		"afterpkg.go":  "package main\n\n// a mention of //go:build e2e\n",
		"block.go":     "/*\n * Copyright the authors.\n */\n\n//go:build e2e\n\npackage main\n",
		"legacy.go":    "// +build e2e\n\npackage main\n",
		"linuxonly.go": "//go:build e2e && linux\n\npackage main\n",
		"darwin.go":    "//go:build e2e && darwin\n\npackage main\n",
	} {
		if err := os.WriteFile(filepath.Join(dir, strings.TrimSuffix(name, ".go")+"_test.go"), []byte(head+bad), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	// A well-named test in an e2e file is fine.
	if err := os.WriteFile(filepath.Join(dir, "good_test.go"), []byte("//go:build e2e\n\npackage main\n\nfunc TestE2EFine(t *testing.T) {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	scanned, misnamed, err := misnamedE2ETests(dir)
	if err != nil {
		t.Fatal(err)
	}
	wantScanned := []string{"and_test.go", "andfirst_test.go", "block_test.go", "bom_test.go", "comment_test.go", "good_test.go", "legacy_test.go", "linuxonly_test.go", "parens_test.go", "plain_test.go"}
	if strings.Join(scanned, " ") != strings.Join(wantScanned, " ") {
		t.Errorf("scanned %v, want %v", scanned, wantScanned)
	}
	if len(misnamed) != len(wantScanned)-1 {
		t.Errorf("misnamed %v: one per scanned file except good_test.go", misnamed)
	}
}

func TestEveryE2EOnlyTestOfThisPackageIsNamedTestE2E(t *testing.T) {
	scanned, misnamed, err := misnamedE2ETests(".")
	if err != nil {
		t.Fatal(err)
	}
	// The guard must have looked at the files it exists for, by name.
	for _, want := range []string{"e2e_test.go", "e2e_attack2_test.go"} {
		found := false
		for _, s := range scanned {
			found = found || s == want
		}
		if !found {
			t.Errorf("%s was not scanned (scanned %v); the guard would pass over it", want, scanned)
		}
	}
	for _, m := range misnamed {
		t.Errorf("%s is e2e-only and would be filtered out of -run '^TestE2E'", m)
	}
}

// The e2e guard reads a file behind a licence block comment: the go command
// honours the //go:build line there, so a misnamed test in it runs nowhere.
func TestTheE2EGuardScansAFileWithABlockCommentHeader(t *testing.T) {
	dir := t.TempDir()
	body := "/*\n * Copyright the authors.\n */\n\n//go:build e2e\n\npackage main\n\nimport \"testing\"\n\nfunc TestNotPrefixed(t *testing.T) {}\n"
	if err := os.WriteFile(filepath.Join(dir, "blk_test.go"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	scanned, misnamed, err := misnamedE2ETests(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(scanned) != 1 || len(misnamed) != 1 {
		t.Fatalf("scanned=%v misnamed=%v: an e2e-only file behind a block comment passes the guard unread", scanned, misnamed)
	}
}

// The e2e guard reads a legacy `// +build e2e` line with no //go:build line.
func TestTheE2EGuardScansALegacyPlusBuildOnlyFile(t *testing.T) {
	dir := t.TempDir()
	body := "// +build e2e\n\npackage main\n\nimport \"testing\"\n\nfunc TestNotPrefixed(t *testing.T) {}\n"
	if err := os.WriteFile(filepath.Join(dir, "legacy_test.go"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	scanned, misnamed, err := misnamedE2ETests(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(scanned) != 1 || len(misnamed) != 1 {
		t.Fatalf("scanned=%v misnamed=%v: a `// +build e2e` only file passes the guard unread", scanned, misnamed)
	}
}
