package safefile

import (
	"errors"
	"syscall"
)

// Win32 error codes package syscall does not name.
const (
	errorSharingViolation syscall.Errno = 32 // ERROR_SHARING_VIOLATION
	errorLockViolation    syscall.Errno = 33 // ERROR_LOCK_VIOLATION
)

// transientRename reports whether a failed rename is a refusal that ends when
// another handle on the destination closes: access denied, a sharing violation
// or a lock violation.
func transientRename(err error) bool {
	return errors.Is(err, syscall.ERROR_ACCESS_DENIED) ||
		errors.Is(err, errorSharingViolation) ||
		errors.Is(err, errorLockViolation)
}
