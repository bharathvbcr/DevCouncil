//go:build darwin || linux

package console

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"unsafe"
)

func terminalWidth(file *os.File) (int, bool) {
	var size struct{ Rows, Cols, X, Y uint16 }
	conn, err := file.SyscallConn()
	if err != nil {
		return 80, false
	}
	var errno syscall.Errno
	err = conn.Control(func(fd uintptr) {
		_, _, errno = syscall.Syscall(syscall.SYS_IOCTL, fd, syscall.TIOCGWINSZ, uintptr(unsafe.Pointer(&size)))
	})
	if err != nil || errno != 0 {
		return 80, false
	}
	if size.Cols == 0 {
		return 80, true
	}
	return min(int(size.Cols), 4096), true
}

type terminalSink struct{ file *os.File }

func (s *terminalSink) Close() error { return s.file.Close() }
func (s *terminalSink) Write(bytes []byte) (int, error) {
	conn, err := s.file.SyscallConn()
	if err != nil {
		return 0, err
	}
	var n int
	var writeErr error
	err = conn.Control(func(fd uintptr) { n, writeErr = syscall.Write(int(fd), bytes) })
	if err != nil {
		return 0, err
	}
	return n, writeErr
}

func openTerminal(original *os.File) (*terminalSink, error) {
	before, err := original.Stat()
	if err != nil {
		return nil, err
	}
	var candidates []string
	if runtime.GOOS == "linux" {
		conn, err := original.SyscallConn()
		if err != nil {
			return nil, err
		}
		var link string
		err = conn.Control(func(fd uintptr) { link = fmt.Sprintf("/proc/self/fd/%d", fd) })
		if err != nil {
			return nil, err
		}
		target, err := os.Readlink(link)
		if err != nil {
			return nil, err
		}
		candidates = []string{target}
	} else {
		dir, err := os.Open("/dev")
		if err != nil {
			return nil, err
		}
		entries, readErr := dir.ReadDir(4096)
		closeErr := dir.Close()
		if readErr != nil && len(entries) == 0 {
			return nil, readErr
		}
		if closeErr != nil {
			return nil, closeErr
		}
		for _, entry := range entries {
			if strings.HasPrefix(entry.Name(), "tty") {
				candidates = append(candidates, filepath.Join("/dev", entry.Name()))
			}
		}
	}
	for _, path := range candidates {
		info, err := os.Stat(path)
		if err != nil || !os.SameFile(before, info) {
			continue
		}
		file, err := os.OpenFile(path, os.O_WRONLY|syscall.O_NONBLOCK|syscall.O_NOCTTY, 0)
		if err != nil {
			return nil, err
		}
		after, err := file.Stat()
		if err != nil || !os.SameFile(before, after) {
			_ = file.Close()
			return nil, fmt.Errorf("terminal identity changed")
		}
		return &terminalSink{file}, nil
	}
	return nil, fmt.Errorf("no independent terminal handle available")
}
