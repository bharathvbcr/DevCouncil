package safefile

import (
	"path/filepath"
	"strings"
)

// RootedOffRoot reports whether p names a place by a root or a volume without
// being absolute, which only happens where an absolute path needs a drive:
// "/etc/passwd" (rooted at the current drive), `\\x` and "C:foo" (relative to
// that drive's current directory) on Windows. Where "/" alone makes a path
// absolute, nothing is rooted off root and this is false.
//
// A check that refuses absolute paths with filepath.IsAbs must also refuse
// these: IsAbs is false for them on Windows, yet the kernel opens them outside
// the directory they were joined under. Treat them as not inside, fail closed.
func RootedOffRoot(p string) bool {
	if filepath.IsAbs(p) {
		return false
	}
	return filepath.VolumeName(p) != "" || strings.HasPrefix(p, "/") || strings.HasPrefix(p, `\`)
}
