//go:build !windows

package platform

import "golang.org/x/sys/unix"

// DiskAvailBytes reports the bytes available to an unprivileged caller on the
// filesystem holding path (statfs f_bavail × f_bsize — the root-reserved
// blocks are deliberately excluded, since the daemon does not run as root).
// Callers gate disk-hungry maintenance (the store VACUUM needs up to a full
// temporary copy of the database) on this number.
func DiskAvailBytes(path string) (uint64, error) {
	var st unix.Statfs_t
	if err := unix.Statfs(path, &st); err != nil {
		return 0, err
	}
	return uint64(st.Bavail) * uint64(st.Bsize), nil
}
