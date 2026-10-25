package verify

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// SubjectPrefix marks a subject hash computed by WorkingTreeSubject.
const SubjectPrefix = "tree-sha256:"

// subjectBound bounds the whole computation. Naming a working tree reads
// every file, so it is given longer than one git read.
const subjectBound = 5 * time.Minute

// maxIgnoreSource bounds the exclude files read into the subject.
const maxIgnoreSource = 1 << 20

// WorkingTreeSubject names the exact content of root's working tree, for a
// review to be bound to: "tree-sha256:" and a SHA-256 over every file verify
// would judge — its path, its kind, its executable bit and the SHA-256 of its
// bytes as they are on disk (a symlink's target text, a submodule's commit).
// When the exclude files outside the tree carry rules, a digest of them is
// appended ("tree-sha256:<digest>-x<rules>"), because they decide which files
// are in the set.
//
// Which files: the ones git would commit if every change were staged —
// tracked, untracked and deleted, minus what the ignore rules exclude and
// minus the tool state verification never judges (policy.ToolStateDirs).
// That set is git's, read through a scratch index built from HEAD's tree
// (never a copy of the person's own, whose assume-unchanged, skip-worktree
// and staged state are not the content), with core.ignorecase off so a
// case-only rename is a different set.
//
// What each file holds is not git's. Hashing what `git add` would store
// inherits every transformation it applies on the way in — line-ending
// conversion (text, eol, autocrlf), ident expansion, working-tree-encoding,
// core.fileMode, core.symlinks — each of which can make different bytes on
// disk store as the same object. So the bytes are read from disk and hashed
// here, and none of those settings decides the subject. The same content has
// the same subject whether it is committed or not, and in any checkout.
//
// The subject is an answer only when it can see everything verify judges, so
// it is refused — an error, never a subject that might match — when part of
// the content is beyond it:
//   - a gitlink (submodule or nested repository) that HEAD does not have; a
//     submodule with changes of its own; a gitlink directory populated
//     without its own repository, or whose repository's working tree is
//     elsewhere. A planted `.git` file is the first case;
//   - a tracked symlink whose target lies outside the repository, inside
//     `.git`, in tool state, or on an ignored path: the subject holds only
//     the link text;
//   - an ignored `.gitignore` in a directory that is not itself ignored: a
//     self-ignoring rules file could hide new source while the build still
//     compiles it;
//   - a sparse checkout, whose missing files would read as deletions.
//
// What the ignore rules leave out (generated files, build output) and tool
// state are not part of the subject, as they are not part of what verify's
// own gates read. Staging writes blob objects for changed files into the
// object store, unreferenced and pruned by git's ordinary gc; nothing else is
// written. Every git call goes through proc.GitSession, so the repository's
// filter drivers and fsmonitor are neutralised as for every other read here.
func WorkingTreeSubject(ctx context.Context, root string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, subjectBound)
	defer cancel()
	git, err := proc.NewGitSession(root)
	if err != nil {
		return "", err
	}
	if sparse, err := runGit(ctx, git, 1<<10, "config", "--bool", "core.sparseCheckout"); err == nil &&
		strings.TrimSpace(string(sparse)) == "true" {
		return "", errors.New("the repository is a sparse checkout: files outside it would read as deleted")
	}
	headless := hasNoHead(ctx, git)
	scratchDir, err := os.MkdirTemp("", "devcouncil-subject-")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(scratchDir)
	env := []string{"GIT_INDEX_FILE=" + filepath.Join(scratchDir, "index")}
	if !headless {
		if _, err := runGitEnv(ctx, git, 1<<20, subjectBound, env, "read-tree", "HEAD"); err != nil {
			return "", fmt.Errorf("reading HEAD into a scratch index: %w", err)
		}
	}
	addEnv := append([]string{"GIT_CONFIG_COUNT=1", "GIT_CONFIG_KEY_0=core.ignorecase", "GIT_CONFIG_VALUE_0=false"}, env...)
	if _, err := runGitEnv(ctx, git, 1<<20, subjectBound, addEnv, append([]string{"add", "--all"}, toolStateExcludes()...)...); err != nil {
		return "", fmt.Errorf("staging the working tree into a scratch index: %w", err)
	}
	if err := checkGitlinks(ctx, git, env, headless); err != nil {
		return "", err
	}
	if err := checkSymlinks(ctx, git, env, root); err != nil {
		return "", err
	}
	if err := checkIgnoredRulesFiles(ctx, git, env); err != nil {
		return "", err
	}
	content, err := contentDigest(ctx, git, env, root)
	if err != nil {
		return "", err
	}
	rules, err := excludeRulesDigest(ctx, git, root)
	if err != nil {
		return "", err
	}
	if rules != "" {
		return SubjectPrefix + content + "-x" + rules, nil
	}
	return SubjectPrefix + content, nil
}

// contentDigest hashes every entry of the scratch index from disk: path,
// kind, executable bit and the SHA-256 of the bytes for a file; the target
// text for a symlink; the commit for a gitlink. Entries come in index order,
// which is byte order of path, so the digest is independent of how the set
// was produced.
func contentDigest(ctx context.Context, git *proc.GitSession, env []string, root string) (string, error) {
	staged, err := runGitEnv(ctx, git, gitListingLimit, subjectBound, env, "ls-files", "--stage", "-z")
	if err != nil {
		return "", fmt.Errorf("listing the scratch index: %w", err)
	}
	h := sha256.New()
	for _, entry := range strings.Split(string(staged), "\x00") {
		if entry == "" {
			continue
		}
		if err := ctx.Err(); err != nil {
			return "", err
		}
		meta, p, ok := strings.Cut(entry, "\t")
		if !ok {
			return "", fmt.Errorf("unreadable index entry %q", entry)
		}
		fields := strings.Fields(meta) // mode, object, stage
		if len(fields) != 3 {
			return "", fmt.Errorf("unreadable index entry %q", entry)
		}
		// Tool state tracked in HEAD is still in the scratch index (staging
		// excludes it only from what is added), and verification never
		// judges it.
		if ToolOwned(p) {
			continue
		}
		full := filepath.Join(root, filepath.FromSlash(p))
		switch fields[0] {
		case "160000":
			fmt.Fprintf(h, "L\x00%s\x00%s\n", p, fields[1])
		default:
			info, err := os.Lstat(full)
			if err != nil {
				return "", fmt.Errorf("%s changed while it was being named: %w", p, err)
			}
			switch {
			case info.Mode()&os.ModeSymlink != 0:
				target, err := os.Readlink(full)
				if err != nil {
					return "", fmt.Errorf("%s: %w", p, err)
				}
				fmt.Fprintf(h, "S\x00%s\x00%s\n", p, target)
			case info.Mode().IsRegular():
				sum, err := fileSHA256(full)
				if err != nil {
					return "", fmt.Errorf("%s: %w", p, err)
				}
				exec := "-"
				if info.Mode().Perm()&0o111 != 0 {
					exec = "x"
				}
				fmt.Fprintf(h, "F\x00%s\x00%s\x00%s\n", p, exec, sum)
			default:
				return "", fmt.Errorf("%s is neither a file nor a symlink on disk (%s)", p, info.Mode().Type())
			}
		}
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

func fileSHA256(p string) (string, error) {
	f, err := os.Open(p)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

// checkGitlinks refuses a tree with a gitlink HEAD does not have, or with a
// submodule that has changes of its own.
func checkGitlinks(ctx context.Context, git *proc.GitSession, env []string, headless bool) error {
	staged, err := runGitEnv(ctx, git, gitListingLimit, subjectBound, env, "ls-files", "--stage", "-z")
	if err != nil {
		return fmt.Errorf("listing the scratch index: %w", err)
	}
	links := gitlinkPaths(staged, func(entry string) (string, string, bool) {
		// "<mode> <object> <stage>\t<path>"
		meta, p, ok := strings.Cut(entry, "\t")
		mode, _, _ := strings.Cut(meta, " ")
		return mode, p, ok
	})
	if len(links) == 0 {
		return nil
	}
	known := map[string]bool{}
	if !headless {
		head, err := runGit(ctx, git, gitListingLimit, "ls-tree", "-r", "-z", "HEAD")
		if err != nil {
			return fmt.Errorf("listing HEAD's gitlinks: %w", err)
		}
		for _, p := range gitlinkPaths(head, func(entry string) (string, string, bool) {
			// "<mode> <type> <object>\t<path>"
			meta, p, ok := strings.Cut(entry, "\t")
			mode, _, _ := strings.Cut(meta, " ")
			return mode, p, ok
		}) {
			known[p] = true
		}
	}
	var unknown []string
	for _, p := range links {
		if !known[p] {
			unknown = append(unknown, p)
		}
	}
	if len(unknown) > 0 {
		return fmt.Errorf("%s is a nested repository or gitlink HEAD does not have, so its files are not in "+
			"the tree and a change to them could not be seen", strings.Join(unknown, ", "))
	}
	// Each gitlink is asked directly, whether or not .gitmodules names it, and
	// for untracked files too: the tree records a gitlink only as the commit
	// it is checked out at, so anything else in it is invisible to the
	// subject. (proc.DirtySubmodules is not enough: it trusts .gitmodules and
	// reports only tracked changes.)
	var dirty []string
	for _, p := range links {
		changed, err := gitlinkHasChanges(ctx, filepath.Join(git.Root(), filepath.FromSlash(p)))
		if err != nil {
			return fmt.Errorf("reading the state of %s: %w", p, err)
		}
		if changed {
			dirty = append(dirty, p)
		}
	}
	if len(dirty) > 0 {
		return fmt.Errorf("submodule %s has changes of its own (tracked or untracked), which the tree "+
			"records only as a commit and so cannot name", strings.Join(dirty, ", "))
	}
	return nil
}

// gitlinkHasChanges reports whether the repository checked out at dir has any
// change of its own, untracked files included. A gitlink with nothing checked
// out (an uninitialised submodule: an empty directory, or none) has no
// content to hide.
func gitlinkHasChanges(ctx context.Context, dir string) (bool, error) {
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) || (err == nil && len(entries) == 0) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	// A populated gitlink directory must be its own repository's working
	// tree. Without a `.git`, git would answer for the enclosing repository;
	// with one whose working tree is elsewhere (core.worktree, a decoy
	// gitfile), the status would describe other files.
	if _, err := os.Lstat(filepath.Join(dir, ".git")); err != nil {
		return false, fmt.Errorf("the directory holds files but no repository of its own, so its content " +
			"is neither in the tree nor in a submodule")
	}
	sub, err := proc.NewGitSession(dir)
	if err != nil {
		return false, err
	}
	top, err := runGit(ctx, sub, 1<<12, "rev-parse", "--show-toplevel")
	if err != nil {
		return false, err
	}
	realDir, err := filepath.EvalSymlinks(dir)
	if err != nil {
		return false, err
	}
	realTop, err := filepath.EvalSymlinks(strings.TrimSpace(string(top)))
	if err != nil || realTop != realDir {
		return false, fmt.Errorf("its repository's working tree is %s, not this directory", strings.TrimSpace(string(top)))
	}
	out, err := runGit(ctx, sub, gitListingLimit, "status", "--porcelain=v1", "-z",
		"--untracked-files=all", "--ignore-submodules=none")
	if err != nil {
		return false, err
	}
	return len(out) > 0, nil
}

// checkSymlinks refuses a tree holding a symlink whose target's content the
// tree does not hold: one that resolves outside the repository, into tool
// state, or onto an ignored path. The tree records a symlink only as its
// target text, so a change to that content would not move the subject.
func checkSymlinks(ctx context.Context, git *proc.GitSession, env []string, root string) error {
	staged, err := runGitEnv(ctx, git, gitListingLimit, subjectBound, env, "ls-files", "--stage", "-z")
	if err != nil {
		return fmt.Errorf("listing the scratch index: %w", err)
	}
	realRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		return err
	}
	var inside []string
	for _, entry := range strings.Split(string(staged), "\x00") {
		meta, p, ok := strings.Cut(entry, "\t")
		if !ok || !strings.HasPrefix(meta, "120000 ") {
			continue
		}
		link := filepath.Join(root, filepath.FromSlash(p))
		target, err := filepath.EvalSymlinks(link)
		if errors.Is(err, os.ErrNotExist) {
			continue // dangling: no content to hide
		}
		if err != nil {
			return fmt.Errorf("%s: %w", p, err)
		}
		rel, err := filepath.Rel(realRoot, target)
		if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			return fmt.Errorf("%s is a symlink to %s, outside the repository, whose content the tree cannot name", p, target)
		}
		rel = filepath.ToSlash(rel)
		if rel == ".git" || strings.HasPrefix(rel, ".git/") || strings.Contains(rel, "/.git/") ||
			strings.HasSuffix(rel, "/.git") {
			return fmt.Errorf("%s is a symlink into a repository's .git directory (%s), which the tree leaves out", p, rel)
		}
		if ToolOwned(rel) {
			return fmt.Errorf("%s is a symlink into tool state (%s), which the tree leaves out", p, rel)
		}
		inside = append(inside, rel)
	}
	if len(inside) == 0 {
		return nil
	}
	res, err := git.Run(ctx, gitListingLimit, append([]string{"check-ignore", "--no-index", "--"}, inside...)...)
	if err != nil && res.ExitCode != 1 {
		return fmt.Errorf("checking symlink targets: %w", err)
	}
	if ignored := strings.Fields(string(res.Stdout)); len(ignored) > 0 {
		return fmt.Errorf("a tracked symlink points at %s, which is ignored, so its content is not in the tree",
			strings.Join(ignored, ", "))
	}
	return nil
}

func gitlinkPaths(listing []byte, split func(string) (mode, p string, ok bool)) []string {
	var out []string
	for _, entry := range strings.Split(string(listing), "\x00") {
		if mode, p, ok := split(entry); ok && mode == "160000" {
			out = append(out, p)
		}
	}
	return out
}

// checkIgnoredRulesFiles refuses a tree in which a `.gitignore` is itself
// ignored while its directory is not: such a file can hide new source from
// the tree, and nothing in the tree records that it did. Read through the
// scratch index, so one staged in the person's own index is still "other".
func checkIgnoredRulesFiles(ctx context.Context, git *proc.GitSession, env []string) error {
	ignored, err := runGitEnv(ctx, git, gitListingLimit, subjectBound, env, "ls-files", "--others", "--ignored",
		"--exclude-standard", "-z", "--", ":(glob,icase)**/.gitignore")
	if err != nil {
		return fmt.Errorf("listing ignored rules files: %w", err)
	}
	var dirs []string
	for _, p := range strings.Split(string(ignored), "\x00") {
		if p == "" {
			continue
		}
		dir := path.Dir(p)
		if dir == "." {
			// The root's own .gitignore, ignored: nothing above it to cover it.
			return fmt.Errorf("%s ignores itself, so the rules it adds are not in the tree", p)
		}
		dirs = append(dirs, dir+"/")
	}
	if len(dirs) == 0 {
		return nil
	}
	// check-ignore exits 1 when none of the paths is ignored, which is an
	// answer, not a failure.
	res, err := git.Run(ctx, gitListingLimit, append([]string{"check-ignore", "--no-index", "--"}, dirs...)...)
	if err != nil && res.ExitCode != 1 {
		return fmt.Errorf("checking ignored rules files: %w", err)
	}
	if res.Truncated {
		return fmt.Errorf("checking ignored rules files: %w", ErrGitOutputTruncated)
	}
	covered := map[string]bool{}
	for _, line := range strings.Split(string(res.Stdout), "\n") {
		covered[strings.TrimSpace(line)] = true
	}
	var exposed []string
	for _, dir := range dirs {
		if !covered[dir] {
			exposed = append(exposed, dir+".gitignore")
		}
	}
	if len(exposed) > 0 {
		sort.Strings(exposed)
		return fmt.Errorf("%s is ignored while its directory is not, so the rules it adds could hide files "+
			"the tree does not show", strings.Join(exposed, ", "))
	}
	return nil
}

// excludeRulesDigest is a digest of the effective rules in every exclude file
// outside the tree — .git/info/exclude, and the excludes file git actually
// uses: core.excludesFile from any scope (includes followed), or the XDG
// default when it is unset — or "" when they hold none. Comments and blank
// lines are not rules.
//
// The user-level file is included although it lies outside the repository:
// an agent whose allowlist admits `git *` can point core.excludesFile
// anywhere with `git config --global`, and rules outside the tree decide what
// the tree leaves out. The cost is that the same tree can have a different
// subject on two machines with different excludes; a review is recorded and
// verified on the machine that runs both.
func excludeRulesDigest(ctx context.Context, git *proc.GitSession, root string) (string, error) {
	var sources []string
	if out, err := runGit(ctx, git, 1<<12, "rev-parse", "--git-path", "info/exclude"); err == nil {
		p := strings.TrimSpace(string(out))
		if !filepath.IsAbs(p) {
			p = filepath.Join(root, p)
		}
		sources = append(sources, p)
	} else {
		return "", fmt.Errorf("locating info/exclude: %w", err)
	}
	// Exit 1 is "unset", an answer; anything else is a failure to read.
	res, err := git.Run(ctx, 1<<12, "config", "--includes", "--path", "core.excludesFile")
	switch {
	case err == nil:
		if p := strings.TrimSpace(string(res.Stdout)); p != "" {
			sources = append(sources, p)
		}
	case res.ExitCode == 1:
		if p := defaultExcludesFile(); p != "" {
			sources = append(sources, p)
		}
	default:
		return "", fmt.Errorf("reading core.excludesFile: %w", err)
	}
	h := sha256.New()
	rules := 0
	for _, src := range sources {
		body, err := readSmall(src, maxIgnoreSource)
		if errors.Is(err, os.ErrNotExist) {
			continue
		}
		if err != nil {
			return "", fmt.Errorf("reading exclude rules %s: %w", src, err)
		}
		fmt.Fprintf(h, "%s\x00", src)
		for _, line := range bytes.Split(body, []byte("\n")) {
			line = bytes.TrimRight(line, "\r")
			if len(bytes.TrimSpace(line)) == 0 || line[0] == '#' {
				continue
			}
			rules++
			h.Write(line)
			h.Write([]byte{0})
		}
	}
	if rules == 0 {
		return "", nil
	}
	return hex.EncodeToString(h.Sum(nil))[:16], nil
}

// defaultExcludesFile is the excludes file git reads when core.excludesFile is
// unset: $XDG_CONFIG_HOME/git/ignore, or ~/.config/git/ignore.
func defaultExcludesFile() string {
	if xdg := os.Getenv("XDG_CONFIG_HOME"); xdg != "" {
		return filepath.Join(xdg, "git", "ignore")
	}
	if home, err := os.UserHomeDir(); err == nil {
		return filepath.Join(home, ".config", "git", "ignore")
	}
	return ""
}

func readSmall(p string, limit int64) ([]byte, error) {
	f, err := os.Open(p)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	body, err := io.ReadAll(io.LimitReader(f, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(body)) > limit {
		return nil, fmt.Errorf("over %d bytes", limit)
	}
	return body, nil
}
