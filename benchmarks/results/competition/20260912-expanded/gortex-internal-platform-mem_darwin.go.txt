//go:build darwin

package platform

import "golang.org/x/sys/unix"

// HostPhysicalMemoryBytes returns total physical RAM in bytes via the
// hw.memsize sysctl. Returns 0 when the sysctl is unavailable, so a caller
// that derives a budget from it can fall back cleanly to "host RAM unknown"
// rather than acting on a bogus figure.
func HostPhysicalMemoryBytes() uint64 {
	n, err := unix.SysctlUint64("hw.memsize")
	if err != nil {
		return 0
	}
	return n
}
