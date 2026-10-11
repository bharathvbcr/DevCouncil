//go:build !unix

package gatescfg

import "os"

// inode is unavailable here; mtime, size and mode still key the cache.
func inode(os.FileInfo) uint64 { return 0 }
