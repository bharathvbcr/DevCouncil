package main

import (
	"strings"
	"testing"
)

// runningSection is one finished libtest section as cargo prints it: the header
// names the source relative to the package, so every crate's lib prints
// `src/lib.rs`, and the binary is named for the crate and a hash.
func runningSection(src, crate string) string {
	return "     Running unittests " + src + " (target/debug/deps/" + crate + "-4fed9988ac85c8ac)\n\nrunning 1 test\ntest t ... ok\n\ntest result: ok. 1 passed; 0 failed\n\n"
}

// presencePlan selects crates ca and cd in one cargo command, each with the
// test source cargo prints a section for.
func presencePlan(units ...Unit) Plan {
	p := selectedCa
	p.Selected = []Selected{{Command: logged(newCommand("rust", "cargo", "test", "--locked", "-p", "ca", "-p", "cd", "--no-fail-fast")), Units: units}}
	return p
}

var (
	unitCa = Unit{Kind: "rust", Pkg: "ca", Runs: []UnitRun{{"src/lib.rs", "ca"}, {"tests/it.rs", "it"}}}
	unitCd = Unit{Kind: "rust", Pkg: "cd", Runs: []UnitRun{{"src/lib.rs", "cd"}}}
)

// Two selected crates, exit 0, a log with a section for ca only: cd, which has
// a test target cargo runs, left nothing, so the fast phase was not examined.
func TestSelectedCrateWithNoSectionAtAllIsNotExamined(t *testing.T) {
	plan := presencePlan(unitCa, unitCd)
	dir := phaseDir(t, plan, plantedMiss, goPass, finished(101))
	setSelected(t, dir, plan, runningSection("src/lib.rs", "ca"), 0)
	code, stdout, r := auditIn(t, dir)
	if code != 2 || !strings.Contains(stdout, "ran nothing for cd") {
		t.Errorf("exit=%d; unit cd has no section in a log shared by ca and cd\n%s", code, stdout)
	}
	if r.SelectedPhaseFailedOnly != nil {
		t.Errorf("selected_phase_failed_only=%v for a phase that was not examined", r.SelectedPhaseFailedOnly)
	}
}

// Every crate has a section: any one of a crate's test targets is enough.
func TestEveryCrateWithASectionIsExamined(t *testing.T) {
	plan := presencePlan(unitCa, unitCd)
	dir := phaseDir(t, plan, plantedMiss, goPass, finished(101))
	setSelected(t, dir, plan, runningSection("tests/it.rs", "it")+runningSection("src/lib.rs", "cd"), 0)
	if code, stdout, _ := auditIn(t, dir); code != 0 {
		t.Errorf("exit=%d\n%s", code, stdout)
	}
}

// A crate with no test target cargo runs (every target test = false, or gated
// by a feature) has no Runs: cargo prints it no section, and that is not a
// reason to refuse the run.
func TestSelectedCrateWithNoRunnableTestTargetDoesNotCauseExitTwo(t *testing.T) {
	plan := presencePlan(unitCa, Unit{Kind: "rust", Pkg: "cd"})
	dir := phaseDir(t, plan, plantedMiss, goPass, finished(101))
	setSelected(t, dir, plan, runningSection("src/lib.rs", "ca"), 0)
	if code, stdout, _ := auditIn(t, dir); code != 0 {
		t.Errorf("exit=%d; a crate with nothing to run was held against the run\n%s", code, stdout)
	}
}

// A crate whose only failure is a doctest (no `Running` source for it) is
// present through the package its footer names.
func TestACrateNamedByAFailedSectionsFooterIsPresent(t *testing.T) {
	plan := presencePlan(unitCa, unitCd)
	dir := phaseDir(t, plan, plantedMiss, goPass, finished(101))
	doc := "   Doc-tests cd\n\nrunning 1 test\ntest src/lib.rs - f (line 1) ... FAILED\n\nfailures:\n\n---- src/lib.rs - f (line 1) stdout ----\n\nfailures:\n    src/lib.rs - f (line 1)\n\ntest result: FAILED. 0 passed; 1 failed\nerror: doctest failed, to rerun pass `-p cd --doc`\n"
	setSelected(t, dir, plan, runningSection("src/lib.rs", "ca")+doc, 101)
	if code, stdout, _ := auditIn(t, dir); code != 0 {
		t.Errorf("exit=%d\n%s", code, stdout)
	}
}

// Cargo prints `src/lib.rs` for every crate's lib, so the source alone cannot
// say whose section it is: with ca's section only, cd is still not examined.
func TestTwoCratesPrintingTheSameSourceAreToldApartByTheirBinary(t *testing.T) {
	plan := presencePlan(unitCa, unitCd)
	dir := phaseDir(t, plan, plantedMiss, goPass, finished(101))
	setSelected(t, dir, plan, runningSection("src/lib.rs", "ca"), 0)
	if code, stdout, _ := auditIn(t, dir); code != 2 || !strings.Contains(stdout, "ran nothing for cd") {
		t.Errorf("exit=%d\n%s", code, stdout)
	}
}
