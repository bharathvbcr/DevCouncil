package safefile_test

import (
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// The rule, case by case. These are the shapes a child process could report
// back that would name something outside the repository it was asked about —
// which is why this is a containment check and not a tidiness one.
func TestValidRepoPathRefusesEscapes(t *testing.T) {
	for _, tc := range []struct {
		name string
		path string
		want string
	}{
		{"empty", "", "names no file"},
		{"absolute posix", "/etc/passwd", "is absolute"},
		{"absolute windows separator", `\windows\system32`, "is absolute"},
		{"parent at the front", "../outside", "climbs out"},
		{"parent in the middle", "src/../../outside", "climbs out"},
		{"parent at the end", "src/..", "climbs out"},
		{"bare parent", "..", "climbs out"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			err := safefile.ValidRepoPath(tc.path)
			if err == nil {
				t.Fatalf("ValidRepoPath(%q) = nil, want a refusal", tc.path)
			}
			if !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("ValidRepoPath(%q) = %q, want it to mention %q", tc.path, err, tc.want)
			}
		})
	}
}

// The volume guard is platform-conditional, and saying so is the point of this
// test.
//
// `filepath.VolumeName` is implemented per GOOS: on Windows it reads `C:` off
// the front, and on every Unix it returns the empty string unconditionally. So
// the "names a volume" branch is live on Windows and inert everywhere else —
// which is correct rather than broken, because on Unix `C:\windows` is not an
// escape at all, just a file with an unusual name.
//
// Written this way because the alternative readings are both wrong: asserting
// a refusal everywhere fails on Unix, and asserting acceptance everywhere
// would fail on Windows and quietly pass on the CI that never runs there.
func TestValidRepoPathVolumeGuardIsPlatformConditional(t *testing.T) {
	err := safefile.ValidRepoPath(`C:\windows`)
	if runtime.GOOS == "windows" {
		if err == nil {
			t.Fatal(`on Windows, C:\windows names a volume and must be refused`)
		}
		if !strings.Contains(err.Error(), "names a volume") {
			t.Fatalf("got %q, want it to name the volume", err)
		}
		return
	}
	if err != nil {
		t.Fatalf(
			"on %s a colon and a backslash are ordinary filename characters, so "+
				"this is a relative path and not an escape; got %v",
			runtime.GOOS, err,
		)
	}
}

func TestValidRepoPathRefusesNonUTF8(t *testing.T) {
	// A lone continuation byte is not valid UTF-8, so the name cannot be
	// reopened by the string it was reported as.
	err := safefile.ValidRepoPath("src/" + string([]byte{0x80}) + ".go")
	if err == nil {
		t.Fatal("a non-UTF-8 path must be refused")
	}
	if !strings.Contains(err.Error(), "UTF-8") {
		t.Fatalf("got %q, want it to name the encoding", err)
	}
}

func TestValidRepoPathAcceptsOrdinaryPaths(t *testing.T) {
	for _, path := range []string{
		"main.go",
		"src/lib.rs",
		"a/b/c/d.txt",
		// A leading dot is a hidden file, not a climb.
		".github/workflows/ci.yml",
		// A `..` *inside* an element is part of a name, not a climb: this is
		// the case a naive `strings.Contains(path, "..")` gets wrong, and
		// refusing it would reject legitimate files.
		"src/foo..bar.go",
		"weird..name",
		// Non-ASCII is ordinary.
		"src/café/módulo.rs",
		// A leading dash is a path here; it is the argv layer's job to keep it
		// from being read as a flag.
		"--weird.rs",
	} {
		if err := safefile.ValidRepoPath(path); err != nil {
			t.Fatalf("ValidRepoPath(%q) = %v, want it accepted", path, err)
		}
	}
}

// The property that makes this worth having in one place: the check is on the
// path's *elements*, so a substring match cannot be the implementation.
func TestADoubleDotInsideANameIsNotAClimb(t *testing.T) {
	if err := safefile.ValidRepoPath("src/..hidden"); err != nil {
		t.Fatalf("`..hidden` is a filename beginning with two dots, not a parent reference: %v", err)
	}
	if err := safefile.ValidRepoPath("src/.."); err == nil {
		t.Fatal("`..` as a whole element is a parent reference and must be refused")
	}
}
