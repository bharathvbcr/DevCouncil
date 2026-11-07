package main

import (
	"go/build/constraint"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

// e2eOnly is whether a file's build constraint can hold with the e2e tag and
// cannot hold without it, whatever its other tags are. The constraint is the
// //go:build line in the header: blank lines and line comments only, before the
// package clause, which is where the go command reads it.
func e2eOnly(body string) bool {
	var expr constraint.Expr
	for _, line := range strings.Split(strings.TrimPrefix(body, "\ufeff"), "\n") {
		line = strings.TrimSpace(line)
		switch {
		case line == "":
			continue
		case constraint.IsGoBuild(line):
			e, err := constraint.Parse(line)
			if err != nil {
				return false
			}
			expr = e
		case strings.HasPrefix(line, "//"):
			continue
		}
		break
	}
	if expr == nil {
		return false
	}
	var others []string
	seen := map[string]bool{}
	var walk func(constraint.Expr)
	walk = func(e constraint.Expr) {
		switch x := e.(type) {
		case *constraint.AndExpr:
			walk(x.X)
			walk(x.Y)
		case *constraint.OrExpr:
			walk(x.X)
			walk(x.Y)
		case *constraint.NotExpr:
			walk(x.X)
		case *constraint.TagExpr:
			if x.Tag != "e2e" && !seen[x.Tag] {
				seen[x.Tag] = true
				others = append(others, x.Tag)
			}
		}
	}
	walk(expr)
	holds := func(e2e bool) bool {
		for mask := 0; mask < 1<<len(others); mask++ {
			if expr.Eval(func(tag string) bool {
				if tag == "e2e" {
					return e2e
				}
				for i, o := range others {
					if o == tag {
						return mask&(1<<i) != 0
					}
				}
				return false
			}) {
				return true
			}
		}
		return false
	}
	return holds(true) && !holds(false)
}

var testDecl = regexp.MustCompile(`(?m)^func (Test\w*)\(`)

// misnamedE2ETests scans the *_test.go files of dir and returns the files that
// only build under the e2e tag and the e2e-only tests whose name does not start
// with TestE2E: CI and ci:local run `-run '^TestE2E'`, so such a test is
// compiled out of ./... and filtered out of that run, and runs nowhere.
func misnamedE2ETests(dir string) (scanned, misnamed []string, err error) {
	files, err := filepath.Glob(filepath.Join(dir, "*_test.go"))
	if err != nil {
		return nil, nil, err
	}
	for _, f := range files {
		raw, err := os.ReadFile(f)
		if err != nil {
			return nil, nil, err
		}
		if !e2eOnly(string(raw)) {
			continue
		}
		scanned = append(scanned, filepath.Base(f))
		for _, m := range testDecl.FindAllStringSubmatch(string(raw), -1) {
			if !strings.HasPrefix(m[1], "TestE2E") {
				misnamed = append(misnamed, filepath.Base(f)+": "+m[1])
			}
		}
	}
	return scanned, misnamed, nil
}

func TestTheE2EGuardReadsTheBuildConstraintNotTheFirstLine(t *testing.T) {
	dir := t.TempDir()
	const bad = "\nimport \"testing\"\n\nfunc TestNotPrefixed(t *testing.T) {}\n"
	for name, head := range map[string]string{
		"plain.go":    "//go:build e2e\n\npackage main\n",
		"comment.go":  "// Copyright the authors.\n// Licensed under the terms.\n\n//go:build e2e\n\npackage main\n",
		"and.go":      "//go:build e2e && unix\n\npackage main\n",
		"andfirst.go": "//go:build linux && e2e\n\npackage main\n",
		"bom.go":      "\ufeff//go:build e2e\n\npackage main\n",
		"parens.go":   "//go:build (e2e && unix) || (e2e && !windows)\n\npackage main\n",
		"untagged.go": "package main\n",
		"negated.go":  "//go:build !e2e\n\npackage main\n",
		"either.go":   "//go:build e2e || linux\n\npackage main\n",
		"othertag.go": "//go:build integration\n\npackage main\n",
		"afterpkg.go": "package main\n\n// a mention of //go:build e2e\n",
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
	wantScanned := []string{"and_test.go", "andfirst_test.go", "bom_test.go", "comment_test.go", "good_test.go", "parens_test.go", "plain_test.go"}
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
