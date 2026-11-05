//go:build unix

package flags

import (
	"os"
	"syscall"
)

// openNoFollow opens a regular file without following a symlink that replaced
// it after lstat, and without blocking if that replacement is a FIFO.
func openNoFollow(path string) (*os.File, error) {
	fd, err := syscall.Open(path, syscall.O_RDONLY|syscall.O_NONBLOCK|syscall.O_NOFOLLOW, 0)
	if err != nil {
		return nil, err
	}
	return os.NewFile(uintptr(fd), path), nil
}

// openFollow opens a dangling symlink. O_NONBLOCK keeps a FIFO that appeared
// in its place from parking the caller. The error is the open's own.
func openFollow(path string) (*os.File, error) {
	fd, err := syscall.Open(path, syscall.O_RDONLY|syscall.O_NONBLOCK, 0)
	if err != nil {
		return nil, err
	}
	return os.NewFile(uintptr(fd), path), nil
}
