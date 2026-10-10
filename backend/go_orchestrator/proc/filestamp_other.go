//go:build !unix

package proc

import (
	"errors"
	"io/fs"
	"os"
)

// stampFile records what is at path now: its size and modification time.
// Windows exposes no inode through os.FileInfo, so a file replaced by one of
// the same size within the clock's resolution is not seen; a session lasts
// one operation, which bounds that window.
func stampFile(path string) fileStamp {
	info, err := os.Stat(path)
	if errors.Is(err, fs.ErrNotExist) {
		return fileStamp{}
	}
	if err != nil {
		return fileStamp{statFailed: true, mtime: -1}
	}
	return fileStamp{exists: true, size: info.Size(), mtime: info.ModTime().UnixNano()}
}
