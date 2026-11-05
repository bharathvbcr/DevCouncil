package flags

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

// Stat reports the link dangling, then the open follows a file that appeared.
// The bytes of that file must not be returned: a live symlink is refused.
func TestASymlinkThatBecomesReadableIsStillRefused(t *testing.T) {
	dir := t.TempDir()
	target := filepath.Join(dir, "real.yaml")
	if err := os.WriteFile(target, []byte("gates:\n  mode: advisory\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(dir, "config.yaml")
	if err := os.Symlink(filepath.Join(dir, "missing.yaml"), link); err != nil {
		t.Fatal(err)
	}

	origStat := statPath
	origOpen := openFollowPath
	t.Cleanup(func() {
		statPath = origStat
		openFollowPath = origOpen
	})
	statPath = func(string) (os.FileInfo, error) {
		return nil, os.ErrNotExist
	}
	openFollowPath = func(string) (*os.File, error) {
		return os.Open(target)
	}

	f, err := OpenRegular(link)
	if f != nil {
		_ = f.Close()
		t.Fatal("a symlink that resolved between stat and open was returned")
	}
	var kind *NonRegularConfigError
	if !errors.As(err, &kind) || kind.Kind != "symlink" {
		t.Fatalf("OpenRegular err = %v, want a symlink refusal", err)
	}
}
