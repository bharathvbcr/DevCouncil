//go:build linux

package platform

import "golang.org/x/sys/unix"

// HostPhysicalMemoryBytes returns total physical RAM in bytes via
// sysinfo(2). Totalram is reported in units of Unit bytes; both fields are
// widened to uint64 so the multiply is correct on 32-bit arches too.
// Returns 0 when the syscall fails, so a caller that derives a budget from
// it can fall back cleanly to "host RAM unknown".
func HostPhysicalMemoryBytes() uint64 {
	var si unix.Sysinfo_t
	if err := unix.Sysinfo(&si); err != nil {
		return 0
	}
	unit := uint64(si.Unit)
	if unit == 0 {
		unit = 1
	}
	return uint64(si.Totalram) * unit
}
