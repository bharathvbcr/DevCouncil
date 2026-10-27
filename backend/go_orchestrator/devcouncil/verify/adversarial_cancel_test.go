package verify

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The supplemental file read takes the caller's context and does not consult
// it. A cancelled context must not come back as a finished diff.
func TestAdvCancelledSupplementalReadReturnsTheCancel(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	root := t.TempDir()
	body := []byte("let aws_key = \"" + "AKIA" + "IOSFODNN7EXAMPLE" + "\";\n")
	if err := os.WriteFile(filepath.Join(root, "f.rs"), body, 0o644); err != nil {
		t.Fatal(err)
	}
	rootFS, err := os.OpenRoot(root)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = rootFS.Close() }()
	var b strings.Builder
	err = appendMissingPath(ctx, nil, rootFS, "f.rs", &b, map[string]struct{}{})
	if strings.Contains(b.String(), "AKIA"+"IOSFODNN7EXAMPLE") {
		t.Fatal("cancelled supplemental read returned a diff containing the credential")
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("cancelled supplemental read returned err=%v diff_len=%d", err, b.Len())
	}
	if b.Len() != 0 {
		t.Fatalf("cancelled read left a partial diff (%d bytes)", b.Len())
	}
}
