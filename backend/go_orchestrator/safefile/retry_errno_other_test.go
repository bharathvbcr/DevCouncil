//go:build !windows

package safefile

import "syscall"

// errorSharingViolationForTest is Windows' ERROR_SHARING_VIOLATION by number.
// No other platform names it a passing refusal, so it is not retried here.
var errorSharingViolationForTest error = syscall.Errno(32)
