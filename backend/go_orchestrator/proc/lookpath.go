package proc

import (
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// ErrRepositoryLocal reports that the only candidate found for a program was
// inside the repository under analysis.
var ErrRepositoryLocal = errors.New("candidate is inside the repository under analysis")

// LookPathOutside resolves `name` on PATH and refuses any candidate whose
// canonical location is inside `root`.
//
// Repository contents are input to analysis. They do not authorize executing a
// discovered program, so a build output, a vendored helper, or anything a
// relative PATH entry resolves to inside the checkout is refused by identity
// rather than by spelling: both sides are canonicalized first, so a PATH
// symlink or an alternate root spelling cannot smuggle repository content in
// as an implicitly trusted installation.
//
// An empty `root` means the current working directory, which is the root a
// command without an explicit `--project-root` is already operating on.
func LookPathOutside(name, root string) (string, error) {
	if root == "" {
		var err error
		root, err = os.Getwd()
		if err != nil {
			return "", err
		}
	}
	canonicalRoot, err := canonicalAbs(root)
	if err != nil {
		return "", err
	}
	candidate, err := exec.LookPath(name)
	if err != nil {
		return "", err
	}
	candidate, err = canonicalAbs(candidate)
	if err != nil {
		return "", err
	}
	relative, err := filepath.Rel(canonicalRoot, candidate)
	if err != nil {
		return "", err
	}
	if relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return "", fmt.Errorf("%s resolves to %s: %w", name, candidate, ErrRepositoryLocal)
	}
	info, err := os.Stat(candidate)
	if err != nil {
		return "", err
	}
	if !info.Mode().IsRegular() {
		return "", fmt.Errorf("%s resolves to %s, which is not a regular file", name, candidate)
	}
	return candidate, nil
}

func canonicalAbs(path string) (string, error) {
	resolved, err := filepath.EvalSymlinks(path)
	if err != nil {
		return "", err
	}
	return filepath.Abs(resolved)
}
