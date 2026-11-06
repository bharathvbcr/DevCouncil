//go:build windows

package flags

import (
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"syscall"
	"testing"
	"time"
	"unsafe"
)

// Win32 values from the Windows SDK. syscall does not export CreateNamedPipe.
const (
	pipeAccessDuplex = 0x00000003
	pipeTypeByte     = 0x00000000
)

var procCreateNamedPipeW = syscall.NewLazyDLL("kernel32.dll").NewProc("CreateNamedPipeW")

func TestOpenRegularRefusesALiveNamedPipeWithoutBlocking(t *testing.T) {
	name := `\\.\pipe\devcouncil-open-regular-` + strconv.Itoa(os.Getpid()) + `-` + strconv.FormatInt(time.Now().UnixNano(), 10)
	server, err := createNamedPipe(name)
	if err != nil {
		t.Fatalf("CreateNamedPipe: %v", err)
	}
	t.Cleanup(func() { _ = syscall.CloseHandle(server) })

	client, err := openPipeClient(name)
	if err != nil {
		t.Fatalf("occupying the only pipe instance: %v", err)
	}
	t.Cleanup(func() { _ = syscall.CloseHandle(client) })

	f, err := openWithin(t, name)
	if f != nil {
		_ = f.Close()
		t.Fatal("OpenRegular returned a named pipe")
	}
	var kind *NonRegularConfigError
	if !errors.As(err, &kind) || kind.Kind != "named pipe" {
		t.Fatalf("OpenRegular err = %v, want kind named pipe", err)
	}
}

func TestOpenRegularRefusesPipeSpellingsWithoutOpening(t *testing.T) {
	for _, path := range []string{
		`\\.\pipe\devcouncil-missing-` + strconv.Itoa(os.Getpid()),
		`//./pipe/devcouncil-missing`,
		`\\?\pipe\devcouncil-missing`,
		`\\127.0.0.1\pipe\devcouncil-missing`,
		`\\?\UNC\127.0.0.1\pipe\devcouncil-missing`,
		`\\.\pipe\帮助`,
		`\\.\GLOBALROOT\Device\NamedPipe\devcouncil-missing`,
	} {
		t.Run(path, func(t *testing.T) {
			f, err := openWithin(t, path)
			if f != nil {
				_ = f.Close()
				t.Fatal("OpenRegular returned a pipe path")
			}
			var kind *NonRegularConfigError
			if !errors.As(err, &kind) || kind.Kind != "named pipe" {
				t.Fatalf("OpenRegular err = %v, want kind named pipe", err)
			}
		})
	}
}

func TestOpenRegularOpensARegularFileOnWindows(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.yaml")
	body := []byte("gates:\n  mode: advisory\n")
	if err := os.WriteFile(path, body, 0o644); err != nil {
		t.Fatal(err)
	}
	f, err := openWithin(t, path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	read := make([]byte, len(body))
	n, err := f.Read(read)
	if err != nil {
		t.Fatal(err)
	}
	if string(read[:n]) != string(got) {
		t.Fatalf("read %q, file is %q", read[:n], got)
	}

	unicode := filepath.Join(t.TempDir(), "帮助.yaml")
	if err := os.WriteFile(unicode, body, 0o644); err != nil {
		t.Fatal(err)
	}
	uf, err := openWithin(t, unicode)
	if err != nil {
		t.Fatal(err)
	}
	_ = uf.Close()

	if len(path) >= 2 && path[1] == ':' {
		extended := `\\?\` + path
		ef, err := openWithin(t, extended)
		if err != nil {
			t.Fatalf("extended path %s: %v", extended, err)
		}
		_ = ef.Close()
	}
}

func TestOpenRegularRefusesADirectoryAndNULOnWindows(t *testing.T) {
	f, err := openWithin(t, t.TempDir())
	if f != nil {
		_ = f.Close()
		t.Fatal("OpenRegular returned a directory")
	}
	var kind *NonRegularConfigError
	if !errors.As(err, &kind) || kind.Kind != "directory" {
		t.Fatalf("directory err = %v, want kind directory", err)
	}

	f, err = openWithin(t, "NUL")
	if f != nil {
		_ = f.Close()
		t.Fatal("OpenRegular returned NUL")
	}
	if err == nil {
		t.Fatal("NUL opened as a config file")
	}
}

func openWithin(t *testing.T, path string) (*os.File, error) {
	t.Helper()
	type result struct {
		f   *os.File
		err error
	}
	done := make(chan result, 1)
	go func() {
		f, err := OpenRegular(path)
		done <- result{f, err}
	}()
	select {
	case got := <-done:
		return got.f, got.err
	case <-time.After(2 * time.Second):
		t.Fatalf("OpenRegular blocked on %s", path)
		return nil, errors.New("blocked")
	}
}

func createNamedPipe(name string) (syscall.Handle, error) {
	namep, err := syscall.UTF16PtrFromString(name)
	if err != nil {
		return 0, err
	}
	r, _, callErr := procCreateNamedPipeW.Call(
		uintptr(unsafe.Pointer(namep)),
		pipeAccessDuplex,
		pipeTypeByte,
		1,
		4096,
		4096,
		0,
		0,
	)
	h := syscall.Handle(r)
	if h == 0 || h == syscall.InvalidHandle {
		if callErr != nil && callErr != syscall.Errno(0) {
			return 0, callErr
		}
		return 0, syscall.EINVAL
	}
	return h, nil
}

func openPipeClient(name string) (syscall.Handle, error) {
	namep, err := syscall.UTF16PtrFromString(name)
	if err != nil {
		return 0, err
	}
	h, err := syscall.CreateFile(
		namep,
		syscall.GENERIC_READ|syscall.GENERIC_WRITE,
		syscall.FILE_SHARE_READ|syscall.FILE_SHARE_WRITE,
		nil,
		syscall.OPEN_EXISTING,
		0,
		0,
	)
	if err != nil {
		return 0, err
	}
	return h, nil
}
