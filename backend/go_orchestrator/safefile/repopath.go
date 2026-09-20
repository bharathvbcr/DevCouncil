package safefile

import (
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"unicode/utf8"
)

// ValidRepoPath refuses anything that is not a repository-relative file path.
//
// The one owner of that question. Two byte-identical copies of this lived in
// dc/dcgrep and dc/dcverify — both subprocess boundaries, both validating
// paths a child process reported back, and both therefore places where a
// hardening fix applied to one would silently leave the other unfixed. A path
// validator is exactly the kind of code that must not have a second
// implementation: the failure mode is not a wrong answer, it is a containment
// check that was tightened in one boundary and not the other.
//
// It lives in safefile because that is where repository containment already
// lives — OpenNoFollow refuses to escape a root, and this refuses to name
// somewhere outside one. The two are the same concern at different layers.
//
// The rules, each of which is a way a child could name something outside the
// tree it was asked about:
//
//   - empty, which names no file at all;
//   - absolute, in either separator, which is not relative to anything;
//   - carrying a volume name, which is the Windows spelling of absolute;
//   - containing a `..` element, which climbs out;
//   - not valid UTF-8, which cannot be reopened by the name given.
//
// Error text is preserved verbatim from the two implementations this replaces,
// so nothing reading these messages sees a change.
func ValidRepoPath(path string) error {
	if path == "" {
		return errors.New("is empty, so it names no file")
	}
	if strings.HasPrefix(path, "/") || strings.HasPrefix(path, `\`) {
		return fmt.Errorf("%q is absolute, and every path here is relative to the repository", path)
	}
	if vol := filepath.VolumeName(filepath.FromSlash(path)); vol != "" {
		return fmt.Errorf("%q names a volume, so it is not inside the repository", path)
	}
	for _, element := range strings.Split(path, "/") {
		if element == ".." {
			return fmt.Errorf("%q climbs out of the repository", path)
		}
	}
	if !utf8.ValidString(path) {
		return fmt.Errorf("%q is not valid UTF-8, so it names no file that can be reopened", path)
	}
	return nil
}
