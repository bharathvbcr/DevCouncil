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
const SubjectPrefix = "git-tree:"

// subjectBound bounds the whole computation. Staging a working tree hashes
// every file, so it is given longer than one git read.
const subjectBound = 5 * time.Minute

// maxIgnoreSource bounds the exclude files read into the subject.
const maxIgnoreSource = 1 << 20

// WorkingTreeSubject names the exact content of root's working tree, for a
// review to be bound to: "git-tree:" and the object id of the tree git would
// commit if every change — tracked, untracked and deleted, minus what the
// ignore rules exclude and minus the tool state verification never judges
// (policy.ToolStateDirs) — were staged. When the repository's own exclude
// files (.git/info/exclude, a local core.excludesFile) carry rules, a digest of those
// rules is appended ("git-tree:<id>-x<digest>"), because they decide which
// files the tree leaves out.
//
// A tree id rather than a hash of the diff, because the diff moves when the
// content does not: the same content reviewed before a commit and verified
// after it has a different diff against a different HEAD, and the same tree.
// A clean tree with no exclude rules has its HEAD's tree as its subject.
//
// The subject is an answer only when it can see everything verify judges, so
// it is refused — an error, never a subject that might match — when part of
// the tree is beyond it:
//   - a gitlink (submodule or nested repository) that HEAD does not have, or
//     any submodule with changes of its own: its files are not in the tree,
//     so an edit to them would not move the subject. A planted `.git` file is
//     the same case;
//   - an ignored `.gitignore` in a directory that is not itself ignored: a
//     self-ignoring rules file could hide new source from the tree while the
//     build still compiles it;
//   - a sparse checkout, whose missing files would read as deletions.
//
// It runs against a scratch index built from HEAD's tree — never a copy of
// the repository's own, whose assume-unchanged, skip-worktree and stat-cache
// state would let a modified file keep its old blob — so the person's staged
// state is untouched and plays no part. Staging writes blob objects for
// changed files into the object store, unreferenced and pruned by git's
// ordinary gc; nothing else is written. Every git call goes through
// proc.GitSession, so the repository's filter drivers and fsmonitor are
// neutralised as for every other read here.
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
	// Staged with core.ignorecase off, so a case-only rename on a
	// case-insensitive filesystem (z_test.go -> z_Test.go, which moves a file
	// from test-only into the package) is a different tree, not the same one.
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
	if err := checkIgnoredRulesFiles(ctx, git); err != nil {
		return "", err
	}
	treeOut, err := runGitEnv(ctx, git, 1<<10, subjectBound, env, "write-tree")
	if err != nil {
		return "", fmt.Errorf("writing the working tree's tree: %w", err)
	}
	tree := strings.TrimSpace(string(treeOut))
	if !isObjectID(tree) {
		return "", fmt.Errorf("git write-tree answered %q, which is not an object id", tree)
	}
	rules, err := excludeRulesDigest(ctx, git, root)
	if err != nil {
		return "", err
	}
	if rules != "" {
		return SubjectPrefix + tree + "-x" + rules, nil
	}
	return SubjectPrefix + tree, nil
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
	sub, err := proc.NewGitSession(dir)
	if err != nil {
		return false, err
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
// the tree, and nothing in the tree records that it did.
func checkIgnoredRulesFiles(ctx context.Context, git *proc.GitSession) error {
	ignored, err := runGit(ctx, git, gitListingLimit, "ls-files", "--others", "--ignored", "--exclude-standard",
		"-z", "--", ":(glob,icase)**/.gitignore")
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

// isObjectID reports whether s is a SHA-1 or SHA-256 object id in lowercase
// hex.
func isObjectID(s string) bool {
	if len(s) != 40 && len(s) != 64 {
		return false
	}
	for _, c := range s {
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}
