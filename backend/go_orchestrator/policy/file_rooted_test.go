package policy

import (
	"runtime"
	"testing"
)

// A path that names a root or a volume without being absolute is outside the
// repository on every platform. On Windows "/etc/passwd" is rooted at the
// current drive and is not absolute, so a normaliser that joined it under the
// repository root answered "inside" for a file the kernel opens elsewhere: the
// permissive answer, from a gate whose only job is to fail closed.
func TestRootedPathsThatAreNotAbsoluteAreOutsideTheRoot(t *testing.T) {
	root := t.TempDir()
	cases := []string{"/etc/passwd", "/tmp/x", `\etc\passwd`, "//server/share/x"}
	if runtime.GOOS == "windows" {
		cases = append(cases, "C:foo", "D:bar/baz")
	}
	for _, raw := range cases {
		if rel, outside := NormalizeRepoPath(root, raw); !outside {
			t.Errorf("NormalizeRepoPath(root, %q) = %q, inside; want outside the root", raw, rel)
		}
	}
}

func TestRootedOffRootIsFalseWhereASlashIsAbsolute(t *testing.T) {
	if runtime.GOOS == "windows" {
		for _, p := range []string{"/etc/passwd", "C:foo", `\x`} {
			if !rootedOffRoot(p) {
				t.Errorf("rootedOffRoot(%q) = false on windows, want true", p)
			}
		}
		if rootedOffRoot(`C:\x`) || rootedOffRoot("rel/x") {
			t.Error("an absolute or a plain relative path is not rooted off root")
		}
		return
	}
	for _, p := range []string{"/etc/passwd", "rel/x", "./a"} {
		if rootedOffRoot(p) {
			t.Errorf("rootedOffRoot(%q) = true, want false where / alone is absolute", p)
		}
	}
}
