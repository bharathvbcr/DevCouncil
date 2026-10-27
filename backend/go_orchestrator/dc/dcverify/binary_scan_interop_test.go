//go:build unix

package dcverify

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// The kind of a degradation travels in the text of a stub_ast_degraded entry,
// because the public JSON contract has no second key for it. This is the real
// binary's side of that agreement: what it writes must be what SplitDegraded
// reads, so a binary the secret scan could not read is never reported as a file
// the stub checks fell back on.
func TestABinaryTheSecretScanCouldNotReadIsToldApartFromAStubFallback(t *testing.T) {
	root := t.TempDir()
	outside := filepath.Join(t.TempDir(), "target")
	if err := os.WriteFile(outside, []byte("x"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(root, "blob.bin")); err != nil {
		t.Fatal(err)
	}
	c := New(testsupport.DCVerify(t), root)
	c.Timeout = 60 * time.Second
	diff := "diff --git a/blob.bin b/blob.bin\nBinary files a/blob.bin and b/blob.bin differ\n"
	result, err := c.Check(context.Background(), Request{Diff: diff, Planned: []string{"blob.bin"}})
	if err != nil {
		t.Fatalf("check: %v", err)
	}
	stub, binaryScan := SplitDegraded(result.StubASTDegraded)
	if len(stub) != 0 {
		t.Fatalf("a skipped binary secret scan was reported as a stub fallback: %q", stub)
	}
	if len(binaryScan) != 1 || !strings.HasPrefix(binaryScan[0], "blob.bin"+BinaryScanSkippedMarker) {
		t.Fatalf("binary secret-scan degradations = %q, want one entry for blob.bin", binaryScan)
	}
}
