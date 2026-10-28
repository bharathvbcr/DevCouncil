package proc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

const (
	// maxSubmodules bounds how many submodules one dirtiness read examines;
	// more is refused, never sampled.
	maxSubmodules = 256
	// maxSubmoduleDepth bounds nesting.
	maxSubmoduleDepth = 8
)

// submoduleWalkBudget is the one deadline over a whole submodule walk. Each
// git call also has its own 60 s bound, but up to maxSubmodules calls at that
// bound each is a walk of hours; the walk as a whole is held to this instead,
// and the per-call bound is derived from it, so no single call outlives it.
// A var so a test can shorten it.
var submoduleWalkBudget = 3 * time.Minute

type walkBudgetKey struct{}

// WithWalkBudget starts the one deadline of a submodule walk. A caller that
// makes several walk calls (DirtySubmodules, then SubmoduleWorktreeFiles for
// each dirty one) calls it once and passes the context to all of them: the
// calls then share one budget instead of each starting its own, which was up
// to 257 of them end to end. Called on a context that already carries a budget
// it changes nothing, so DirtySubmodules and SubmoduleWorktreeFiles apply it
// themselves for the caller that makes only one call.
func WithWalkBudget(ctx context.Context) (context.Context, context.CancelFunc) {
	if ctx.Value(walkBudgetKey{}) != nil {
		return ctx, func() {}
	}
	ctx, cancel := context.WithTimeout(ctx, submoduleWalkBudget)
	return context.WithValue(ctx, walkBudgetKey{}, struct{}{}), cancel
}

// DirtySubmodules names the submodules under the session's root (within
// pathspecs, when any are given) whose working trees hold a change: a change
// to a tracked file, a file git does not track (git calls both "modified
// content"), or a dirty submodule of their own. It is what `git diff` reported
// as `Subproject commit …-dirty` before the guards passed
// --ignore-submodules=dirty.
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
// Gitlinks come from the index (mode 160000), whether or not .gitmodules
// names them: git describes a gitlink with no .gitmodules entry as a submodule
// all the same, and a read that skipped it for want of one passed its bytes
// silently. A gitlink whose path is not a plain directory inside root holding
// a .git is not read: a missing or empty directory is not checked out, a
// directory with files in it but no .git is refused (no listing names those
// files), and anything else — a symlink, a path that leaves root — is
// refused.
//
// The whole walk runs under submoduleWalkBudget.
func (s *GitSession) DirtySubmodules(ctx context.Context, pathspecs ...string) ([]string, error) {
	ctx, cancel := WithWalkBudget(ctx)
	defer cancel()
	return s.dirtySubmodules(ctx, 0, pathspecs)
}

func (s *GitSession) dirtySubmodules(ctx context.Context, depth int, pathspecs []string) ([]string, error) {
	args := []string{"ls-files", "--stage", "-z"}
	if len(pathspecs) > 0 {
		args = append(append(args, "--"), pathspecs...)
	}
	gitlinks, err := s.gitlinks(ctx, args)
	if err != nil {
		return nil, err
	}
	if len(gitlinks) == 0 {
		return nil, nil
	}
	if depth >= maxSubmoduleDepth {
		return nil, fmt.Errorf("submodules under %s nest deeper than %d; refusing to report a partial answer", s.root, maxSubmoduleDepth)
	}
	var dirty []string
	for _, path := range gitlinks {
		if err := ctx.Err(); err != nil {
			return nil, fmt.Errorf("the submodule walk did not finish within %s: %w", submoduleWalkBudget, err)
		}
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
			untracked, err := sub.output(ctx, 1<<20, "ls-files", "--others", "--exclude-standard", "-z")
			if err != nil {
				return nil, fmt.Errorf("submodule %s: %w", path, err)
			}
			if len(bytes.Trim(untracked, "\x00")) == 0 {
				nested, err := sub.dirtySubmodules(ctx, depth+1, nil)
				if err != nil {
					return nil, fmt.Errorf("submodule %s: %w", path, err)
				}
				if len(nested) == 0 {
					continue
				}
			}
		}
		dirty = append(dirty, path)
	}
	return dirty, nil
}

// NestedRepositoryError is the refusal for an untracked directory git lists
// with a trailing slash: a repository nested inside the tree that is neither
// ignored nor a submodule. Its files are in no listing, so none can be scanned;
// the change is refused, and the message says what to do about it.
func NestedRepositoryError(dir string) error {
	return fmt.Errorf("untracked nested repository %s is not scanned: ignore it or add it as a submodule", dir)
}

// SubmoduleChanges is what a dirty submodule's working tree changed, as
// superproject-relative paths.
type SubmoduleChanges struct {
	// Files are changed files that are on disk, and so have bytes to read.
	Files []string
	// Deleted are changed files that are named but no longer on disk. A
	// deletion has no bytes: it is a change with nothing to scan, which is not
	// the same as a read that failed.
	Deleted []string
}

// Empty reports that git named no change at all.
func (c SubmoduleChanges) Empty() bool { return len(c.Files) == 0 && len(c.Deleted) == 0 }

// SubmoduleWorktreeFiles names the tracked files a dirty submodule has changed,
// as superproject-relative paths. The listing is the guarded
// `diff HEAD --name-only -z` this session already uses: the superproject's
// git, the submodule's filters and fsmonitor not run, symlink gitlinks refused.
// A deletion (named, not on disk) is reported in Deleted. A symlink, a path
// that escapes the submodule, or a read that fails is an error — never a
// followed read and never a partial list reported as complete.
func (s *GitSession) SubmoduleWorktreeFiles(ctx context.Context, submodulePath string) (SubmoduleChanges, error) {
	if !filepath.IsLocal(filepath.FromSlash(submodulePath)) {
		return SubmoduleChanges{}, fmt.Errorf("refusing submodule path %q", submodulePath)
	}
	ctx, cancel := WithWalkBudget(ctx)
	defer cancel()
	return s.submoduleWorktreeFiles(ctx, submodulePath, 1)
}

func (s *GitSession) submoduleWorktreeFiles(ctx context.Context, submodulePath string, depth int) (SubmoduleChanges, error) {
	if depth > maxSubmoduleDepth {
		return SubmoduleChanges{}, fmt.Errorf("submodules under %s nest deeper than %d; refusing to report a partial answer", s.root, maxSubmoduleDepth)
	}
	dir, ok, err := s.submoduleDir(submodulePath)
	if err != nil {
		return SubmoduleChanges{}, err
	}
	if !ok {
		return SubmoduleChanges{}, fmt.Errorf("submodule %s is not a checked-out directory", submodulePath)
	}
	sub := &GitSession{root: dir, git: s.git}
	// The tracked changes git names, then the files it does not track: a file
	// created inside the submodule is "modified content" to git and appears in
	// neither listing of the superproject.
	tracked, err := sub.output(ctx, 1<<20, "diff", "HEAD", "--name-only", "-z")
	if err != nil {
		return SubmoduleChanges{}, fmt.Errorf("submodule %s: %w", submodulePath, err)
	}
	untracked, err := sub.output(ctx, 1<<20, "ls-files", "--others", "--exclude-standard", "-z")
	if err != nil {
		return SubmoduleChanges{}, fmt.Errorf("submodule %s: %w", submodulePath, err)
	}
	listing := bytes.Join([][]byte{tracked, untracked}, []byte{0})
	var out SubmoduleChanges
	seen := map[string]struct{}{}
	add := func(into *[]string, rel string) {
		if _, ok := seen[rel]; ok {
			return
		}
		seen[rel] = struct{}{}
		*into = append(*into, rel)
	}
	merge := func(from SubmoduleChanges, prefix string) {
		for _, n := range from.Files {
			add(&out.Files, pathJoinSlash(prefix, n))
		}
		for _, n := range from.Deleted {
			add(&out.Deleted, pathJoinSlash(prefix, n))
		}
	}
	for _, raw := range bytes.Split(listing, []byte{0}) {
		// The name exactly as git wrote it: trimming or folding it would
		// make `t.txt ` and `t.txt`, or `a\b` and `a/b`, one file.
		name := string(raw)
		if name == "" {
			continue
		}
		if strings.HasSuffix(name, "/") {
			return SubmoduleChanges{}, NestedRepositoryError(pathJoinSlash(submodulePath, name))
		}
		if !filepath.IsLocal(filepath.FromSlash(name)) {
			return SubmoduleChanges{}, fmt.Errorf("submodule %s names %q, which is not a plain path", submodulePath, name)
		}
		info, missing, serr := lstatWithin(dir, name)
		if serr != nil {
			return SubmoduleChanges{}, fmt.Errorf("submodule %s: %s", submodulePath, serr.Error())
		}
		rel := pathJoinSlash(submodulePath, name)
		if missing {
			add(&out.Deleted, rel)
			continue
		}
		if info.Mode()&os.ModeSymlink != 0 {
			return SubmoduleChanges{}, fmt.Errorf("refusing to follow symlink %s", rel)
		}
		if info.IsDir() {
			nested, nerr := sub.submoduleWorktreeFiles(ctx, name, depth+1)
			if nerr != nil {
				return SubmoduleChanges{}, nerr
			}
			merge(nested, submodulePath)
			continue
		}
		if !info.Mode().IsRegular() {
			return SubmoduleChanges{}, fmt.Errorf("submodule %s: %s is not a regular file", submodulePath, name)
		}
		add(&out.Files, rel)
	}
	// Name-only is run with --ignore-submodules=dirty, so a nested dirty
	// submodule is not in the listing above. Ask the same guarded walk
	// DirtySubmodules uses, one level down, and do not reset its depth.
	nested, err := sub.dirtySubmodules(ctx, depth, nil)
	if err != nil {
		return SubmoduleChanges{}, fmt.Errorf("submodule %s: %w", submodulePath, err)
	}
	for _, name := range nested {
		inner, err := sub.submoduleWorktreeFiles(ctx, name, depth+1)
		if err != nil {
			return SubmoduleChanges{}, err
		}
		merge(inner, submodulePath)
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
		// Not checked out. An empty directory is what an uninitialized
		// submodule leaves. Files here are another matter: git treats the
		// directory as the gitlink, so no listing of the superproject names
		// them, and reading none of them would pass their bytes unseen.
		entries, rerr := os.ReadDir(dir)
		if rerr != nil {
			return "", false, rerr
		}
		if len(entries) > 0 {
			return "", false, fmt.Errorf("submodule %s holds files but no git repository; no listing names them, so they cannot be read", path)
		}
		return "", false, nil
	} else if err != nil {
		return "", false, err
	}
	return dir, true, nil
}

// output runs one guarded read and returns its stdout. A read whose output
// passes limit is refused, never truncated.
func (s *GitSession) output(ctx context.Context, limit int, args ...string) ([]byte, error) {
	out := &CappedBuffer{Limit: limit}
	if err := s.stream(ctx, out, args...); err != nil {
		return nil, err
	}
	if out.Overflowed() {
		return nil, fmt.Errorf("git %s in %s wrote more than %d bytes; refusing a truncated answer", strings.Join(args, " "), s.root, limit)
	}
	return out.Bytes(), nil
}

// stream runs one guarded read, writing its stdout to w as it arrives, under
// the per-call bound.
func (s *GitSession) stream(ctx context.Context, w io.Writer, args ...string) error {
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	cmd, err := s.Command(ctx, args...)
	if err != nil {
		return err
	}
	var stderr bytes.Buffer
	cmd.Stdout = w
	cmd.Stderr = &stderr
	cmd.WaitDelay = 2 * time.Second
	err, timedOut := RunBounded(ctx, cmd.Run)
	if timedOut {
		return fmt.Errorf("git %s in %s did not finish: %w", strings.Join(args, " "), s.root, ctx.Err())
	}
	if err != nil {
		return fmt.Errorf("git %s in %s: %w: %s", strings.Join(args, " "), s.root, err, strings.TrimSpace(stderr.String()))
	}
	return nil
}

// maxIndexEntry bounds one NUL-terminated record of the index listing. A path
// is far shorter; a record past this is not a listing worth believing.
const maxIndexEntry = 1 << 20

// gitlinks runs the index listing in args (`ls-files --stage -z`) and returns
// the paths of its mode-160000 entries. The listing is filtered as it streams,
// so memory is bounded by one record and by the gitlinks kept, not by the size
// of the index: a listing capped in total failed on a repository of some
// 800 000 files that had no submodule at all. A record longer than
// maxIndexEntry, or more gitlinks than maxSubmodules, is refused, never
// truncated.
func (s *GitSession) gitlinks(ctx context.Context, args []string) ([]string, error) {
	f := &gitlinkFilter{}
	if err := s.stream(ctx, f, args...); err != nil {
		return nil, err
	}
	f.flush()
	if f.overlong {
		return nil, fmt.Errorf("git %s in %s wrote a record longer than %d bytes; refusing a truncated answer", strings.Join(args, " "), s.root, maxIndexEntry)
	}
	if f.count > maxSubmodules {
		return nil, fmt.Errorf("%s has %d submodules, more than the %d one read examines; refusing to report a partial answer", s.root, f.count, maxSubmodules)
	}
	return f.links, nil
}

// gitlinkFilter is an io.Writer over `ls-files --stage -z` output that keeps
// only the paths of mode-160000 records. Like CappedBuffer it always reports a
// full write: a short one would close the pipe on a child that ran fine.
type gitlinkFilter struct {
	pending  []byte
	links    []string
	count    int
	overlong bool
}

func (f *gitlinkFilter) Write(p []byte) (int, error) {
	n := len(p)
	for len(p) > 0 {
		end := bytes.IndexByte(p, 0)
		if end < 0 {
			if len(f.pending)+len(p) > maxIndexEntry {
				f.overlong, f.pending = true, nil
			} else if !f.overlong {
				f.pending = append(f.pending, p...)
			}
			return n, nil
		}
		if f.overlong || len(f.pending)+end > maxIndexEntry {
			f.overlong, f.pending = true, nil
		} else {
			f.pending = append(f.pending, p[:end]...)
			f.flush()
		}
		p = p[end+1:]
	}
	return n, nil
}

// flush takes the record in pending, if any, and clears it.
func (f *gitlinkFilter) flush() {
	if len(f.pending) == 0 {
		return
	}
	meta, path, ok := strings.Cut(string(f.pending), "\t")
	if ok && strings.HasPrefix(meta, "160000 ") {
		f.count++
		if f.count <= maxSubmodules {
			f.links = append(f.links, path)
		}
	}
	f.pending = f.pending[:0]
}

// gitlinkMode is the tree mode of a submodule, and zeroMode the mode git writes
// for the side of a change where the path does not exist.
const (
	gitlinkMode = "160000"
	zeroMode    = "000000"
)

// The shapes a change to a gitlink takes. A raw record names a gitlink on one
// side or both, and these are the only combinations there are.
const (
	// GitlinkAdded is a gitlink where there was nothing.
	GitlinkAdded = "added"
	// GitlinkDeleted is a gitlink removed.
	GitlinkDeleted = "deleted"
	// GitlinkMoved is a gitlink on both sides, at another commit.
	GitlinkMoved = "moved"
	// GitlinkFileToSubmodule is a file replaced by a gitlink.
	GitlinkFileToSubmodule = "typechange: file to submodule"
	// GitlinkSubmoduleToFile is a gitlink replaced by a file.
	GitlinkSubmoduleToFile = "typechange: submodule to file"
)

// GitlinkChange is one gitlink the change touches: the superproject-relative
// path as git wrote it, and the shape of the change. It carries no commit id:
// what it names is that the commits between the two sides were not read, not
// which they were.
type GitlinkChange struct {
	Path  string
	Shape string
}

// MovedGitlinks lists every gitlink the working tree's change touches, from
// `git diff HEAD --raw`, which names a gitlink on either side of a record
// whether or not .gitmodules lists it. It is enumeration and nothing more: the
// submodule's own history is not opened, so nothing in the submodule's
// repository (its config, attributes, replace refs, grafts or objects) is
// consulted.
//
// The listing is asked with --ignore-submodules=dirty, which the guards put on
// every diff, and that overrides `ignore = all` in .gitmodules, in the
// superproject's config and in diff.ignoreSubmodules: a submodule configured to
// hide from `git diff` is still listed. A gitlink under a tool-state directory
// is listed too.
//
// headless is a repository with no HEAD yet: it has nothing to compare against,
// so every gitlink in its index is new.
//
// A listing that cannot be believed is an error, never a shorter list: a record
// that does not parse, one past maxIndexEntry, or more than maxSubmodules
// gitlinks.
func (s *GitSession) MovedGitlinks(ctx context.Context, headless bool) ([]GitlinkChange, error) {
	ctx, cancel := WithWalkBudget(ctx)
	defer cancel()
	if headless {
		paths, err := s.gitlinks(ctx, []string{"ls-files", "--stage", "-z"})
		if err != nil {
			return nil, err
		}
		moved := make([]GitlinkChange, 0, len(paths))
		for _, path := range paths {
			moved = append(moved, GitlinkChange{Path: path, Shape: GitlinkAdded})
		}
		return sortedGitlinks(moved), nil
	}
	f := &movedGitlinkFilter{}
	if err := s.stream(ctx, f, "diff", "HEAD", "--raw", "-z", "--no-abbrev", "--no-renames"); err != nil {
		return nil, err
	}
	if err := f.finish(s.root); err != nil {
		return nil, err
	}
	return sortedGitlinks(f.moved), nil
}

// sortedGitlinks orders a listing by path, so two runs over one change report
// the same reasons in the same order.
func sortedGitlinks(moved []GitlinkChange) []GitlinkChange {
	sort.Slice(moved, func(i, j int) bool {
		if moved[i].Path != moved[j].Path {
			return moved[i].Path < moved[j].Path
		}
		return moved[i].Shape < moved[j].Shape
	})
	return moved
}

// gitlinkShape names the change a raw record makes to a gitlink, given the
// modes of its two sides, at least one of which is gitlinkMode.
func gitlinkShape(oldMode, newMode string) string {
	switch {
	case oldMode == gitlinkMode && newMode == gitlinkMode:
		return GitlinkMoved
	case oldMode == zeroMode:
		return GitlinkAdded
	case newMode == zeroMode:
		return GitlinkDeleted
	case oldMode == gitlinkMode:
		return GitlinkSubmoduleToFile
	default:
		return GitlinkFileToSubmodule
	}
}

// movedGitlinkFilter is an io.Writer over `diff --raw -z` output that keeps the
// records in which either side is a gitlink. Like gitlinkFilter it streams, so
// memory is bounded by one record and the gitlinks kept, not by the size of the
// diff, and like CappedBuffer it always reports a full write.
//
// A record is a `:oldmode newmode oldid newid status` field and then one path
// (two for a rename or copy, the second of which is the path that changed), each
// NUL-terminated. A record that does not parse is remembered and refused by
// finish, never skipped.
type movedGitlinkFilter struct {
	pending   []byte
	header    []string
	paths     []string
	want      int
	moved     []GitlinkChange
	count     int
	overlong  bool
	malformed string
}

func (f *movedGitlinkFilter) Write(p []byte) (int, error) {
	n := len(p)
	for len(p) > 0 {
		end := bytes.IndexByte(p, 0)
		if end < 0 {
			f.hold(p)
			return n, nil
		}
		f.hold(p[:end])
		f.field()
		p = p[end+1:]
	}
	return n, nil
}

func (f *movedGitlinkFilter) hold(b []byte) {
	if f.overlong {
		return
	}
	if len(f.pending)+len(b) > maxIndexEntry {
		f.overlong, f.pending = true, nil
		return
	}
	f.pending = append(f.pending, b...)
}

// field takes the NUL-terminated field in pending and clears it.
func (f *movedGitlinkFilter) field() {
	text := string(f.pending)
	f.pending = f.pending[:0]
	if f.overlong || f.malformed != "" {
		return
	}
	if f.want == 0 {
		parts := strings.Split(text, " ")
		if len(parts) != 5 || !strings.HasPrefix(parts[0], ":") || parts[4] == "" {
			f.malformed = "a record that is not a raw diff header"
			return
		}
		f.header, f.paths, f.want = parts, f.paths[:0], 1
		if parts[4][0] == 'R' || parts[4][0] == 'C' {
			f.want = 2
		}
		return
	}
	f.paths = append(f.paths, text)
	f.want--
	if f.want > 0 {
		return
	}
	path := f.paths[len(f.paths)-1]
	if path == "" {
		f.malformed = "a change with no path"
		return
	}
	oldMode, newMode := strings.TrimPrefix(f.header[0], ":"), f.header[1]
	if oldMode != gitlinkMode && newMode != gitlinkMode {
		return
	}
	f.count++
	if f.count <= maxSubmodules {
		f.moved = append(f.moved, GitlinkChange{Path: path, Shape: gitlinkShape(oldMode, newMode)})
	}
}

// finish reports a listing that cannot be believed.
func (f *movedGitlinkFilter) finish(root string) error {
	switch {
	case f.overlong:
		return fmt.Errorf("git diff HEAD --raw in %s wrote a record longer than %d bytes; refusing a truncated answer", root, maxIndexEntry)
	case f.malformed != "":
		return fmt.Errorf("git diff HEAD --raw in %s wrote %s; refusing to read it as a shorter list", root, f.malformed)
	case f.want != 0 || len(f.pending) != 0:
		return fmt.Errorf("git diff HEAD --raw in %s ended inside a record; refusing to read it as a shorter list", root)
	case f.count > maxSubmodules:
		return fmt.Errorf("%s moved %d submodules, more than the %d one read examines; refusing to report a partial answer", root, f.count, maxSubmodules)
	}
	return nil
}
