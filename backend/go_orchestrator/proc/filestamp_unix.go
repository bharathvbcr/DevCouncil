//go:build unix

package proc

import (
	"errors"
	"io/fs"
	"os"
	"syscall"
)

// stampFile records what is at path now: its identity (device and inode),
// size and modification time. A file that cannot be examined stamps as
// failed, which never equals a stamp taken when it could be.
func stampFile(path string) fileStamp {
	info, err := os.Stat(path)
	if errors.Is(err, fs.ErrNotExist) {
		return fileStamp{}
	}
	if err != nil {
		return fileStamp{statFailed: true, mtime: -1}
	}
	stamp := fileStamp{exists: true, size: info.Size(), mtime: info.ModTime().UnixNano()}
	if sys, ok := info.Sys().(*syscall.Stat_t); ok {
		stamp.dev, stamp.ino = uint64(sys.Dev), uint64(sys.Ino)
	}
	return stamp
}
