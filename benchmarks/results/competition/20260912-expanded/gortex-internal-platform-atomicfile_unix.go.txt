//go:build !windows

package platform

// transientSharingError reports whether a failed rename, read or delete can
// succeed on a retry. It never can on POSIX: rename(2) and unlink(2) are
// atomic and no open handle can refuse them, so every error they return is a
// real one.
func transientSharingError(error) bool { return false }
