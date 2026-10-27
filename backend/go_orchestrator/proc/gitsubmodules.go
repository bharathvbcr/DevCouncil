package proc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	// maxSubmodules bounds how many submodules one dirtiness read examines;
	// more is refused, never sampled.
	maxSubmodules = 256
	// maxSubmoduleDepth bounds nesting.
	maxSubmoduleDepth = 8
	// submoduleListingLimit bounds the index listing gitlinks are read from.
	submoduleListingLimit = 64 << 20
)

// DirtySubmodules names the submodules under the session's root (within
// pathspecs, when any are given) whose working trees have changes to
// tracked files — what `git diff` reported as `Subproject commit …-dirty`
// before the guards passed --ignore-submodules=dirty.
//
// git answers that question by running status inside each submodule, and
// that status runs the submodule's own filter drivers and fsmonitor, which
// the superproject's config listing never sees. Here each submodule is
// asked with a guarded read of its own instead: its own config is read and
// its own filters neutralized, and nested submodules are asked the same
// way, to maxSubmoduleDepth. The git is the superproject's: a submodule is
// inside the repository under analysis, and the program that reads it is
// still resolved outside the superproject's root.
//
// Gitlinks come from the index; a repository without .gitmodules has no
// submodule git would describe, and costs no spawn. A gitlink whose path is
// not a plain directory inside root holding a .git is not read: one with no
// .git is not checked out, and anything else — a symlink, a path that
// leaves root — is refused.
func (s *GitSession) DirtySubmodules(ctx context.Context, pathspecs ...string) ([]string, error) {
	return s.dirtySubmodules(ctx, 0, pathspecs)
}

func (s *GitSession) dirtySubmodules(ctx context.Context, depth int, pathspecs []string) ([]string, error) {
	if _, err := os.Lstat(filepath.Join(s.root, ".gitmodules")); errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	} else if err != nil {
		return nil, err
	}
	if depth >= maxSubmoduleDepth {
		return nil, fmt.Errorf("submodules under %s nest deeper than %d; refusing to report a partial answer", s.root, maxSubmoduleDepth)
	}
	args := []string{"ls-files", "--stage", "-z"}
	if len(pathspecs) > 0 {
		args = append(append(args, "--"), pathspecs...)
	}
	listing, err := s.output(ctx, submoduleListingLimit, args...)
	if err != nil {
		return nil, err
	}
	var gitlinks []string
	for _, entry := range bytes.Split(listing, []byte{0}) {
		meta, path, ok := strings.Cut(string(entry), "\t")
		if ok && strings.HasPrefix(meta, "160000 ") {
			gitlinks = append(gitlinks, path)
		}
	}
	if len(gitlinks) > maxSubmodules {
		return nil, fmt.Errorf("%s has %d submodules, more than the %d one read examines; refusing to report a partial answer", s.root, len(gitlinks), maxSubmodules)
	}
	var dirty []string
	for _, path := range gitlinks {
		dir, ok, err := s.submoduleDir(path)
		if err != nil {
			return nil, err
		}
		if !ok {
			continue
		}
		sub := &GitSession{root: dir, git: s.git}
		changed, err := sub.output(ctx, 1<<20, "diff", "HEAD", "--name-only", "-z")
		if err != nil {
			return nil, fmt.Errorf("submodule %s: %w", path, err)
		}
		if len(bytes.Trim(changed, "\x00")) == 0 {
			nested, err := sub.dirtySubmodules(ctx, depth+1, nil)
			if err != nil {
				return nil, fmt.Errorf("submodule %s: %w", path, err)
			}
			if len(nested) == 0 {
				continue
			}
		}
		dirty = append(dirty, path)
	}
	return dirty, nil
}

// SubmoduleWorktreeFiles names the tracked files a dirty submodule has changed,
// as superproject-relative paths. The listing is the guarded
// `diff HEAD --name-only -z` this session already uses: the superproject's
// git, the submodule's filters and fsmonitor not run, symlink gitlinks refused.
// A deletion (named, not on disk) is omitted. A symlink, a path that escapes
// the submodule, or a read that fails is an error — never a followed read and
// never a partial list reported as complete.
func (s *GitSession) SubmoduleWorktreeFiles(ctx context.Context, submodulePath string) ([]string, error) {
	if !filepath.IsLocal(filepath.FromSlash(submodulePath)) {
		return nil, fmt.Errorf("refusing submodule path %q", submodulePath)
	}
	return s.submoduleWorktreeFiles(ctx, submodulePath, 1)
}

func (s *GitSession) submoduleWorktreeFiles(ctx context.Context, submodulePath string, depth int) ([]string, error) {
	if depth > maxSubmoduleDepth {
		return nil, fmt.Errorf("submodules under %s nest deeper than %d; refusing to report a partial answer", s.root, maxSubmoduleDepth)
	}
	dir, ok, err := s.submoduleDir(submodulePath)
	if err != nil {
		return nil, err
	}
	if !ok {
		return nil, fmt.Errorf("submodule %s is not a checked-out directory", submodulePath)
	}
	sub := &GitSession{root: dir, git: s.git}
	listing, err := sub.output(ctx, 1<<20, "diff", "HEAD", "--name-only", "-z")
	if err != nil {
		return nil, fmt.Errorf("submodule %s: %w", submodulePath, err)
	}
	var out []string
	seen := map[string]struct{}{}
	add := func(rel string) {
		if _, ok := seen[rel]; ok {
			return
		}
		seen[rel] = struct{}{}
		out = append(out, rel)
	}
	for _, raw := range bytes.Split(listing, []byte{0}) {
		name := strings.TrimSpace(strings.ReplaceAll(string(raw), "\\", "/"))
		if name == "" {
			continue
		}
		if !filepath.IsLocal(filepath.FromSlash(name)) {
			return nil, fmt.Errorf("submodule %s names %q, which is not a plain path", submodulePath, name)
		}
		info, missing, serr := lstatWithin(dir, name)
		if serr != nil {
			return nil, fmt.Errorf("submodule %s: %s", submodulePath, serr.Error())
		}
		if missing {
			continue
		}
		rel := pathJoinSlash(submodulePath, name)
		if info.Mode()&os.ModeSymlink != 0 {
			return nil, fmt.Errorf("refusing to follow symlink %s", rel)
		}
		if info.IsDir() {
			nested, nerr := sub.submoduleWorktreeFiles(ctx, name, depth+1)
			if nerr != nil {
				return nil, nerr
			}
			for _, n := range nested {
				add(pathJoinSlash(submodulePath, n))
			}
			continue
		}
		if !info.Mode().IsRegular() {
			return nil, fmt.Errorf("submodule %s: %s is not a regular file", submodulePath, name)
		}
		add(rel)
	}
	// Name-only is run with --ignore-submodules=dirty, so a nested dirty
	// submodule is not in the listing above. Ask the same guarded walk
	// DirtySubmodules uses, one level down, and do not reset its depth.
	nested, err := sub.dirtySubmodules(ctx, depth, nil)
	if err != nil {
		return nil, fmt.Errorf("submodule %s: %w", submodulePath, err)
	}
	for _, name := range nested {
		inner, err := sub.submoduleWorktreeFiles(ctx, name, depth+1)
		if err != nil {
			return nil, err
		}
		for _, n := range inner {
			add(pathJoinSlash(submodulePath, n))
		}
	}
	return out, nil
}

// lstatWithin stats name under dir without following a symlink at any component.
// missing is a final component that is not on disk (a deletion).
func lstatWithin(dir, name string) (info fs.FileInfo, missing bool, err error) {
	current := dir
	parts := strings.Split(filepath.FromSlash(name), string(filepath.Separator))
	for i, part := range parts {
		if part == "" || part == "." || part == ".." {
			return nil, false, fmt.Errorf("%q is not a plain path", name)
		}
		current = filepath.Join(current, part)
		info, err = os.Lstat(current)
		if err != nil {
			if errors.Is(err, fs.ErrNotExist) && i == len(parts)-1 {
				return nil, true, nil
			}
			return nil, false, err
		}
		if info.Mode()&os.ModeSymlink != 0 {
			return info, false, fmt.Errorf("refusing to follow symlink %s", name)
		}
		if i < len(parts)-1 && !info.IsDir() {
			return nil, false, fmt.Errorf("%s is not a directory", name)
		}
	}
	return info, false, nil
}

func pathJoinSlash(parent, name string) string {
	if parent == "" {
		return name
	}
	return parent + "/" + strings.TrimPrefix(name, "/")
}

// submoduleDir returns the checked-out directory of the gitlink at path. ok
// is false for a gitlink that is not checked out.
func (s *GitSession) submoduleDir(path string) (string, bool, error) {
	native := filepath.FromSlash(path)
	if path == "" || filepath.IsAbs(native) || filepath.Clean(native) != native || native == ".." ||
		strings.HasPrefix(native, ".."+string(filepath.Separator)) {
		return "", false, fmt.Errorf("the index names a submodule at %q, which is not a plain path inside %s", path, s.root)
	}
	dir := filepath.Join(s.root, native)
	// Every component on the way must be a real directory: a symlink in the
	// path would carry the read outside the repository.
	current := s.root
	for _, part := range strings.Split(native, string(filepath.Separator)) {
		current = filepath.Join(current, part)
		info, err := os.Lstat(current)
		if errors.Is(err, fs.ErrNotExist) {
			return "", false, nil
		}
		if err != nil {
			return "", false, err
		}
		if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			return "", false, fmt.Errorf("submodule %s: %s is not a directory", path, current)
		}
	}
	if _, err := os.Lstat(filepath.Join(dir, ".git")); errors.Is(err, fs.ErrNotExist) {
		return "", false, nil
	} else if err != nil {
		return "", false, err
	}
	return dir, true, nil
}

// output runs one guarded read and returns its stdout. A read whose output
// passes limit is refused, never truncated.
func (s *GitSession) output(ctx context.Context, limit int, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	cmd, err := s.Command(ctx, args...)
	if err != nil {
		return nil, err
	}
	out := &CappedBuffer{Limit: limit}
	var stderr bytes.Buffer
	cmd.Stdout = out
	cmd.Stderr = &stderr
	cmd.WaitDelay = 2 * time.Second
	err, timedOut := RunBounded(ctx, cmd.Run)
	if timedOut {
		return nil, fmt.Errorf("git %s in %s did not finish: %w", strings.Join(args, " "), s.root, ctx.Err())
	}
	if err != nil {
		return nil, fmt.Errorf("git %s in %s: %w: %s", strings.Join(args, " "), s.root, err, strings.TrimSpace(stderr.String()))
	}
	if out.Overflowed() {
		return nil, fmt.Errorf("git %s in %s wrote more than %d bytes; refusing a truncated answer", strings.Join(args, " "), s.root, limit)
	}
	return out.Bytes(), nil
}
