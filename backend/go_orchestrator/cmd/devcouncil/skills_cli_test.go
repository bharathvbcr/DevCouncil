// `devcouncil skills scaffold` exit-code contracts.
//
// Ported from the retired `tests/unit/test_devmap_skill_delivery.py`, which
// drove these through the Python Typer app deleted in 3286db5. The installer's
// contracts live in rust/devmap-cli/src/skills.rs, which this command hands its
// library to; what is here is what a caller reads without parsing the payload:
// the exit code. A devmap binary is required (testsupport.Devmap).
package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func skillFiles(t *testing.T, root string) []string {
	t.Helper()
	var found []string
	err := filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if !d.IsDir() {
			rel, relErr := filepath.Rel(root, path)
			if relErr != nil {
				return relErr
			}
			found = append(found, rel)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	return found
}

// --check must distinguish "not installed" from "installed", and must not
// become an install by running.
func TestSkillsScaffoldCheckReportsMissingAndNeverWrites(t *testing.T) {
	t.Setenv("DEVMAP_BIN", testsupport.Devmap(t))
	root := t.TempDir()
	args := []string{"skills", "scaffold", "--project-root", root, "--skill", "core-engineering"}

	stdout, restore := swapStdout(t)
	code := dispatch(append(append([]string{}, args...), "--check"))
	restore()
	if code != 1 {
		t.Fatalf("--check on an empty tree exited %d: %s", code, stdout.String())
	}
	if got := skillFiles(t, root); len(got) != 0 {
		t.Fatalf("--check wrote %v", got)
	}

	stdout, restore = swapStdout(t)
	code = dispatch(args)
	restore()
	if code != 0 {
		t.Fatalf("install exited %d: %s", code, stdout.String())
	}
	// One SKILL.md per DevMap default destination, plus the receipt.
	installed := skillFiles(t, root)
	receipts, copies := 0, 0
	for _, rel := range installed {
		switch {
		case rel == ".devcouncil-skills.json":
			receipts++
		case filepath.Base(rel) == "SKILL.md" && filepath.Base(filepath.Dir(rel)) == "core-engineering":
			copies++
		default:
			t.Fatalf("install wrote an unexpected file %s", rel)
		}
	}
	if receipts != 1 || copies < 2 {
		t.Fatalf("want the receipt and a copy per host, got %v", installed)
	}
	stamps := map[string]time.Time{}
	for _, rel := range installed {
		info, err := os.Stat(filepath.Join(root, rel))
		if err != nil {
			t.Fatal(err)
		}
		stamps[rel] = info.ModTime()
	}

	stdout, restore = swapStdout(t)
	code = dispatch(append(append([]string{}, args...), "--check"))
	restore()
	if code != 0 {
		t.Fatalf("--check on an installed tree exited %d: %s", code, stdout.String())
	}
	for rel, stamp := range stamps {
		info, err := os.Stat(filepath.Join(root, rel))
		if err != nil {
			t.Fatal(err)
		}
		if !info.ModTime().Equal(stamp) {
			t.Fatalf("--check rewrote %s", rel)
		}
	}
}

// A name that does not exist is an error, even alongside names that do:
// installing the half that matched and exiting 0 reports a typo as success.
func TestSkillsScaffoldRejectsUnknownSkillWithoutClaimingUpToDate(t *testing.T) {
	for _, args := range [][]string{
		{"--skill", "not-a-skill"},
		{"--skill", "core-engineering", "--skill", "not-a-skill"},
	} {
		root := t.TempDir()
		full := append([]string{"skills", "scaffold", "--project-root", root}, args...)
		stderr, restoreErr := swapStderr(t)
		stdout, restoreOut := swapStdout(t)
		code := dispatch(full)
		restoreOut()
		restoreErr()
		if code == 0 {
			t.Fatalf("%v exited 0: %s%s", args, stdout.String(), stderr.String())
		}
		if out := stdout.String() + stderr.String(); !strings.Contains(out, "Unknown skill") {
			t.Fatalf("%v did not name the unknown skill: %q", args, out)
		}
		if got := skillFiles(t, root); len(got) != 0 {
			t.Fatalf("%v wrote %v", args, got)
		}
	}
}
