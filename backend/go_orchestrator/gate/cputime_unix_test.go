//go:build unix

package gate

import (
	"syscall"
	"time"
)

// processCPU is the CPU time this process has used (user + system). Unlike
// wall time it does not grow while the process waits for a core, so a timing
// test on a shared runner measures the work, not the neighbours.
func processCPU() time.Duration {
	var ru syscall.Rusage
	if err := syscall.Getrusage(syscall.RUSAGE_SELF, &ru); err != nil {
		return time.Duration(time.Now().UnixNano())
	}
	return time.Duration(ru.Utime.Nano() + ru.Stime.Nano())
}
