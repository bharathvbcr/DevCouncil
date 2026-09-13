//go:build !linux && !darwin

package platform

// HostPhysicalMemoryBytes has no portable reader on this platform, so it
// returns 0. A caller then treats host RAM as unknown and skips any policy
// that needs it (e.g. a RAM-derived default memory limit) rather than
// guessing. Linux and darwin carry real implementations.
func HostPhysicalMemoryBytes() uint64 { return 0 }
