package safefile

import (
	"runtime"
	"testing"
)

func TestRootedOffRoot(t *testing.T) {
	if runtime.GOOS == "windows" {
		for _, p := range []string{"/etc/passwd", "C:foo", `\x`, "/"} {
			if !RootedOffRoot(p) {
				t.Errorf("RootedOffRoot(%q) = false on windows, want true", p)
			}
		}
		if RootedOffRoot(`C:\x`) || RootedOffRoot("rel/x") {
			t.Error("an absolute or a plain relative path is not rooted off root")
		}
		return
	}
	for _, p := range []string{"/etc/passwd", "/", "rel/x", "./a"} {
		if RootedOffRoot(p) {
			t.Errorf("RootedOffRoot(%q) = true, want false where / alone is absolute", p)
		}
	}
}
