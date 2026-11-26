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
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gating"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// TestAdversarialCompoundGapsDemotablePlusHardSafety tests that when a demotable
// gap (such as test_failed) is present alongside a hard-safety unproven gap
// (acceptance_criteria_unproven or llm_review_unproven), advisory and warn
// modes do NOT fail open. Status must remain "blocked", passed must be false,
// and ToMCP must reflect blocking gaps while demoting test_failed to advisory actions.
func TestAdversarialCompoundGapsDemotablePlusHardSafety(t *testing.T) {
	for _, mode := range []string{"advisory", "warn", "ADVISORY", "WARN", " advisory "} {
		for _, hardGapType := range []string{"acceptance_criteria_unproven", "llm_review_unproven"} {
			demotable := Gap{
				ID:          "GAP-TEST-FAIL",
				Severity:    "high",
				GapType:     "test_failed",
				TaskID:      "TASK-COMPOUND",
				Description: "unit tests failed on pkg/calc",
				Blocking:    true,
			}
			hardSafety := Gap{
				ID:          "GAP-HARD-" + hardGapType,
				Severity:    "high",
				GapType:     hardGapType,
				TaskID:      "TASK-COMPOUND",
				Description: "hard safety requirement was not discharged: " + hardGapType,
				Blocking:    true,
			}

			gaps := []Gap{demotable, hardSafety}

			status, passed := StatusFromGaps(gaps, mode)
			if status != "blocked" || passed {
				t.Fatalf("mode %q with %s: StatusFromGaps = %q, passed = %v; want blocked, false", mode, hardGapType, status, passed)
			}

			meta := runMeta{GateMode: mode, Sandbox: SandboxLocal}
			vStatus, vPassed, reasons := Verdict(gaps, meta)
			if vStatus != "blocked" || vPassed || reasons != nil {
				t.Fatalf("mode %q with %s: Verdict = %q, passed = %v, reasons = %v; want blocked, false, nil", mode, hardGapType, vStatus, vPassed, reasons)
			}

			payload := ToMCP("TASK-COMPOUND", gaps, meta)
			if payload.Status != "blocked" || payload.Passed {
				t.Fatalf("mode %q with %s: ToMCP Status = %q, Passed = %v; want blocked, false", mode, hardGapType, payload.Status, payload.Passed)
			}

			// DemoteForMode under advisory demotes test_failed, leaving only hardSafety as blocking
			if len(payload.BlockingGaps) != 1 || payload.BlockingGaps[0].ID != hardSafety.ID {
				t.Fatalf("mode %q with %s: BlockingGaps = %#v; want exactly [%s]", mode, hardGapType, payload.BlockingGaps, hardSafety.ID)
			}
			if len(payload.NextActions) != 1 || payload.NextActions[0].GapID != hardSafety.ID || !payload.NextActions[0].Blocking {
				t.Fatalf("mode %q with %s: NextActions = %#v; want [%s] with Blocking=true", mode, hardGapType, payload.NextActions, hardSafety.ID)
			}
			if len(payload.AdvisoryActions) != 1 || payload.AdvisoryActions[0].GapID != demotable.ID || payload.AdvisoryActions[0].Blocking {
				t.Fatalf("mode %q with %s: AdvisoryActions = %#v; want [%s] with Blocking=false", mode, hardGapType, payload.AdvisoryActions, demotable.ID)
			}
		}
	}
}

// TestAdversarialBothUnprovenKindsTogether ensures that when acceptance_criteria_unproven
// and llm_review_unproven occur simultaneously, neither is demoted under advisory/warn modes.
// Both must remain strictly blocking in StatusFromGaps, Verdict, and ToMCP.
func TestAdversarialBothUnprovenKindsTogether(t *testing.T) {
	acGap := Gap{
		ID:          "GAP-AC-1",
		Severity:    "high",
		GapType:     "acceptance_criteria_unproven",
		TaskID:      "TASK-BOTH-UNPROVEN",
		Description: "criterion AC-1 was not proven",
		Blocking:    true,
	}
	llmGap := Gap{
		ID:          "GAP-LLM-1",
		Severity:    "high",
		GapType:     "llm_review_unproven",
		TaskID:      "TASK-BOTH-UNPROVEN",
		Description: "architectural LLM review was not recorded",
		Blocking:    true,
	}

	gaps := []Gap{acGap, llmGap}

	for _, mode := range []string{"advisory", "warn", "enforce", " ADVISORY ", " Warn "} {
		status, passed := StatusFromGaps(gaps, mode)
		if status != "blocked" || passed {
			t.Fatalf("mode %q: StatusFromGaps = %q, passed = %v; want blocked, false", mode, status, passed)
		}

		meta := runMeta{GateMode: mode, Sandbox: SandboxLocal}
		vStatus, vPassed, reasons := Verdict(gaps, meta)
		if vStatus != "blocked" || vPassed || len(reasons) > 0 {
			t.Fatalf("mode %q: Verdict = %q, passed = %v, reasons = %v; want blocked, false, empty", mode, vStatus, vPassed, reasons)
		}

		payload := ToMCP("TASK-BOTH-UNPROVEN", gaps, meta)
		if payload.Status != "blocked" || payload.Passed {
			t.Fatalf("mode %q: ToMCP Status = %q, Passed = %v; want blocked, false", mode, payload.Status, payload.Passed)
		}

		if len(payload.BlockingGaps) != 2 {
			t.Fatalf("mode %q: BlockingGaps count = %d; want 2", mode, len(payload.BlockingGaps))
		}
		if len(payload.NextActions) != 2 {
			t.Fatalf("mode %q: NextActions count = %d; want 2", mode, len(payload.NextActions))
		}
		for _, action := range payload.NextActions {
			if !action.Blocking {
				t.Fatalf("mode %q: NextAction %s has Blocking = false; want true", mode, action.GapID)
			}
		}
		if len(payload.AdvisoryActions) != 0 {
			t.Fatalf("mode %q: AdvisoryActions count = %d; want 0", mode, len(payload.AdvisoryActions))
		}
	}
}

// TestAdversarialAdvisoryModeWhitespaceAndCaseVariations exercises strange casing
// and whitespace surrounding advisory and warn gate modes to ensure parsing is robust,
// never silently skips or enforces, and maintains fail-closed behavior on unproven gaps.
func TestAdversarialAdvisoryModeWhitespaceAndCaseVariations(t *testing.T) {
	variations := []string{
		" ADVISORY ",
		" Warn ",
		"advisory",
		"warn",
		"ADVISORY",
		"WARN",
		"\tadvisory\n",
		"\tWARN\r\n",
		"  warn  ",
		"  advisory  ",
	}

	for _, mode := range variations {
		if verificationSkipped(mode) {
			t.Fatalf("mode %q was reported as verificationSkipped; expected active advisory/warn", mode)
		}
		if !modeAdvisory(mode) {
			t.Fatalf("mode %q was not recognized as modeAdvisory", mode)
		}
		if modeEnforces(mode) {
			t.Fatalf("mode %q was incorrectly recognized as modeEnforces", mode)
		}

		// 1. With an unproven gap -> must block
		unproven := []Gap{{
			ID:          "GAP-UNPROVEN",
			Severity:    "high",
			GapType:     "acceptance_criteria_unproven",
			TaskID:      "TASK-VARIATION",
			Description: "criterion unproven",
			Blocking:    true,
		}}
		status, passed := StatusFromGaps(unproven, mode)
		if status != "blocked" || passed {
			t.Fatalf("mode %q with unproven: StatusFromGaps = %q, passed = %v; want blocked, false", mode, status, passed)
		}

		meta := runMeta{GateMode: mode, Sandbox: SandboxLocal}
		vStatus, vPassed, _ := Verdict(unproven, meta)
		if vStatus != "blocked" || vPassed {
			t.Fatalf("mode %q with unproven: Verdict = %q, passed = %v; want blocked, false", mode, vStatus, vPassed)
		}

		// 2. With a demotable test failure -> must demote in StatusFromGaps and degrade in Verdict
		demotable := []Gap{{
			ID:          "GAP-FAIL",
			Severity:    "high",
			GapType:     "test_failed",
			TaskID:      "TASK-VARIATION",
			Description: "test command exited 1",
			Blocking:    true,
		}}
		dStatus, dPassed := StatusFromGaps(demotable, mode)
		if dStatus != "verified" || !dPassed {
			t.Fatalf("mode %q with demotable: StatusFromGaps = %q, passed = %v; want verified, true", mode, dStatus, dPassed)
		}

		dvStatus, dvPassed, dvReasons := Verdict(demotable, meta)
		if dvStatus != "degraded" || dvPassed {
			t.Fatalf("mode %q with demotable: Verdict = %q, passed = %v; want degraded, false", mode, dvStatus, dvPassed)
		}
		if len(dvReasons) != 1 || !strings.Contains(dvReasons[0], "demoted by advisory mode") {
			t.Fatalf("mode %q with demotable: reasons = %v; want 1 reason mentioning demoted by advisory mode", mode, dvReasons)
		}

		// 3. With clean gaps -> must verify
		cStatus, cPassed := StatusFromGaps(nil, mode)
		if cStatus != "verified" || !cPassed {
			t.Fatalf("mode %q clean: StatusFromGaps = %q, passed = %v; want verified, true", mode, cStatus, cPassed)
		}
		cvStatus, cvPassed, cvReasons := Verdict(nil, meta)
		if cvStatus != "verified" || !cvPassed || len(cvReasons) > 0 {
			t.Fatalf("mode %q clean: Verdict = %q, passed = %v, reasons = %v; want verified, true, empty", mode, cvStatus, cvPassed, cvReasons)
		}
	}
}

// TestAdversarialSoftGapDemotionVsHardSafety tests that soft gaps alone demote cleanly,
// but introducing a hard safety unproven gap immediately shifts the verdict from verified
// to blocked, ensuring no accidental pass.
func TestAdversarialSoftGapDemotionVsHardSafety(t *testing.T) {
	for _, mode := range []string{"advisory", "warn"} {
		softGap := Gap{
			ID:          "GAP-COARSE",
			Severity:    "low",
			GapType:     "coarse_acceptance_proof",
			TaskID:      "TASK-SOFT-VS-HARD",
			Description: "task-level coarse acceptance proof",
			Blocking:    true,
		}

		// Alone: soft gap is demoted to non-blocking
		status, passed := StatusFromGaps([]Gap{softGap}, mode)
		if status != "verified" || !passed {
			t.Fatalf("mode %q soft gap alone: StatusFromGaps = %q, passed = %v; want verified, true", mode, status, passed)
		}

		meta := runMeta{GateMode: mode, Sandbox: SandboxLocal}
		payload := ToMCP("TASK-SOFT-VS-HARD", []Gap{softGap}, meta)
		if payload.Status != "verified" || !payload.Passed {
			t.Fatalf("mode %q soft gap alone: ToMCP = %q, passed = %v; want verified, true", mode, payload.Status, payload.Passed)
		}
		if len(payload.NextActions) != 0 {
			t.Fatalf("mode %q soft gap alone: NextActions = %#v; want empty", mode, payload.NextActions)
		}
		if len(payload.AdvisoryActions) != 1 || payload.AdvisoryActions[0].GapID != softGap.ID {
			t.Fatalf("mode %q soft gap alone: AdvisoryActions = %#v; want [%s]", mode, payload.AdvisoryActions, softGap.ID)
		}

		// Now add hard safety gap: llm_review_unproven
		hardGap := Gap{
			ID:          "GAP-LLM-UNPROVEN",
			Severity:    "high",
			GapType:     "llm_review_unproven",
			TaskID:      "TASK-SOFT-VS-HARD",
			Description: "required LLM safety review was not conducted",
			Blocking:    true,
		}

		combinedGaps := []Gap{softGap, hardGap}
		combStatus, combPassed := StatusFromGaps(combinedGaps, mode)
		if combStatus != "blocked" || combPassed {
			t.Fatalf("mode %q combined: StatusFromGaps = %q, passed = %v; want blocked, false", mode, combStatus, combPassed)
		}

		combPayload := ToMCP("TASK-SOFT-VS-HARD", combinedGaps, meta)
		if combPayload.Status != "blocked" || combPayload.Passed {
			t.Fatalf("mode %q combined: ToMCP = %q, passed = %v; want blocked, false", mode, combPayload.Status, combPayload.Passed)
		}
		if len(combPayload.BlockingGaps) != 1 || combPayload.BlockingGaps[0].ID != hardGap.ID {
			t.Fatalf("mode %q combined: BlockingGaps = %#v; want exactly [%s]", mode, combPayload.BlockingGaps, hardGap.ID)
		}
		if len(combPayload.NextActions) != 1 || combPayload.NextActions[0].GapID != hardGap.ID {
			t.Fatalf("mode %q combined: NextActions = %#v; want exactly [%s]", mode, combPayload.NextActions, hardGap.ID)
		}
		if len(combPayload.AdvisoryActions) != 1 || combPayload.AdvisoryActions[0].GapID != softGap.ID {
			t.Fatalf("mode %q combined: AdvisoryActions = %#v; want exactly [%s]", mode, combPayload.AdvisoryActions, softGap.ID)
		}
	}
}

// TestAdversarialNonBlockingUnprovenGaps verifies that unproven gaps marked
// Blocking: false (e.g. optional criteria or non-blocking notifications) do
// not block the run in StatusFromGaps or ToMCP.
func TestAdversarialNonBlockingUnprovenGaps(t *testing.T) {
	for _, mode := range []string{"enforce", "advisory", "warn"} {
		gaps := []Gap{
			{
				ID:          "GAP-OPTIONAL-AC",
				Severity:    "low",
				GapType:     "acceptance_criteria_unproven",
				TaskID:      "TASK-NONBLOCK",
				Description: "optional benchmark criteria unproven",
				Blocking:    false,
			},
			{
				ID:          "GAP-OPTIONAL-LLM",
				Severity:    "low",
				GapType:     "llm_review_unproven",
				TaskID:      "TASK-NONBLOCK",
				Description: "optional documentation review unproven",
				Blocking:    false,
			},
		}

		status, passed := StatusFromGaps(gaps, mode)
		if status != "verified" || !passed {
			t.Fatalf("mode %q non-blocking unproven: StatusFromGaps = %q, passed = %v; want verified, true", mode, status, passed)
		}

		meta := runMeta{GateMode: mode, Sandbox: SandboxLocal}
		vStatus, vPassed, reasons := Verdict(gaps, meta)
		if vStatus != "verified" || !vPassed || len(reasons) > 0 {
			t.Fatalf("mode %q non-blocking unproven: Verdict = %q, passed = %v, reasons = %v; want verified, true, empty", mode, vStatus, vPassed, reasons)
		}

		payload := ToMCP("TASK-NONBLOCK", gaps, meta)
		if payload.Status != "verified" || !payload.Passed {
			t.Fatalf("mode %q non-blocking unproven: ToMCP = %q, passed = %v; want verified, true", mode, payload.Status, payload.Passed)
		}
		if len(payload.BlockingGaps) != 0 {
			t.Fatalf("mode %q: BlockingGaps = %#v; want empty", mode, payload.BlockingGaps)
		}
		if len(payload.NextActions) != 0 {
			t.Fatalf("mode %q: NextActions = %#v; want empty", mode, payload.NextActions)
		}
		if len(payload.AdvisoryActions) != 2 {
			t.Fatalf("mode %q: AdvisoryActions count = %d; want 2", mode, len(payload.AdvisoryActions))
		}
	}
}

// TestAdversarialDegradedReasonsSynthesisAndPersistence verifies that when pre-existing
// degraded reasons exist (including special characters, spaces, and paths) and a demotable
// command failure occurs under advisory mode:
// 1. Verdict synthesizes the advisory demotion reason and appends it after pre-existing reasons.
// 2. Persist stores the exact combined degraded_reasons in SQLite environment_json.
// 3. Stored JSON retains exact ordering and content matching ToMCP DegradedReasons.
// 4. Under enforce mode, no synthetic advisory reason is appended.
func TestAdversarialDegradedReasonsSynthesisAndPersistence(t *testing.T) {
	db := filepath.Join(t.TempDir(), "adv_state.sqlite")
	client := store.New(testsupport.DCStore(t), db)
	t.Cleanup(client.Close)

	preExistingReasons := []string{
		"scanner did not run: path with spaces/unicode caf\u00e9",
		"tool state unscanned: diff > 10MB",
		"symlinked directory d/secret was refused",
	}

	failingGap := Gap{
		ID:          "GAP-FAIL-1",
		Severity:    "high",
		GapType:     "test_failed",
		TaskID:      "TASK-ADV-PERSIST",
		Description: "go test -v ./pkg/... returned exit code 1",
		Blocking:    true,
	}
	gaps := []Gap{failingGap}

	// 1. Advisory run: pre-existing reasons + demoted test failure
	metaAdvisory := runMeta{
		GateMode:        " Warn ",
		Sandbox:         SandboxLocal,
		DegradedReasons: preExistingReasons,
	}

	payloadAdvisory := ToMCP("TASK-ADV-PERSIST", gaps, metaAdvisory)
	if payloadAdvisory.Status != "degraded" || payloadAdvisory.Passed {
		t.Fatalf("Advisory ToMCP Status = %q, Passed = %v; want degraded, false", payloadAdvisory.Status, payloadAdvisory.Passed)
	}

	expectedAdvisoryReasons := append([]string{}, preExistingReasons...)
	expectedAdvisoryReasons = append(expectedAdvisoryReasons, "expected verification command failed (demoted by advisory mode): "+failingGap.Description)

	if !reflect.DeepEqual(payloadAdvisory.DegradedReasons, expectedAdvisoryReasons) {
		t.Fatalf("Advisory payload DegradedReasons =\n%#v\nwant =\n%#v", payloadAdvisory.DegradedReasons, expectedAdvisoryReasons)
	}

	if err := Persist(context.Background(), client, "TASK-ADV-PERSIST", gaps, metaAdvisory, payloadAdvisory.Status); err != nil {
		t.Fatalf("Persist advisory: %v", err)
	}

	bin, err := exec.LookPath("sqlite3")
	if err != nil {
		t.Fatal("sqlite3 CLI is required to verify stored degraded_reasons")
	}

	out, err := exec.Command(bin, db,
		"SELECT environment_json FROM verification_runs WHERE task_id='TASK-ADV-PERSIST'").Output()
	if err != nil {
		t.Fatalf("sqlite3 read advisory: %v", err)
	}

	var envAdvisory struct {
		DegradedReasons []string `json:"degraded_reasons"`
	}
	if err := json.Unmarshal(out, &envAdvisory); err != nil {
		t.Fatalf("unmarshal environment_json: %v (raw: %s)", err, out)
	}

	if !reflect.DeepEqual(envAdvisory.DegradedReasons, expectedAdvisoryReasons) {
		t.Fatalf("Stored advisory degraded_reasons =\n%#v\nwant =\n%#v", envAdvisory.DegradedReasons, expectedAdvisoryReasons)
	}

	// 2. Enforce run: failingGap is blocking, status is blocked, so Verdict does NOT synthesize advisory reasons
	failingGapEnforce := Gap{
		ID:          "GAP-FAIL-ENFORCE",
		Severity:    "high",
		GapType:     "test_failed",
		TaskID:      "TASK-ENFORCE-PERSIST",
		Description: "go test -v ./pkg/... returned exit code 1",
		Blocking:    true,
	}
	gapsEnforce := []Gap{failingGapEnforce}

	metaEnforce := runMeta{
		GateMode:        "enforce",
		Sandbox:         SandboxLocal,
		DegradedReasons: preExistingReasons,
	}

	payloadEnforce := ToMCP("TASK-ENFORCE-PERSIST", gapsEnforce, metaEnforce)
	if payloadEnforce.Status != "blocked" || payloadEnforce.Passed {
		t.Fatalf("Enforce ToMCP Status = %q, Passed = %v; want blocked, false", payloadEnforce.Status, payloadEnforce.Passed)
	}
	// Under blocked verdict, Verdict returns reasons: nil
	if len(payloadEnforce.DegradedReasons) != 0 {
		t.Fatalf("Enforce payload DegradedReasons = %#v; want empty", payloadEnforce.DegradedReasons)
	}

	if err := Persist(context.Background(), client, "TASK-ENFORCE-PERSIST", gapsEnforce, metaEnforce, payloadEnforce.Status); err != nil {
		t.Fatalf("Persist enforce: %v", err)
	}

	outEnforce, err := exec.Command(bin, db,
		"SELECT environment_json FROM verification_runs WHERE task_id='TASK-ENFORCE-PERSIST'").Output()
	if err != nil {
		t.Fatalf("sqlite3 read enforce: %v", err)
	}

	var envEnforce struct {
		DegradedReasons []string `json:"degraded_reasons"`
	}
	if err := json.Unmarshal(outEnforce, &envEnforce); err != nil {
		t.Fatalf("unmarshal enforce environment_json: %v (raw: %s)", err, outEnforce)
	}

	if len(envEnforce.DegradedReasons) != 0 {
		t.Fatalf("Stored enforce degraded_reasons = %#v; want empty array", envEnforce.DegradedReasons)
	}
}

// Verify that HardSafetyGapTypes invariants in gating package are fully enforced
func TestAdversarialHardSafetySetMembership(t *testing.T) {
	requiredHardGaps := []string{
		"acceptance_criteria_unproven",
		"llm_review_unproven",
		"security_risk",
		"dependency_risk",
		"orphan_diff",
		"task_not_implemented",
		"stub_detected",
		"invalid_verification_command",
		"skipped_verification_command",
		"rigor_check_unavailable",
		"unsupported_verification_method",
		"runner_hijack",
		"lean_tcb",
		"unattested_test_run",
		"oracle_tampering",
	}

	for _, gapType := range requiredHardGaps {
		if !gating.IsHardSafetyGap(gapType) {
			t.Errorf("gap type %q is NOT in gating.HardSafetyGapTypes", gapType)
		}
		if gating.MayDemote(gapType, true) {
			t.Errorf("MayDemote(%q, true) = true; want false", gapType)
		}
	}
}
