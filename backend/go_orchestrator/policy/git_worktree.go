package policy

import (
	"io"
	"os"
	"path/filepath"
	"strings"
)

const maxGitMetadataBytes = 4096

type gitWorktreeIdentity struct {
	checkout  string
	gitDir    string
	commonDir string
}

// SameRegisteredWorktree reports whether candidate resolves to the root of a
// registered Git worktree sharing the same common repository directory as
// baseRoot, returning the canonical worktree root.
//
// It fails closed on empty roots, shell expansions, malformed paths,
// subdirectories inside a worktree, symlinked .git entries, and unverified
// external checkouts whose metadata merely claims baseRoot's common directory.
func SameRegisteredWorktree(baseRoot, candidate string) (string, bool) {
	if baseRoot == "" || candidate == "" || hasExpansion(candidate) {
		return "", false
	}
	if _, bad := malformedPath(candidate); bad {
		return "", false
	}
	baseID, ok := resolveWorktreeIdentity(baseRoot)
	if !ok {
		return "", false
	}
	slashed := filepath.FromSlash(normalizeSlashes(candidate))
	candPath := slashed
	if !filepath.IsAbs(candPath) {
		candPath = filepath.Join(baseID.checkout, candPath)
	}
	candID, ok := resolveWorktreeIdentity(candPath)
	if !ok || candID.commonDir != baseID.commonDir {
		return "", false
	}
	return candID.checkout, true
}

func resolveWorktreeIdentity(dir string) (gitWorktreeIdentity, bool) {
	checkout, err := filepath.EvalSymlinks(dir)
	if err != nil {
		return gitWorktreeIdentity{}, false
	}
	info, err := os.Stat(checkout)
	if err != nil || !info.IsDir() {
		return gitWorktreeIdentity{}, false
	}

	dotgit := filepath.Join(checkout, ".git")
	dotInfo, err := os.Lstat(dotgit)
	if err != nil {
		return gitWorktreeIdentity{}, false
	}

	var gitDir string
	switch {
	case dotInfo.IsDir():
		resolved, err := filepath.EvalSymlinks(dotgit)
		if err != nil {
			return gitWorktreeIdentity{}, false
		}
		gitDir = resolved
	case dotInfo.Mode().IsRegular():
		raw, ok := readBoundedRegularFile(dotgit, maxGitMetadataBytes)
		if !ok {
			return gitWorktreeIdentity{}, false
		}
		targetRaw, hasPrefix := strings.CutPrefix(raw, "gitdir: ")
		if !hasPrefix {
			return gitWorktreeIdentity{}, false
		}
		targetPath, ok := resolveMetadataTarget(checkout, targetRaw)
		if !ok {
			return gitWorktreeIdentity{}, false
		}
		resolved, err := filepath.EvalSymlinks(targetPath)
		if err != nil {
			return gitWorktreeIdentity{}, false
		}
		gitDir = resolved
	default:
		// Reject symlinked, device, FIFO, or socket .git entries.
		return gitWorktreeIdentity{}, false
	}

	commonPtr := filepath.Join(gitDir, "commondir")
	commonDir := gitDir
	if _, lerr := os.Lstat(commonPtr); lerr == nil {
		raw, ok := readBoundedRegularFile(commonPtr, maxGitMetadataBytes)
		if !ok {
			return gitWorktreeIdentity{}, false
		}
		targetPath, ok := resolveMetadataTarget(gitDir, raw)
		if !ok {
			return gitWorktreeIdentity{}, false
		}
		resolved, err := filepath.EvalSymlinks(targetPath)
		if err != nil {
			return gitWorktreeIdentity{}, false
		}
		commonDir = resolved
	} else if !os.IsNotExist(lerr) {
		return gitWorktreeIdentity{}, false
	}

	headInfo, err := os.Lstat(filepath.Join(gitDir, "HEAD"))
	if err != nil || !headInfo.Mode().IsRegular() {
		return gitWorktreeIdentity{}, false
	}
	objInfo, err := os.Stat(filepath.Join(commonDir, "objects"))
	if err != nil || !objInfo.IsDir() {
		return gitWorktreeIdentity{}, false
	}

	id := gitWorktreeIdentity{
		checkout:  checkout,
		gitDir:    gitDir,
		commonDir: commonDir,
	}
	if !isRegisteredMember(id, dotInfo) {
		return gitWorktreeIdentity{}, false
	}
	return id, true
}

// isRegisteredMember enforces bidirectional verification between the common Git
// directory and linked worktrees so a forged checkout cannot claim membership
// via a one-way commondir pointer.
func isRegisteredMember(id gitWorktreeIdentity, dotInfo os.FileInfo) bool {
	if id.gitDir == id.commonDir {
		return dotInfo.IsDir()
	}
	registry, err := filepath.EvalSymlinks(filepath.Join(id.commonDir, "worktrees"))
	if err != nil {
		return false
	}
	if filepath.Dir(id.gitDir) != registry {
		return false
	}
	raw, ok := readBoundedRegularFile(filepath.Join(id.gitDir, "gitdir"), maxGitMetadataBytes)
	if !ok {
		return false
	}
	named, ok := resolveMetadataTarget(id.gitDir, raw)
	if !ok || filepath.Base(named) != ".git" {
		return false
	}
	holder, err := filepath.EvalSymlinks(filepath.Dir(named))
	if err != nil {
		return false
	}
	return holder == id.checkout
}

func resolveMetadataTarget(base, raw string) (string, bool) {
	trimmed := strings.TrimRight(raw, "\r\n")
	if trimmed == "" {
		return "", false
	}
	for _, r := range trimmed {
		if r < 0x20 || r == 0x7f {
			return "", false
		}
	}
	p := filepath.FromSlash(normalizeSlashes(trimmed))
	if filepath.IsAbs(p) {
		return filepath.Clean(p), true
	}
	return filepath.Clean(filepath.Join(base, p)), true
}

func readBoundedRegularFile(p string, limit int64) (string, bool) {
	lstat, err := os.Lstat(p)
	if err != nil || !lstat.Mode().IsRegular() || lstat.Size() > limit {
		return "", false
	}
	f, err := os.Open(p)
	if err != nil {
		return "", false
	}
	defer f.Close()
	fstat, err := f.Stat()
	if err != nil || !fstat.Mode().IsRegular() || fstat.Size() > limit {
		return "", false
	}
	data, err := io.ReadAll(io.LimitReader(f, limit+1))
	if err != nil || int64(len(data)) > limit {
		return "", false
	}
	return string(data), true
}

// stepGitGlobalOption parses one global option at words[i] (after `git`) and
// returns its canonical option name, value (if it takes one), whether it took a
// value, and the index of the next word.
func stepGitGlobalOption(words []shellWord, i int) (name, value string, hasValue bool, next int) {
	opt := words[i].text
	if strings.HasPrefix(opt, "--") {
		n, v, eq := strings.Cut(opt, "=")
		if eq {
			return n, v, true, i + 1
		}
		if gitGlobalOptionsWithValue[n] && i+1 < len(words) {
			return n, words[i+1].text, true, i + 2
		}
		return n, "", false, i + 1
	}
	if (strings.HasPrefix(opt, "-C") || strings.HasPrefix(opt, "-c")) && len(opt) > 2 {
		return opt[:2], opt[2:], true, i + 1
	}
	if gitGlobalOptionsWithValue[opt] && i+1 < len(words) {
		return opt, words[i+1].text, true, i + 2
	}
	return opt, "", false, i + 1
}

// resolveGitClauseWorktree inspects global options on the git invocation at
// words[gitAt]. If no directory option escapes callerRoot's registered
// worktree family, it returns the effective working tree root (or callerRoot if
// no -C option was supplied) and escaped=false. Otherwise it returns the
// offending option name and escaped=true.
func resolveGitClauseWorktree(words []shellWord, gitAt int, callerRoot string) (effectiveRoot, escapedOpt string, escaped bool) {
	curRoot := callerRoot
	for i := gitAt + 1; i < len(words); {
		w := words[i]
		if !w.quotedHead {
			if targetNext, isRedir := redirectionPrefix(w.text); isRedir {
				i++
				if targetNext && i < len(words) {
					i++
				}
				continue
			}
		}
		if w.text == "--" || w.text == "-" || !strings.HasPrefix(w.text, "-") {
			break
		}
		name, val, hasVal, next := stepGitGlobalOption(words, i)
		i = next
		switch name {
		case "--git-dir", "--work-tree":
			return "", name, true
		case "-C":
			if !hasVal || curRoot == "" {
				return "", "-C", true
			}
			nextRoot, ok := SameRegisteredWorktree(curRoot, val)
			if !ok {
				return "", "-C", true
			}
			curRoot = nextRoot
		}
	}
	return curRoot, "", false
}

func sameWorktreePath(a, b string) bool {
	if a == "" || b == "" {
		return a == b
	}
	ra, errA := filepath.EvalSymlinks(a)
	rb, errB := filepath.EvalSymlinks(b)
	if errA == nil && errB == nil {
		return ra == rb
	}
	return filepath.Clean(a) == filepath.Clean(b)
}
