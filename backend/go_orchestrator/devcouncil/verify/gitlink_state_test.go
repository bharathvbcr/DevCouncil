package verify_test

import (
	"context"
	"os"
	"reflect"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// The collection names a typechange in both directions, which the end-to-end
// tests cannot show: dcverify already refuses a diff that writes a typechange
// as two sections for one path, so that run is blocked before its reasons are
// shown.
func TestCollectGitStateReportNamesEveryGitlinkShape(t *testing.T) {
	t.Run("a file replaced by a submodule", func(t *testing.T) {
		root := gitRepo(t)
		writeFile(t, root, "ts", "a plain file\n")
		attackCommit(t, root, "add ts")
		runGitIn(t, root, "rm", "-q", "-f", "ts")
		addSubmodule(t, root, freshSource(t, "ts-src", "x\n"), "ts")
		state, err := verify.CollectGitStateReport(context.Background(), root)
		if err != nil {
			t.Fatal(err)
		}
		if want := []proc.GitlinkChange{{Path: "ts", Shape: proc.GitlinkFileToSubmodule}}; !reflect.DeepEqual(state.MovedGitlinks, want) {
			t.Fatalf("got %+v, want %+v", state.MovedGitlinks, want)
		}
	})
	t.Run("a submodule replaced by a file", func(t *testing.T) {
		root, subDir := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
		runGitIn(t, root, "rm", "-q", "--cached", "sub")
		if err := os.RemoveAll(subDir); err != nil {
			t.Fatal(err)
		}
		writeFile(t, root, "sub", "now a plain file\n")
		runGitIn(t, root, "add", "sub")
		state, err := verify.CollectGitStateReport(context.Background(), root)
		if err != nil {
			t.Fatal(err)
		}
		if want := []proc.GitlinkChange{{Path: "sub", Shape: proc.GitlinkSubmoduleToFile}}; !reflect.DeepEqual(state.MovedGitlinks, want) {
			t.Fatalf("got %+v, want %+v", state.MovedGitlinks, want)
		}
	})
	t.Run("a gitlink that did not change is not listed", func(t *testing.T) {
		root, _ := submoduleRepoWith(t, map[string]string{"s.txt": "ok\n"})
		writeFile(t, root, "README", "base\nan edit\n")
		state, err := verify.CollectGitStateReport(context.Background(), root)
		if err != nil {
			t.Fatal(err)
		}
		if len(state.MovedGitlinks) != 0 {
			t.Fatalf("an unchanged gitlink was listed: %+v", state.MovedGitlinks)
		}
	})
}

// A reason carries the path as git quotes it when it holds a control
// character, so a name cannot forge a second reason, and prints a secret-spelled
// path as the withheld marker. Every shape has its reason.
func TestGitlinkReasonsForEveryShapeAreWritten(t *testing.T) {
	root := gitRepo(t)
	planned := benignChange(t, root)
	state, err := verify.CollectGitStateReport(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	state.MovedGitlinks = []proc.GitlinkChange{
		{Path: "a", Shape: proc.GitlinkAdded},
		{Path: "d", Shape: proc.GitlinkDeleted},
		{Path: "m", Shape: proc.GitlinkMoved},
		{Path: "in", Shape: proc.GitlinkFileToSubmodule},
		{Path: "out", Shape: proc.GitlinkSubmoduleToFile},
		{Path: "line\nbreak", Shape: proc.GitlinkMoved},
		{Path: "credentials/vault", Shape: proc.GitlinkDeleted},
	}
	in := verify.Input{
		Task:     &store.Task{ID: "TASK-GL-REASONS", Difficulty: "easy", PlannedFiles: []dc.PlannedFile{planned}},
		GateMode: "enforce", Rigor: dcverify.New(testsupport.DCVerify(t), root),
	}
	in.WithGitState(state)
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-GL-REASONS", gaps, meta)
	want := []string{
		"submodule commit range not scanned: a (added)",
		"submodule commit range not scanned: d (deleted)",
		"submodule commit range not scanned: m (moved)",
		"submodule commit range not scanned: in (typechange: file to submodule)",
		"submodule commit range not scanned: out (typechange: submodule to file)",
		`submodule commit range not scanned: "line\nbreak" (moved)`,
		"submodule commit range not scanned: secret_path (deleted)",
	}
	if result.Status != "degraded" || result.Passed {
		t.Fatalf("status=%s passed=%v: %v", result.Status, result.Passed, advTypes(gaps))
	}
	if got := result.DegradedReasons; !reflect.DeepEqual(got, want) {
		t.Fatalf("degraded reasons:\n got %q\nwant %q", got, want)
	}
	for _, reason := range result.DegradedReasons {
		if strings.Contains(reason, "vault") {
			t.Fatalf("a reason prints a secret-spelled path: %q", reason)
		}
	}
}

// The reasons are written whether or not dcverify ran: with none installed the
// run is degraded for that and for each gitlink.
func TestGitlinkReasonsAreIndependentOfTheRigorPass(t *testing.T) {
	in := verify.Input{
		Task:          &store.Task{ID: "TASK-GL-NORIGOR", Difficulty: "easy"},
		GateMode:      "enforce",
		AllowNoRigor:  true,
		MovedGitlinks: []proc.GitlinkChange{{Path: "sub", Shape: proc.GitlinkMoved}},
	}
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-GL-NORIGOR", gaps, meta)
	if result.Status != "degraded" || !reflect.DeepEqual(result.DegradedReasons, []string{"submodule commit range not scanned: sub (moved)"}) {
		t.Fatalf("status=%s reasons=%q types=%v", result.Status, result.DegradedReasons, advTypes(gaps))
	}
}
