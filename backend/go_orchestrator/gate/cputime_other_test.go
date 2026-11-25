//go:build !unix

package gate

import "time"

// processCPU falls back to the wall clock where rusage is unavailable.
func processCPU() time.Duration { return time.Duration(time.Now().UnixNano()) }
