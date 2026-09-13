//go:build windows

package platform

import (
	"errors"
	"syscall"

	"golang.org/x/sys/windows"
)

// transientSharingError reports whether a failed rename, read or delete is the
// sharing failure another holder of the file causes, rather than a real one.
// Windows refuses to move, replace or delete a file while another handle to it
// is open unless that handle asked for FILE_SHARE_DELETE, which os.Open does
// not; the refusal surfaces as one of these codes and clears as soon as the
// other holder is done. ERROR_FILE_NOT_FOUND is deliberately absent: a
// vanished file means somebody else already claimed it, and waiting cannot
// bring it back.
func transientSharingError(err error) bool {
	for _, code := range [...]syscall.Errno{
		windows.ERROR_ACCESS_DENIED,
		windows.ERROR_SHARING_VIOLATION,
		windows.ERROR_LOCK_VIOLATION,
		windows.ERROR_USER_MAPPED_FILE,
	} {
		if errors.Is(err, code) {
			return true
		}
	}
	return false
}
