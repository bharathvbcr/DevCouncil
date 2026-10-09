package proc

import (
	"errors"
	"fmt"
	"io/fs"
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
// rather than by spelling. Symlinks are resolved first, and containment is
// then decided by comparing directories with os.SameFile, not their names:
// a filesystem that folds case or Unicode normalization (macOS APFS, Windows
// NTFS) gives one directory many spellings, and a PATH entry or root spelled
// another way must not smuggle repository content in as an implicitly
// trusted installation.
//
// An empty `root` means the current working directory, which is the root a
// command without an explicit `--project-root` is already operating on.
func LookPathOutside(name, root string) (string, error) {
	rootDir, err := resolveRoot(root)
	if err != nil {
		return "", err
	}
	candidate, err := exec.LookPath(name)
	if err != nil {
		return "", err
	}
	return outsideRoot(name, candidate, rootDir)
}

// Override is an operator's explicit path for a component, and the flag or
// environment variable it came from, which a refusal names.
type Override struct {
	Path   string
	Source string
}

// EnvOverride reads an override from an environment variable. An empty or
// whitespace-only value is unset.
func EnvOverride(env string) Override {
	return Override{Path: strings.TrimSpace(os.Getenv(env)), Source: env}
}

// ResolveComponent names the program a component client runs — dcstore,
// devmap, dcverify, dcgrep — and is the one place each of them is found.
//
// Three rungs, in order:
//
//  1. An override is used or refused, never replaced by a search. It may name
//     a path inside root: setting it is a deliberate act by someone with a
//     shell, not something repository contents can arrange. It is made
//     absolute against the working directory before it is checked, and
//     handed on absolute, so a client that runs in another directory runs
//     the file that was checked.
//  2. PATH, through LookPathOutside.
//  3. ~/.local/bin, where `devcouncil install` puts components, searched only
//     when PATH has no candidate at all, and held to the same canonical
//     outside-root check. A PATH candidate that was refused stays refused: a
//     refusal that fell through to another rung would hide that something in
//     the repository is on PATH.
//
// A component found nowhere returns an error satisfying
// errors.Is(err, exec.ErrNotFound); one refused for its location satisfies
// errors.Is(err, ErrRepositoryLocal) or exec.ErrDot.
func ResolveComponent(name, root string, override Override) (string, error) {
	if override.Path != "" {
		path, err := filepath.Abs(override.Path)
		if err != nil {
			return "", fmt.Errorf("%s names %s: %w (an explicit path is used or refused, never replaced)",
				override.Source, override.Path, err)
		}
		info, err := os.Stat(path)
		if err != nil {
			return "", fmt.Errorf("%s names %s: %w (an explicit path is used or refused, never replaced)",
				override.Source, override.Path, err)
		}
		if info.IsDir() {
			return "", fmt.Errorf("%s names %s, which is a directory (an explicit path is used or refused, never replaced)",
				override.Source, override.Path)
		}
		return path, nil
	}
	found, err := LookPathOutside(name, root)
	if err == nil || !errors.Is(err, exec.ErrNotFound) {
		return found, err
	}
	home, homeErr := os.UserHomeDir()
	if homeErr != nil || home == "" {
		return "", fmt.Errorf("%s is not on PATH, and there is no home directory to search: %w", name, err)
	}
	localBin := filepath.Join(home, ".local", "bin")
	// A path with a separator is checked rather than searched for, so this
	// applies the same executable test PATH candidates get.
	candidate, localErr := exec.LookPath(filepath.Join(localBin, name))
	if localErr != nil {
		if errors.Is(localErr, fs.ErrNotExist) {
			return "", fmt.Errorf("%s is not on PATH or in %s: %w", name, localBin, exec.ErrNotFound)
		}
		return "", localErr
	}
	rootDir, err := resolveRoot(root)
	if err != nil {
		return "", err
	}
	return outsideRoot(name, candidate, rootDir)
}

// resolveRoot identifies root, where empty means the working directory. A
// root that does not exist yields nil: it contains no program, and the
// caller's own command reports the missing root.
func resolveRoot(root string) (os.FileInfo, error) {
	if root == "" {
		var err error
		root, err = os.Getwd()
		if err != nil {
			return nil, err
		}
	}
	info, err := os.Stat(root)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	return info, err
}

// outsideRoot returns candidate's canonical path, refusing it when that lies
// inside the directory rootDir identifies or is not a regular file.
func outsideRoot(name, candidate string, rootDir os.FileInfo) (string, error) {
	candidate, err := canonicalAbs(candidate)
	if err != nil {
		return "", err
	}
	inside, err := within(candidate, rootDir)
	if err != nil {
		return "", fmt.Errorf("%s resolves to %s, whose location could not be checked: %w", name, candidate, err)
	}
	if inside {
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

// within reports whether path, or any directory above it, is dir. Each
// ancestor is compared by identity; one that cannot be examined is an error,
// never an answer of "outside". A nil dir is a root that does not exist.
func within(path string, dir os.FileInfo) (bool, error) {
	if dir == nil {
		return false, nil
	}
	for current := path; ; {
		info, err := os.Stat(current)
		if err != nil {
			return false, err
		}
		if os.SameFile(info, dir) {
			return true, nil
		}
		parent := filepath.Dir(current)
		if parent == current {
			return false, nil
		}
		current = parent
	}
}

func canonicalAbs(path string) (string, error) {
	resolved, err := filepath.EvalSymlinks(path)
	if err != nil {
		return "", err
	}
	return filepath.Abs(resolved)
}
