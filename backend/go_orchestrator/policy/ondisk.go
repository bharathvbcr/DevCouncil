package policy

import (
	"os"
	"path/filepath"
	"strings"
)

// maxDirEntriesRead bounds the listing one path component costs. A directory
// holding more is not searched: the answer is "no other spelling found", which
// is what the pattern check alone would have said.
const maxDirEntriesRead = 4096

// onDiskSpelling returns rel as the filesystem stores it, when that differs from
// how rel is written, and whether it does.
//
// The secret and restricted rungs match patterns against text. On APFS and NTFS
// two different texts name one file, and a table of folding rules is a claim
// about what the filesystem does, not a fact about the file: `id_rſa`, a name
// that is canonically equivalent to another under the volume's own comparison,
// or any orbit a Unicode revision adds, passes a pattern that was written for
// the other spelling. For a path that already exists the question has an exact
// answer — which directory entry is this file — so each component is matched
// against its directory's entries by identity (os.SameFile) and the stored name
// is what the pattern check runs on a second time. It is the portable form of
// asking the kernel for the path an open file descriptor has (F_GETPATH).
//
// A path that does not exist yet has no stored spelling; the folded pattern
// match is all there is to ask, which is why both checks exist.
func onDiskSpelling(root, rel string) (string, bool) {
	if root == "" || rel == "" {
		return "", false
	}
	parts := strings.Split(rel, "/")
	dir := root
	stored := make([]string, 0, len(parts))
	for i, part := range parts {
		want, err := os.Lstat(filepath.Join(dir, part))
		if err != nil {
			stored = append(stored, parts[i:]...)
			break
		}
		name := storedName(dir, part, want)
		stored = append(stored, name)
		dir = filepath.Join(dir, name)
	}
	spelled := strings.Join(stored, "/")
	return spelled, spelled != rel
}

// storedName is the entry of dir that is the file named by part, or part when
// it cannot be told apart. The exact spelling is looked for first, in chunks, so
// the common case reads one chunk of a directory.
func storedName(dir, part string, want os.FileInfo) string {
	f, err := os.Open(dir)
	if err != nil {
		return part
	}
	defer f.Close()
	var others []os.DirEntry
	read := 0
	for read < maxDirEntriesRead {
		entries, err := f.ReadDir(256)
		for _, e := range entries {
			if e.Name() == part {
				return part
			}
			others = append(others, e)
		}
		read += len(entries)
		if err != nil || len(entries) == 0 {
			break
		}
	}
	for _, e := range others {
		if info, err := e.Info(); err == nil && os.SameFile(info, want) {
			return e.Name()
		}
	}
	return part
}
