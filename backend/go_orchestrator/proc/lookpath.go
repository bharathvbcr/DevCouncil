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
// canonical location is inside the repository `root` is in.
//
// The boundary is the repository's top, not `root` itself: a command run from
// repo/backend analyzes the whole checkout, and a program planted at
// repo/tools/bin is as much repository content as one under repo/backend.
// The top is found without running git (see repositoryTop): the nearest
// directory above `root` holding a `.git`, else the nearest holding a
// `.devcouncil` directory, else `root`. The walk stops at the nearest `.git`,
// so a home directory that is itself a repository does not swallow
// ~/.local/bin for every checkout beneath it.
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
//  2. PATH, through LookPathOutside, refusing anything inside the repository
//     root is in — its top, not root itself.
//  3. ~/.local/bin, where `devcouncil install` puts components, searched only
//     when PATH has no candidate at all, and held to the same canonical
//     outside-the-repository check. A PATH candidate that was refused stays refused: a
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

// resolveRoot identifies the containment boundary for root, where empty means
// the working directory: the repository root sits in, as repositoryTop finds
// it. A root that does not exist yields nil: it contains no program, and the
// caller's own command reports the missing root.
func resolveRoot(root string) (os.FileInfo, error) {
	if root == "" {
		var err error
		root, err = os.Getwd()
		if err != nil {
			return nil, err
		}
	}
	start, err := canonicalAbs(root)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	top, err := repositoryTop(start)
	if err != nil {
		return nil, err
	}
	return os.Stat(top)
}

// repositoryTop returns the top of the repository dir is in, found from the
// filesystem alone — no git runs to answer it, because the git that would
// answer is one of the programs being resolved.
//
// The walk goes up from dir and stops at the nearest directory holding a
// `.git` (a directory, or the file a worktree or submodule has); that
// directory is the top. With no `.git` above dir, the nearest directory
// holding a `.devcouncil` directory is the top, and with neither, dir itself.
// A `.devcouncil` below the nearest `.git` lies inside it, so the top is the
// outer of the two; one above the nearest `.git` is not consulted, and
// neither is any outer `.git`. Stopping there is deliberate: a home directory
// that is itself a repository (a dotfiles checkout) must not make every
// repository under it reach up and swallow ~/.local/bin.
//
// A marker that cannot be examined is an error, never an answer of "absent":
// answering "absent" would shrink the boundary.
func repositoryTop(dir string) (string, error) {
	devcouncil := ""
	for current := dir; ; {
		if _, err := os.Lstat(filepath.Join(current, ".git")); err == nil {
			return current, nil
		} else if !errors.Is(err, fs.ErrNotExist) {
			return "", fmt.Errorf("finding the repository around %s: %w", dir, err)
		}
		if devcouncil == "" {
			info, err := os.Stat(filepath.Join(current, ".devcouncil"))
			switch {
			case err == nil && info.IsDir():
				devcouncil = current
			case err != nil && !errors.Is(err, fs.ErrNotExist):
				return "", fmt.Errorf("finding the repository around %s: %w", dir, err)
			}
		}
		parent := filepath.Dir(current)
		if parent == current {
			break
		}
		current = parent
	}
	if devcouncil != "" {
		return devcouncil, nil
	}
	return dir, nil
}

// maxLinkHops bounds the symlink chain outsideRoot follows, matching the
// limit POSIX systems apply to path resolution.
const maxLinkHops = 40

// outsideRoot returns candidate's canonical path, refusing it when any name
// on the way to it is bound inside the directory rootDir identifies, or when
// it is not a regular file.
//
// Every link in candidate's chain is checked, not only where it ends. A
// repository need not ship a program to choose one: a link it carries,
// named git and pointing at /usr/bin/true, is a git that exits 0 with no
// output, and verification would read that as "nothing changed". The
// binding is repository content even when the bytes it points at are not.
func outsideRoot(name, candidate string, rootDir os.FileInfo) (string, error) {
	current, err := filepath.Abs(candidate)
	if err != nil {
		return "", err
	}
	for hops := 0; ; hops++ {
		if hops > maxLinkHops {
			return "", fmt.Errorf("%s resolves through more than %d links from %s", name, maxLinkHops, candidate)
		}
		// The directory that holds a name decides what the name means.
		holder, err := canonicalAbs(filepath.Dir(current))
		if err != nil {
			return "", err
		}
		inside, err := within(holder, rootDir)
		if err != nil {
			return "", fmt.Errorf("%s resolves through %s, whose location could not be checked: %w", name, current, err)
		}
		if inside {
			return "", fmt.Errorf("%s resolves through %s: %w", name, filepath.Join(holder, filepath.Base(current)), ErrRepositoryLocal)
		}
		link, err := os.Lstat(current)
		if err != nil {
			return "", err
		}
		if link.Mode()&os.ModeSymlink == 0 {
			break
		}
		target, err := os.Readlink(current)
		if err != nil {
			return "", err
		}
		if !filepath.IsAbs(target) {
			target = filepath.Join(holder, target)
		}
		current = target
	}
	resolved, err := canonicalAbs(current)
	if err != nil {
		return "", err
	}
	info, err := os.Stat(resolved)
	if err != nil {
		return "", err
	}
	if !info.Mode().IsRegular() {
		return "", fmt.Errorf("%s resolves to %s, which is not a regular file", name, resolved)
	}
	return resolved, nil
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
