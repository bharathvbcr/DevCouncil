//go:build unix

package flags

import (
	"errors"
	"net"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

func TestOpenRegularRefusesAFIFOWithoutBlocking(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.yaml")
	if err := syscall.Mkfifo(path, 0o644); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() {
		f, err := OpenRegular(path)
		if f != nil {
			_ = f.Close()
		}
		done <- err
	}()
	select {
	case err := <-done:
		var kind *NonRegularConfigError
		if !errors.As(err, &kind) || kind.Kind != "named pipe" {
			t.Fatalf("FIFO error = %v, want kind named pipe", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("OpenRegular blocked on a FIFO")
	}
}

func TestOpenRegularRefusesASocket(t *testing.T) {
	// macOS rejects a unix socket whose path is longer than sun_path.
	dir, err := os.MkdirTemp("", "gp")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	path := filepath.Join(dir, "c")
	ln, err := net.Listen("unix", path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = ln.Close() })
	_, err = OpenRegular(path)
	var kind *NonRegularConfigError
	if !errors.As(err, &kind) || kind.Kind != "socket" {
		t.Fatalf("socket error = %v, want kind socket", err)
	}
}

func TestOpenRegularRefusesASymlinkToAFIFO(t *testing.T) {
	dir := t.TempDir()
	fifo := filepath.Join(dir, "pipe")
	if err := syscall.Mkfifo(fifo, 0o644); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "config.yaml")
	if err := os.Symlink(fifo, path); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() {
		f, err := OpenRegular(path)
		if f != nil {
			_ = f.Close()
		}
		done <- err
	}()
	select {
	case err := <-done:
		var kind *NonRegularConfigError
		if !errors.As(err, &kind) || kind.Kind != "symlink" {
			t.Fatalf("symlink-to-FIFO error = %v, want kind symlink", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("OpenRegular followed a symlink onto a FIFO")
	}
}
