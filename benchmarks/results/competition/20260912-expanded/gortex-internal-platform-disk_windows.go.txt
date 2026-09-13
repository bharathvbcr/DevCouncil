//go:build windows

package platform

import "golang.org/x/sys/windows"

// DiskAvailBytes reports the bytes available to the calling user on the
// volume holding path (GetDiskFreeSpaceEx's caller-quota figure, so per-user
// quotas are respected). Callers gate disk-hungry maintenance (the store
// VACUUM needs up to a full temporary copy of the database) on this number.
func DiskAvailBytes(path string) (uint64, error) {
	p, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return 0, err
	}
	var availToCaller, total, totalFree uint64
	if err := windows.GetDiskFreeSpaceEx(p, &availToCaller, &total, &totalFree); err != nil {
		return 0, err
	}
	return availToCaller, nil
}
