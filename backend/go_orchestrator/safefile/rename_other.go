//go:build !windows

package safefile

// transientRename is false off Windows: a POSIX rename over an open file
// succeeds, so any error it returns is real and is returned at once.
func transientRename(error) bool { return false }
