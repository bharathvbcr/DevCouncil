package verify

import (
	"context"
	"encoding/json"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func unprovenGap(kind string) Gap {
	return Gap{
		ID:          "GAP-" + kind,
		Severity:    "high",
		GapType:     kind,
		TaskID:      "TASK-UNPROVEN",
		Description: "the criterion was not discharged",
		Blocking:    true,
	}
}

func TestAdvisoryDoesNotVerifyAnUnprovenCriterion(t *testing.T) {
	for _, mode := range []string{"advisory", "warn"} {
		for _, kind := range []string{"acceptance_criteria_unproven", "llm_review_unproven"} {
			gaps := []Gap{unprovenGap(kind)}
			status, passed := StatusFromGaps(gaps, mode)
			if status == "verified" || passed {
				t.Errorf("%s/%s StatusFromGaps = %s passed=%v", mode, kind, status, passed)
			}
			meta := runMeta{GateMode: mode, Sandbox: SandboxLocal}
			got, passed, reasons := Verdict(gaps, meta)
			if got == "verified" || passed || reasons != nil {
				t.Errorf("%s/%s Verdict = %s passed=%v reasons=%v", mode, kind, got, passed, reasons)
			}
			payload := ToMCP("TASK-UNPROVEN", gaps, meta)
			if payload.Status == "verified" || payload.Passed {
				t.Errorf("%s/%s ToMCP = %s passed=%v", mode, kind, payload.Status, payload.Passed)
			}
		}
	}
	soft := []Gap{{
		ID: "GAP-COARSE", Severity: "low", GapType: "coarse_acceptance_proof",
		TaskID: "TASK-UNPROVEN", Description: "task-level proof", Blocking: true,
	}}
	if status, passed := StatusFromGaps(soft, "advisory"); status != "verified" || !passed {
		t.Fatalf("coarse_acceptance_proof under advisory = %s passed=%v, want verified", status, passed)
	}
}

func TestPersistStoresTheAdvisoryDegradedReason(t *testing.T) {
	db := filepath.Join(t.TempDir(), "state.sqlite")
	client := store.New(testsupport.DCStore(t), db)
	gaps := []Gap{{
		ID: "GAP-TEST", Severity: "high", GapType: "test_failed",
		TaskID: "TASK-DEGRADE", Description: "boom", Blocking: true,
	}}
	meta := runMeta{
		GateMode:        "advisory",
		Sandbox:         SandboxLocal,
		DegradedReasons: []string{"scanner did not run"},
	}
	payload := ToMCP(gaps[0].TaskID, gaps, meta)
	if payload.Status != "degraded" || payload.Passed {
		t.Fatalf("payload = %s passed=%v, want degraded", payload.Status, payload.Passed)
	}
	joined := strings.Join(payload.DegradedReasons, "\n")
	if !strings.Contains(joined, "scanner did not run") || !strings.Contains(joined, "demoted by advisory mode") {
		t.Fatalf("payload reasons = %#v", payload.DegradedReasons)
	}
	if err := Persist(context.Background(), client, gaps[0].TaskID, gaps, meta, payload.Status); err != nil {
		t.Fatal(err)
	}
	bin, err := exec.LookPath("sqlite3")
	if err != nil {
		t.Fatal("sqlite3 CLI is required to read the stored degraded_reasons")
	}
	out, err := exec.Command(bin, db,
		"SELECT environment_json FROM verification_runs WHERE task_id='TASK-DEGRADE'").Output()
	if err != nil {
		t.Fatalf("sqlite3: %v", err)
	}
	var env struct {
		DegradedReasons []string `json:"degraded_reasons"`
	}
	if err := json.Unmarshal(out, &env); err != nil {
		t.Fatalf("environment_json %s: %v", out, err)
	}
	if !reflect.DeepEqual(env.DegradedReasons, payload.DegradedReasons) {
		t.Fatalf("stored %#v, payload %#v", env.DegradedReasons, payload.DegradedReasons)
	}
}
