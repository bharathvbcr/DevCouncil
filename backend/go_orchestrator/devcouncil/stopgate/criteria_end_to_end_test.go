package stopgate_test

// The shipped path for acceptance-criterion dispatch: a real store holding a
// task that claims a criterion, the requirement row that defines it, and a
// real git repository. verify's own tests hand Run the requirements directly;
// this proves VerifyTask reads them from the store at all.

import (
	"context"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/hex"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// installSigner writes the trusted-signers file naming keyID under root, as an
// operator installs it. It lives in `.devcouncil/`, which the subject excludes.
func installSigner(t *testing.T, root, keyID string, priv ed25519.PrivateKey) {
	t.Helper()
	line, err := dc.PublicSignerLine(keyID, priv)
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "review_signers.json"), []byte(`{"signers":[`+line+`]}`), 0o600); err != nil {
		t.Fatal(err)
	}
}

func linkCriterion(t *testing.T, db, taskID, method string) {
	t.Helper()
	testsupport.MergeTaskScope(t, db, taskID, map[string]any{
		"requirement_ids":          []string{"REQ-1"},
		"acceptance_criterion_ids": []string{"AC-1"},
		"expected_tests":           []string{"true"},
	})
	sql := "INSERT INTO requirements (id,title,description,priority,source,acceptance_criteria_json) " +
		"VALUES ('REQ-1','r','','high','user','[{\"id\":\"AC-1\",\"description\":\"d\"," +
		"\"verification_method\":\"" + method + "\"}]');"
	if out, err := exec.Command(testsupport.Tool(t, "sqlite3"), db, sql).CombinedOutput(); err != nil {
		t.Fatalf("criterion fixture: %v %s", err, out)
	}
}

func TestTheShippedHostDispatchesOnVerificationMethod(t *testing.T) {
	cases := []struct {
		method      string
		wantStatus  string
		wantGapType string
	}{
		{"manual", "blocked", "unsupported_verification_method"},
		{"unit_test", "verified", "coarse_acceptance_proof"},
		// A passing expected test is not a review: with none recorded, an
		// llm_review criterion blocks.
		{"llm_review", "blocked", "llm_review_unproven"},
		// Not a method: the row cannot be read, which blocks rather than
		// verifying a criterion nobody checked.
		{"vibe_check", "blocked", "acceptance_criteria_unproven"},
	}
	for _, tc := range cases {
		t.Run(tc.method, func(t *testing.T) {
			root := repoWithWorkingTreeChange(t, "src/app.go", "package app\n")
			client := newStore(t)
			plantTask(t, client, "TASK-AC", "src/app.go")
			linkCriterion(t, client.DB, "TASK-AC", tc.method)

			out := stopgate.Run(context.Background(), stopgate.Input{
				Root: root, Store: client, TaskID: "TASK-AC", GateMode: "enforce",
			})
			if out.Decision.Skipped {
				t.Fatalf("verify did not run: %+v", out.Decision)
			}
			if out.MCP.Status != tc.wantStatus {
				t.Errorf("status = %s, want %s; gaps=%+v", out.MCP.Status, tc.wantStatus, out.Gaps)
			}
			found := false
			for _, g := range out.Gaps {
				if g.GapType == tc.wantGapType {
					found = true
				}
			}
			if !found {
				t.Errorf("no %s gap: %+v", tc.wantGapType, out.Gaps)
			}
		})
	}
}

// The whole llm_review path through the shipped host: the harness names the
// working tree, records a review of it in the real store, and verify — reading
// the criterion, the review, the rubric and the tree itself — discharges the
// criterion. Then the author changes the code, and the same review no longer
// vouches for it.
func TestARecordedReviewDischargesLLMReviewUntilTheContentMoves(t *testing.T) {
	ctx := context.Background()
	root := repoWithWorkingTreeChange(t, "src/app.go", "package app\n")
	rubric := "# Feature spec standard\nEvery failure mode is named.\n"
	commitFile(t, root, "docs/rubric.md", rubric)
	client := newStore(t)
	plantTask(t, client, "TASK-AC", "src/app.go")
	linkCriterion(t, client.DB, "TASK-AC", "llm_review")

	subject, err := verify.WorkingTreeSubject(ctx, root)
	if err != nil {
		t.Fatal(err)
	}
	// The harness holds a signing key outside the repository and installs its
	// public half in the trusted-signers file; verify discharges only a
	// review signed by it.
	_, priv, _ := ed25519.GenerateKey(nil)
	installSigner(t, root, "harness", priv)
	sum := sha256.Sum256([]byte(rubric))
	record := dc.ReviewRecord{
		Schema: dc.ReviewRecordSchema, TaskID: "TASK-AC", AcceptanceCriterionID: "AC-1",
		SubjectHash: subject, Harness: "end-to-end",
		CriterionSHA256: dc.CriterionDigest(dc.AcceptanceCriterion{ID: "AC-1", Description: "d", Method: dc.VerifyLLMReview}),
		Rubric:          dc.ReviewRubric{Path: "docs/rubric.md", SHA256: hex.EncodeToString(sum[:])},
		Isolation:       dc.ReviewIsolation{Probe: dc.IsolationPassed},
		Reviewers: []dc.ReviewerRun{
			{ID: "a", Persona: "design_critic", Status: dc.ReviewerCompleted},
			{ID: "b", Persona: "design_critic", Status: dc.ReviewerCompleted},
		},
	}
	if err := record.Sign("harness", priv); err != nil {
		t.Fatal(err)
	}
	if _, err := client.ReviewAppend(ctx, record); err != nil {
		t.Fatal(err)
	}

	run := func() stopgate.Outcome {
		out := stopgate.Run(ctx, stopgate.Input{Root: root, Store: client, TaskID: "TASK-AC", GateMode: "enforce"})
		if out.Decision.Skipped {
			t.Fatalf("verify did not run: %+v", out.Decision)
		}
		return out
	}
	if out := run(); out.MCP.Status != "verified" {
		t.Fatalf("a recorded review of this content did not discharge: %s %+v", out.MCP.Status, out.Gaps)
	}

	if err := os.WriteFile(filepath.Join(root, "src/app.go"), []byte("package app\n\nfunc Changed() {}\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	out := run()
	if out.MCP.Status != "blocked" {
		t.Fatalf("the review still vouched after the code changed: %s %+v", out.MCP.Status, out.Gaps)
	}
	for _, g := range out.Gaps {
		if g.GapType == "llm_review_unproven" {
			if !strings.Contains(strings.Join(g.Evidence, " "), "reviewed "+subject) {
				t.Errorf("the gap does not name the stale review: %v", g.Evidence)
			}
			return
		}
	}
	t.Fatalf("no llm_review_unproven gap: %+v", out.Gaps)
}

func commitFile(t *testing.T, root, rel, body string) {
	t.Helper()
	full := filepath.Join(root, rel)
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
	git := testsupport.Tool(t, "git")
	for _, args := range [][]string{{"add", rel}, {"commit", "-qm", "add " + rel, "--", rel}} {
		cmd := exec.Command(git, args...)
		cmd.Dir = root
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@example.invalid",
			"GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@example.invalid")
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
}
