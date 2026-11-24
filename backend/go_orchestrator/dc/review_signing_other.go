//go:build !windows

package dc

import (
	"fmt"
	"os"
)

// checkKeyFileAccess refuses a private key that group or others can read, the
// way an ssh client refuses a loose private key. On Unix the file mode is the
// access control, so the check is the mode.
func checkKeyFileAccess(path string, info os.FileInfo) error {
	if info.Mode().Perm()&0o077 != 0 {
		return fmt.Errorf("%s is readable by group or others (%o); a signing key must be 0600", path, info.Mode().Perm())
	}
	return nil
}

// CreateSigningKeyFile creates a new private key file for writing. It never
// overwrites an existing file (O_EXCL), and the file is 0600 from the moment it
// exists, before any secret byte is written.
func CreateSigningKeyFile(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
}
