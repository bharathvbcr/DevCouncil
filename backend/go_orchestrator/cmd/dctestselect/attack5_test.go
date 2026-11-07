package main

import (
	"path/filepath"
	"testing"
)

const okSection = "     Running unittests src/lib.rs (t/l)\n\nrunning 1 test\ntest ok ... ok\n\ntest result: ok. 1 passed; 0 failed\n"

func setSelected(t *testing.T, dir string, plan Plan, rustLog string, code int) {
	t.Helper()
	sha := planSHA(t, filepath.Join(dir, "plan.json"))
	writeFile(t, dir, "selected-rust.log", rustLog)
	st := phaseStatus{Schema: statusSchema, PlanSHA256: sha, Commands: []ExitInfo{{Cmd: plan.Selected[0].Display, Log: "selected-rust.log", Code: code, Completed: true, Detail: "exit status"}}}
	if err := writeAtomic(filepath.Join(dir, "selected-status.json"), st); err != nil {
		t.Fatal(err)
	}
}

// A5-2: exit 101, ca FAILED, cd has a Running header then a test aborts.
func TestAttack5AbortedSecondCrateIsNotExamined(t *testing.T) {
	log := "     Running unittests src/lib.rs (t/a)\n\nrunning 1 test\ntest a::bad ... FAILED\n\nfailures:\n\n---- a::bad stdout ----\n\nfailures:\n    a::bad\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p ca --lib`\n" +
		"     Running unittests src/lib.rs (t/d)\n\nrunning 1 test\nfatal runtime error: stack overflow\n"
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	setSelected(t, dir, selectedCa, log, 101)
	if code, out, _ := auditIn(t, dir); code != 2 {
		t.Errorf("exit=%d\n%s", code, out)
	}
}

// A5-3: exit 101, ca FAILED, a later compile error line.
func TestAttack5CompileErrorAfterFailedSectionIsNotExamined(t *testing.T) {
	log := "     Running unittests src/lib.rs (t/a)\n\nrunning 1 test\ntest a::bad ... FAILED\n\nfailures:\n\n---- a::bad stdout ----\n\nfailures:\n    a::bad\n\ntest result: FAILED. 0 passed; 1 failed\nerror: test failed, to rerun pass `-p ca --lib`\nerror: could not compile `cd` (lib test) due to 1 previous error\n"
	dir := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	setSelected(t, dir, selectedCa, log, 101)
	if code, out, _ := auditIn(t, dir); code != 2 {
		t.Errorf("exit=%d\n%s", code, out)
	}
}

var goSel = Plan{Schema: planSchema, Base: baseSHA,
	Full:     []Command{newCommand("rust", "cargo", "test", "--locked", "--workspace", "--no-fail-fast")},
	Selected: []Selected{{Command: logged(newCommand("go", "go", "test", "-count=1", "-json", "toy/a", "toy/b")), Units: []Unit{{Kind: "go", Pkg: "toy/a"}, {Kind: "go", Pkg: "toy/b"}}}}}

func setGo(t *testing.T, dir, log string, code int) {
	t.Helper()
	sha := planSHA(t, filepath.Join(dir, "plan.json"))
	writeFile(t, dir, "selected-go.jsonl", log)
	st := phaseStatus{Schema: statusSchema, PlanSHA256: sha, Commands: []ExitInfo{{Cmd: goSel.Selected[0].Display, Log: "selected-go.jsonl", Code: code, Completed: true, Detail: "exit status"}}}
	if err := writeAtomic(filepath.Join(dir, "selected-status.json"), st); err != nil {
		t.Fatal(err)
	}
}

// A5-4: go exit 1 explained by toy/a's failing test; toy/b printed only output, no final action.
func TestAttack5GoCommandWhoseExitIsExplainedByAnotherPackageIsNotExaminedForTheSilentOne(t *testing.T) {
	dir := phaseDir(t, goSel, plantedMiss, goPass, finished(101))
	log := `{"Action":"fail","Package":"toy/a","Test":"TestX"}` + "\n" + `{"Action":"fail","Package":"toy/a"}` + "\n" + `{"Action":"output","Package":"toy/b","Output":"setup failed\n"}` + "\n"
	setGo(t, dir, log, 1)
	if code, out, _ := auditIn(t, dir); code != 2 {
		t.Errorf("exit=%d\n%s", code, out)
	}
}

// A5-5: go exit 1; toy/a passes, toy/b only a package-level "skip" (no test files / build tag) while the exit comes from a package outside the units.
func TestAttack5GoExitFromAPackageOutsideTheUnits(t *testing.T) {
	dir := phaseDir(t, goSel, plantedMiss, goPass, finished(101))
	log := `{"Action":"pass","Package":"toy/a"}` + "\n" + `{"Action":"skip","Package":"toy/b"}` + "\n" + `{"Action":"fail","Package":"toy/zzz","Test":"TestZ"}` + "\n" + `{"Action":"fail","Package":"toy/zzz"}` + "\n"
	setGo(t, dir, log, 1)
	code, out, r := auditIn(t, dir)
	t.Logf("exit=%d selected_only=%v\n%s", code, r.SelectedPhaseFailedOnly, out)
}

// A5-6: every unit "skip" with exit 0: nothing ran for a and b.
func TestAttack5GoAllSkipExitZero(t *testing.T) {
	dir := phaseDir(t, goSel, plantedMiss, goPass, finished(101))
	log := `{"Action":"skip","Package":"toy/a"}` + "\n" + `{"Action":"skip","Package":"toy/b"}` + "\n"
	setGo(t, dir, log, 0)
	code, out, _ := auditIn(t, dir)
	t.Logf("exit=%d\n%s", code, out)
}
