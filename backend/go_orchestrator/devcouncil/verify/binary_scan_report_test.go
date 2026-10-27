//go:build unix

package verify_test

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// A binary file the secret scan could not read is a degraded run, and the
// reason has to say the credential scan skipped it. It used to be reported as
// "stub detection read N file(s) with the weaker line checks", which an
// operator cannot read as an unscanned binary.
func TestASkippedBinarySecretScanIsReportedAsThatScan(t *testing.T) {
	root := gitRepo(t)
	outside := filepath.Join(t.TempDir(), "target")
	if err := os.WriteFile(outside, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, "blob.bin")); err != nil {
		t.Fatal(err)
	}
	in := verify.Input{
		Task: &store.Task{
			ID: "TASK-BINSCAN", Difficulty: "easy",
			PlannedFiles: []dc.PlannedFile{{Path: "blob.bin", AllowedChange: dc.ChangeModify}},
		},
		GateMode:     "enforce",
		ChangedFiles: []string{"blob.bin"},
		DiffContent:  "diff --git a/blob.bin b/blob.bin\nBinary files a/blob.bin and b/blob.bin differ\n",
		WorkPresent:  true,
		Rigor:        dcverify.New(testsupport.DCVerify(t), root),
	}
	gaps, meta := verify.Run(context.Background(), in)
	result := verify.ToMCP("TASK-BINSCAN", gaps, meta)
	if result.Passed || result.Status == "verified" {
		t.Fatalf("status=%s passed=%v", result.Status, result.Passed)
	}
	joined := strings.Join(result.DegradedReasons, " | ")
	if !strings.Contains(joined, "secret scan did not read 1 binary file(s)") || !strings.Contains(joined, "blob.bin") {
		t.Fatalf("degraded reasons do not say the secret scan skipped the binary file: %q", joined)
	}
	if strings.Contains(joined, "stub detection read") {
		t.Fatalf("a skipped secret scan was reported as a stub-detection fallback: %q", joined)
	}
}
