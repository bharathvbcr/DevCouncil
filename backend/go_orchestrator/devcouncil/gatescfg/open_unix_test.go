//go:build unix

package gatescfg

import (
	"net"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func within(t *testing.T, root string, want string) {
	t.Helper()
	for _, load := range []func(string) Snapshot{Load, Resolve} {
		done := make(chan Snapshot, 1)
		go func(load func(string) Snapshot) { done <- load(root) }(load)
		select {
		case snap := <-done:
			if snap.VerificationMode != ModeEnforce || snap.VerificationOrigin != OriginInvalid || !snap.ConfigPresent || !strings.Contains(snap.Error, want) {
				t.Fatalf("snap = %+v, want enforce/invalid mentioning %q", snap, want)
			}
		case <-time.After(2 * time.Second):
			t.Fatal("config open blocked")
		}
	}
}

func TestFIFOConfigDoesNotBlock(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(filepath.Join(dir, "config.yaml"), 0o644); err != nil {
		t.Fatal(err)
	}
	within(t, root, "named pipe")
}

func TestDirectoryConfigIsRefused(t *testing.T) {
	root := t.TempDir()
	if err := os.MkdirAll(filepath.Join(root, ".devcouncil", "config.yaml"), 0o755); err != nil {
		t.Fatal(err)
	}
	within(t, root, "directory")
}

func TestSocketConfigIsRefused(t *testing.T) {
	// macOS rejects a unix socket whose path is longer than sun_path.
	root, err := os.MkdirTemp("", "gp")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(root) })
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", filepath.Join(dir, "config.yaml"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	within(t, root, "socket")
}

func TestSymlinkToFIFOIsRefusedBeforeOpen(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	fifo := filepath.Join(dir, "pipe")
	if err := syscall.Mkfifo(fifo, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(fifo, filepath.Join(dir, "config.yaml")); err != nil {
		t.Fatal(err)
	}
	within(t, root, "symlink")
}

func TestSymlinkLoopConfigFailsClosed(t *testing.T) {
	root := t.TempDir()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "config.yaml")
	if err := os.Symlink(path, path); err != nil {
		t.Fatal(err)
	}
	done := make(chan Snapshot, 1)
	go func() { done <- Load(root) }()
	select {
	case snap := <-done:
		if snap.VerificationMode != ModeEnforce || snap.VerificationOrigin != OriginInvalid || !snap.ConfigPresent || snap.Error == "" {
			t.Fatalf("symlink loop = %+v, want enforce/invalid", snap)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("symlink loop blocked")
	}
}

func TestUnicodeRootStillReadsARegularFile(t *testing.T) {
	parent := t.TempDir()
	root := filepath.Join(parent, "配置")
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte("gates:\n  mode: advisory\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	snap := Load(root)
	if snap.VerificationMode != ModeAdvisory || snap.VerificationOrigin != OriginConfig || snap.Error != "" {
		t.Fatalf("unicode root = %+v, want advisory/config", snap)
	}
}
