//go:build windows

package safefile

// syncDirectory is a no-op on Windows: a directory cannot be opened and
// flushed the way it can elsewhere, and NTFS journals the rename itself.
func syncDirectory(string) error { return nil }
