package proc

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"time"
)

// GitVersion is the version a git binary reports, reduced to the three
// numbers that decide what it honours.
type GitVersion struct {
	Major, Minor, Patch int
	// Raw is the whole `git version` line, for messages and doctor output.
	Raw string
}

func (v GitVersion) String() string {
	return fmt.Sprintf("%d.%d.%d", v.Major, v.Minor, v.Patch)
}

func (v GitVersion) atLeast(major, minor, patch int) bool {
	if v.Major != major {
		return v.Major > major
	}
	if v.Minor != minor {
		return v.Minor > minor
	}
	return v.Patch >= patch
}

// noLazyFetchBackports are the maintenance releases that carry the
// CVE-2024-32465 fix, which taught promisor-remote.c to honour
// GIT_NO_LAZY_FETCH before the variable reached a feature release.
var noLazyFetchBackports = map[int]int{39: 4, 40: 2, 41: 1, 42: 2, 43: 4, 44: 1}

// HonorsNoLazyFetch reports whether this git refuses to fetch a missing
// object from a promisor remote when GIT_NO_LAZY_FETCH=1.
//
// Checked against git's source rather than recalled (ft-ff2153d9): the
// variable is read in environment.c from v2.45.0, and in promisor-remote.c
// in the CVE-2024-32465 maintenance releases v2.39.4, v2.40.2, v2.41.1,
// v2.42.2, v2.43.4 and v2.44.1. v2.44.0 does not read it, so the owner's
// "below 2.44" threshold is applied as "a git that does not honour the
// variable", which is what the threshold stood for.
func (v GitVersion) HonorsNoLazyFetch() bool {
	if v.atLeast(2, 45, 0) {
		return true
	}
	if v.Major != 2 {
		return false
	}
	patch, ok := noLazyFetchBackports[v.Minor]
	return ok && v.Patch >= patch
}

// ParseGitVersion reads `git version 2.54.0 (Apple Git-157)`,
// `git version 2.39.5.windows.1` and the like. Only the leading numbers
// count; a missing patch level is zero.
func ParseGitVersion(line string) (GitVersion, error) {
	line = strings.TrimSpace(line)
	rest, ok := strings.CutPrefix(line, "git version ")
	if !ok {
		return GitVersion{}, fmt.Errorf("unrecognised git version output %q", line)
	}
	if i := strings.IndexAny(rest, " \t"); i >= 0 {
		rest = rest[:i]
	}
	parts := strings.SplitN(rest, ".", 4)
	numbers := [3]int{}
	for i := 0; i < len(numbers) && i < len(parts); i++ {
		n, err := strconv.Atoi(parts[i])
		if err != nil {
			if i < 2 {
				return GitVersion{}, fmt.Errorf("unrecognised git version output %q", line)
			}
			break
		}
		numbers[i] = n
	}
	return GitVersion{Major: numbers[0], Minor: numbers[1], Patch: numbers[2], Raw: line}, nil
}

// gitBinaryKey identifies a git binary by path and by what is at the path,
// so a git upgraded in place is asked again.
type gitBinaryKey struct {
	path  string
	size  int64
	mtime int64
}

var gitVersions sync.Map // gitBinaryKey -> GitVersion

// DetectGitVersion asks git (an absolute path, as LookPathOutside returns
// it) for its version, once per binary per process. `git version` reads no
// repository, so it runs nothing a repository names; it gets the same
// environment as every other git DevCouncil runs all the same.
func DetectGitVersion(ctx context.Context, git string) (GitVersion, error) {
	info, err := os.Stat(git)
	if err != nil {
		return GitVersion{}, err
	}
	key := gitBinaryKey{path: git, size: info.Size(), mtime: info.ModTime().UnixNano()}
	if v, ok := gitVersions.Load(key); ok {
		return v.(GitVersion), nil
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, git, "version")
	ConfigureGroup(cmd)
	cmd.Env = gitEnv()
	cmd.Dir = os.TempDir()
	out := &CappedBuffer{Limit: 4096}
	var stderr bytes.Buffer
	cmd.Stdout = out
	cmd.Stderr = &stderr
	cmd.WaitDelay = 2 * time.Second
	err, timedOut := RunBounded(ctx, cmd.Run)
	if timedOut {
		return GitVersion{}, fmt.Errorf("%s version did not finish: %w", git, ctx.Err())
	}
	if err != nil {
		return GitVersion{}, fmt.Errorf("%s version: %w: %s", git, err, strings.TrimSpace(stderr.String()))
	}
	v, err := ParseGitVersion(string(out.Bytes()))
	if err != nil {
		return GitVersion{}, err
	}
	gitVersions.Store(key, v)
	return v, nil
}

// LazyFetchUnguardedError refuses a read in a partial clone with a git that
// does not honour GIT_NO_LAZY_FETCH. A missing object there would be fetched
// from the promisor remote, and the fetch runs what the repository's config
// names — core.sshCommand, credential helpers, a remote helper.
type LazyFetchUnguardedError struct {
	Git     string
	Version GitVersion
	Root    string
}

func (e *LazyFetchUnguardedError) Error() string {
	return fmt.Sprintf("%s is a partial clone, and git %s at %s does not honour GIT_NO_LAZY_FETCH, "+
		"so a read could fetch missing objects from its promisor remote and run the programs that "+
		"fetch runs; refusing to read. Upgrade git to 2.45.0 or later, or to a maintenance release "+
		"carrying the CVE-2024-32465 fix (2.39.4, 2.40.2, 2.41.1, 2.42.2, 2.43.4, 2.44.1)",
		e.Root, e.Version, e.Git)
}

// gitEnv is the environment every git DevCouncil runs gets: no terminal
// prompt, no optional index lock (a read must never leave index.lock in
// someone's checkout), and no lazy fetch from a promisor remote, so a
// missing object fails the read loudly instead of fetching.
func gitEnv() []string {
	return append(os.Environ(), "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0", "GIT_NO_LAZY_FETCH=1")
}
