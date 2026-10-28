package proc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

// GitCommand builds one git read against root: NewGitSession and
// GitSession.Command in one call, for a caller that makes a single read. An
// operation that makes several reads of one root holds a GitSession instead,
// so the repository's config is read once rather than once per read.
//
// A git that cannot be resolved, or a read that is refused, is returned as an
// *exec.Error, the type a missing git produces, so callers report that git
// could not run.
func GitCommand(ctx context.Context, root string, args ...string) (*exec.Cmd, error) {
	session, err := NewGitSession(root)
	if err != nil {
		return nil, err
	}
	return session.Command(ctx, args...)
}

// GitSession is one operation's git reads against one root: the git
// LookPathOutside finds, its version, and the repository config the guards
// are computed from. Both of the host's git runners build their commands
// here, so which git runs and with which guards has one owner. Capping output
// and classifying failures stay with the runners.
//
// The config is read once per session and read again only when the
// repository's own config files change (their identity, size and mtime are
// compared before each read). A session lives as long as one operation —
// a get_diff, a verify's git state — and no longer: a value pulled in through
// include.path from another file is not watched, so a session must not
// outlive the operation it was made for.
type GitSession struct {
	root string
	git  string

	mu       sync.Mutex
	config   *repositoryConfig
	stamp    configStamp
	stampSet bool
}

// NewGitSession resolves git for reads against root.
func NewGitSession(root string) (*GitSession, error) {
	git, err := LookPathOutside("git", root)
	if err != nil {
		return nil, &exec.Error{Name: "git", Err: err}
	}
	return &GitSession{root: root, git: git}, nil
}

// Root is the directory the session reads.
func (s *GitSession) Root() string { return s.root }

// Command builds a git read: run in the session's root in its own process
// group, with the guards GitArgs adds, the repository's filter drivers
// neutralized (readRepositoryConfig), and no lazy fetch.
//
// A git that does not honour GIT_NO_LAZY_FETCH may still read an ordinary
// clone; in a partial clone the read is refused with a
// *LazyFetchUnguardedError, because a missing object would be fetched and
// the fetch runs what the repository's config names. This is decision D9 in
// ft-ff2153d9.
func (s *GitSession) Command(ctx context.Context, args ...string) (*exec.Cmd, error) {
	_, sub := splitSubcommand(args)
	free := filterFree[sub]
	// The version is asked only when it decides something: a filter-free
	// read skips the config listing on a git that honours GIT_NO_LAZY_FETCH,
	// and any other read lists the config anyway and asks the version only
	// when the listing shows a promisor.
	honours := false
	if free {
		version, err := DetectGitVersion(ctx, s.git)
		if err != nil {
			return nil, &exec.Error{Name: "git", Err: err}
		}
		honours = version.HonorsNoLazyFetch()
	}
	var filters []string
	if !honours {
		config, err := s.repositoryConfig(ctx)
		if err != nil {
			return nil, &exec.Error{Name: "git", Err: err}
		}
		if config.partialClone {
			version, err := DetectGitVersion(ctx, s.git)
			if err != nil {
				return nil, &exec.Error{Name: "git", Err: err}
			}
			if !version.HonorsNoLazyFetch() {
				return nil, &exec.Error{Name: "git", Err: &LazyFetchUnguardedError{Git: s.git, Version: version, Root: s.root}}
			}
		}
		if !free {
			filters = config.filters
		}
	}
	cmd := exec.CommandContext(ctx, s.git, append(filters, GitArgs(args)...)...)
	// Group isolation, as at every subprocess boundary in this module: a
	// deadline must reach the hooks, helpers and pagers git itself starts.
	ConfigureGroup(cmd)
	cmd.Dir = s.root
	cmd.Env = gitEnv()
	return cmd, nil
}

// repositoryConfig is the repository's config, read once and kept while the
// repository's own config files are unchanged.
func (s *GitSession) repositoryConfig(ctx context.Context) (*repositoryConfig, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	stamp, stampOK := stampRepositoryConfig(s.root)
	if s.config != nil && stampOK && s.stampSet && stamp == s.stamp {
		return s.config, nil
	}
	config, err := readRepositoryConfig(ctx, s.git, s.root)
	if err != nil {
		return nil, err
	}
	s.config, s.stamp, s.stampSet = config, stamp, stampOK
	return config, nil
}

// GitArgs returns args with the guards every DevCouncil git read carries.
//
// A repository's own .git/config can name programs that git runs during an
// ordinary read, and a .git/config can arrive with the repository. Verified
// against git 2.54: core.fsmonitor runs on diff and ls-files, diff.external
// on diff, a diff driver's textconv on blame, and gpg.program on log through
// log.showSignature when a commit carries a signature header. A planted
// fsmonitor that answers "nothing changed" also hides changes from the read.
// The guards switch each of those off from the command line, which outranks
// every config file:
//
//   - before the subcommand: core.fsmonitor=false, core.hooksPath at the null
//     device (no hook can be found there), log.showSignature=false,
//     core.useReplaceRefs=false. A `git replace` ref rewrites history at read
//     time, and the repository under analysis can carry one: a commit that adds
//     a credential, replaced by a clean twin, is read as the twin by every
//     diff, log and show unless replace refs are switched off;
//   - after diff, log or show: --no-ext-diff --no-textconv --no-color;
//   - after blame: --no-textconv;
//   - after diff and status: --ignore-submodules=dirty. Asked whether a
//     submodule's working tree is dirty, git runs status inside it, and that
//     status runs the submodule's own filter drivers and fsmonitor, which
//     the superproject's config listing never sees. A submodule whose
//     checked-out commit moved is still reported; GitSession.DirtySubmodules
//     reports working-tree dirtiness with a guarded read of each submodule.
//
// --no-color is not about running a program, but it is the same kind of hole:
// color.ui or color.diff set to "always" colors output even into a pipe, and a
// reader that recognizes "diff --git" or "@@" lines — the secret-path filter on
// every diff an agent or model is shown, among others — then recognizes none.
//
// Global options the caller passes before its subcommand (`-c k=v`, `-C dir`)
// are kept in place; the subcommand is the first argument that is not one.
//
// Filter drivers need the repository's config read first, because no flag
// disables them; GitSession.Command does that with readRepositoryConfig.
func GitArgs(args []string) []string {
	out := make([]string, 0, len(args)+12)
	out = append(out,
		"-c", "core.fsmonitor=false",
		"-c", "core.hooksPath="+os.DevNull,
		"-c", "log.showSignature=false",
		"-c", "core.useReplaceRefs=false",
	)
	i, _ := splitSubcommand(args)
	out = append(out, args[:i]...)
	if i < len(args) {
		out = append(out, args[i])
		out = append(out, subcommandGuards[args[i]]...)
		out = append(out, args[i+1:]...)
	}
	return out
}

// splitSubcommand returns the index of the subcommand in args and its name:
// the first argument that is not a global option (`-c k=v` and `-C dir` take
// a value). With no subcommand the index is len(args) and the name empty.
func splitSubcommand(args []string) (int, string) {
	i := 0
	for i < len(args) && strings.HasPrefix(args[i], "-") {
		if args[i] == "-c" || args[i] == "-C" {
			i++
		}
		i++
	}
	if i >= len(args) {
		return len(args), ""
	}
	return i, args[i]
}

// filterFree names the read subcommands that never pass a working-tree file
// through a filter driver, so GitCommand skips reading the filter config for
// them. Listed this way round on purpose: a subcommand added later gets the
// filter read until someone shows it cannot need one.
var filterFree = map[string]bool{"rev-parse": true, "rev-list": true, "log": true}

var subcommandGuards = map[string][]string{
	"diff":   {"--no-ext-diff", "--no-textconv", "--no-color", "--ignore-submodules=dirty"},
	"log":    {"--no-ext-diff", "--no-textconv", "--no-color"},
	"show":   {"--no-ext-diff", "--no-textconv", "--no-color"},
	"blame":  {"--no-textconv"},
	"status": {"--ignore-submodules=dirty"},
}

// filterVars are the settings that make a filter driver run a program, plus
// required, which turns a filter that cannot run into a failed read.
var filterVars = []string{"clean", "smudge", "process", "required"}

// filterConfigLimit bounds the config listing read before a git read. A
// listing that does not fit is refused: a truncated one could omit a driver.
const filterConfigLimit = 1 << 20

// repositoryConfigPattern selects what one `git config` listing must carry:
// every filter driver setting, and the settings that make the repository a
// partial clone, so detecting one costs no extra spawn. Keys are matched in
// git's canonical form, section and variable lowercased.
const repositoryConfigPattern = `^(filter\.|extensions\.partialclone$|remote\..*\.(promisor|partialclonefilter)$)`

// repositoryConfig is what the guards need from the repository's config.
type repositoryConfig struct {
	// filters are -c arguments neutralizing the repository's filter drivers.
	filters []string
	// partialClone: a promisor remote may supply missing objects on demand.
	partialClone bool
}

// readRepositoryConfig lists the repository's filter and partial-clone
// config once, with scopes.
//
// filter.<name>.clean runs on diff, and no flag turns filters off, so the
// repository's config is read first (reading config runs nothing). A driver
// any repository-scoped entry touches — scope local or worktree, which
// includes values pulled in through include.path from a tracked file — has
// each setting the repository gave it set again on the command line: to the
// operator's own value when system, global or command scope defines one (a
// repository that redefines git-lfs's filter gets git-lfs's back), else to
// empty. Only those settings: git treats an empty process as configured, so
// blanking one the repository never set would stop the operator's clean from
// running. Its required is always set, to the operator's value or false, so a
// blanked filter is skipped rather than failing the read. Drivers only the
// operator defines, such as a global git-lfs install, are left alone. This is
// the owner's decision recorded in ft-ff2153d9.
//
// A driver name a -c argument cannot spell (one containing "=") is refused
// rather than left running. A root git cannot enter has no repository config
// to neutralize; git reports the root itself.
func readRepositoryConfig(ctx context.Context, git, root string) (*repositoryConfig, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, git, "-C", root, "config", "--show-scope", "-z", "--get-regexp", repositoryConfigPattern)
	ConfigureGroup(cmd)
	cmd.Env = gitEnv()
	out := &CappedBuffer{Limit: filterConfigLimit}
	var stderr bytes.Buffer
	cmd.Stdout = out
	cmd.Stderr = &stderr
	cmd.WaitDelay = 2 * time.Second
	err, timedOut := RunBounded(ctx, cmd.Run)
	if timedOut {
		return nil, fmt.Errorf("reading the repository's config did not finish: %w", ctx.Err())
	}
	if err != nil {
		var exit *exec.ExitError
		if errors.As(err, &exit) && (exit.ExitCode() == 1 || exit.ExitCode() == 128) {
			// 1: nothing matched anywhere. 128: git cannot enter root, so
			// there is no repository config; the read reports it.
			return &repositoryConfig{}, nil
		}
		return nil, fmt.Errorf("reading the repository's config: %w: %s", err, strings.TrimSpace(stderr.String()))
	}
	if out.Overflowed() {
		return nil, fmt.Errorf("the repository's filter config exceeds %d bytes; refusing to read with filters it could not check", filterConfigLimit)
	}
	filters, err := filterOverrides(out.Bytes())
	if err != nil {
		return nil, err
	}
	return &repositoryConfig{filters: filters, partialClone: isPartialClone(out.Bytes())}, nil
}

// isPartialClone reads the same listing for the settings that let a
// promisor remote supply missing objects: extensions.partialClone, a
// remote's promisor flag, or a remote's partial-clone filter. Any scope
// counts; a promisor configured globally fetches just the same.
func isPartialClone(listing []byte) bool {
	fields := bytes.Split(listing, []byte{0})
	for i := 0; i+1 < len(fields); i += 2 {
		key, value, hasValue := strings.Cut(string(fields[i+1]), "\n")
		switch {
		case key == "extensions.partialclone":
			if strings.TrimSpace(value) != "" {
				return true
			}
		case strings.HasPrefix(key, "remote.") && strings.HasSuffix(key, ".promisor"):
			if !hasValue || gitBool(value) {
				return true
			}
		case strings.HasPrefix(key, "remote.") && strings.HasSuffix(key, ".partialclonefilter"):
			if strings.TrimSpace(value) != "" {
				return true
			}
		}
	}
	return false
}

// gitBool reads a config boolean as git does. A value git would reject
// counts as true: an unreadable promisor flag is not evidence of no promisor.
func gitBool(value string) bool {
	switch strings.ToLower(strings.TrimSpace(value)) {
	case "false", "no", "off", "0", "":
		return false
	}
	return true
}

// configStamp identifies the repository's own config files as they are now:
// the common config and the worktree's config.worktree.
type configStamp [2]fileStamp

type fileStamp struct {
	exists     bool
	dev, ino   uint64
	size       int64
	mtime      int64
	statFailed bool
}

// stampRepositoryConfig finds root's git directory without running git — a
// .git directory, or a .git file naming one, and its commondir — and stamps
// the config files in it. ok is false when root is not a checkout's top
// (a subdirectory, a bare repository): then nothing is cached.
func stampRepositoryConfig(root string) (configStamp, bool) {
	gitDir := filepath.Join(root, ".git")
	info, err := os.Stat(gitDir)
	if err != nil {
		return configStamp{}, false
	}
	if !info.IsDir() {
		data, err := os.ReadFile(gitDir)
		if err != nil {
			return configStamp{}, false
		}
		target, ok := strings.CutPrefix(strings.TrimSpace(string(data)), "gitdir: ")
		if !ok {
			return configStamp{}, false
		}
		if !filepath.IsAbs(target) {
			target = filepath.Join(root, target)
		}
		gitDir = target
	}
	commonDir := gitDir
	if data, err := os.ReadFile(filepath.Join(gitDir, "commondir")); err == nil {
		common := strings.TrimSpace(string(data))
		if !filepath.IsAbs(common) {
			common = filepath.Join(gitDir, common)
		}
		commonDir = common
	}
	return configStamp{
		stampFile(filepath.Join(commonDir, "config")),
		stampFile(filepath.Join(gitDir, "config.worktree")),
	}, true
}

// filterOverrides turns `git config --show-scope -z --get-regexp ^filter\.`
// output (scope NUL key [LF value] NUL, repeated) into -c arguments.
func filterOverrides(listing []byte) ([]string, error) {
	type setting struct {
		value    string
		hasValue bool
	}
	operator := map[string]setting{}
	repository := map[string]bool{}
	repositoryKeys := map[string]bool{}
	fields := bytes.Split(listing, []byte{0})
	for i := 0; i+1 < len(fields); i += 2 {
		scope := string(fields[i])
		key, value, hasValue := strings.Cut(string(fields[i+1]), "\n")
		dot := strings.LastIndexByte(key, '.')
		if !strings.HasPrefix(key, "filter.") || dot <= len("filter.") {
			continue
		}
		name := key[len("filter."):dot]
		switch scope {
		case "local", "worktree":
			repository[name] = true
			repositoryKeys[key] = true
		default:
			// system, global and command: the operator's own settings. Git
			// lists them in precedence order, so the last one wins.
			operator[key] = setting{value: value, hasValue: hasValue}
		}
	}
	names := make([]string, 0, len(repository))
	for name := range repository {
		if strings.Contains(name, "=") {
			return nil, fmt.Errorf("the repository defines filter %q, whose name a -c argument cannot spell; refusing to read with it", name)
		}
		names = append(names, name)
	}
	sort.Strings(names)
	var args []string
	for _, name := range names {
		for _, v := range filterVars {
			key := "filter." + name + "." + v
			if v != "required" && !repositoryKeys[key] {
				continue
			}
			switch own, ok := operator[key]; {
			case ok && own.hasValue:
				args = append(args, "-c", key+"="+own.value)
			case ok:
				args = append(args, "-c", key)
			case v == "required":
				args = append(args, "-c", key+"=false")
			default:
				args = append(args, "-c", key+"=")
			}
		}
	}
	return args, nil
}
