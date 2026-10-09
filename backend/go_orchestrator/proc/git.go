package proc

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"sort"
	"strings"
	"time"
)

// GitCommand builds a git read against root: the git LookPathOutside finds,
// run in root in its own process group, with the guards GitArgs adds. Both of the host's git runners
// build their commands here, so which git runs and with which guards has one
// owner. Capping output and classifying failures stay with the runners.
//
// A git that cannot be resolved is returned as an *exec.Error, the type a
// missing git produces, so callers report that git could not run.
func GitCommand(ctx context.Context, root string, args ...string) (*exec.Cmd, error) {
	git, err := LookPathOutside("git", root)
	if err != nil {
		return nil, &exec.Error{Name: "git", Err: err}
	}
	var filters []string
	if _, sub := splitSubcommand(args); !filterFree[sub] {
		filters, err = repositoryFilterOverrides(ctx, git, root)
		if err != nil {
			return nil, &exec.Error{Name: "git", Err: err}
		}
	}
	cmd := exec.CommandContext(ctx, git, append(filters, GitArgs(args)...)...)
	// Group isolation, as at every subprocess boundary in this module: a
	// deadline must reach the hooks, helpers and pagers git itself starts.
	ConfigureGroup(cmd)
	cmd.Dir = root
	// No terminal prompt, and no optional index lock: a read must never
	// block on a credential prompt or leave index.lock in someone's checkout.
	cmd.Env = append(os.Environ(), "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0")
	return cmd, nil
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
//     device (no hook can be found there), log.showSignature=false;
//   - after diff, log or show: --no-ext-diff --no-textconv;
//   - after blame: --no-textconv.
//
// Global options the caller passes before its subcommand (`-c k=v`, `-C dir`)
// are kept in place; the subcommand is the first argument that is not one.
//
// Filter drivers need the repository's config read first, because no flag
// disables them; GitCommand does that with repositoryFilterOverrides.
func GitArgs(args []string) []string {
	out := make([]string, 0, len(args)+12)
	out = append(out,
		"-c", "core.fsmonitor=false",
		"-c", "core.hooksPath="+os.DevNull,
		"-c", "log.showSignature=false",
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
	"diff":  {"--no-ext-diff", "--no-textconv"},
	"log":   {"--no-ext-diff", "--no-textconv"},
	"show":  {"--no-ext-diff", "--no-textconv"},
	"blame": {"--no-textconv"},
}

// filterVars are the settings that make a filter driver run a program, plus
// required, which turns a filter that cannot run into a failed read.
var filterVars = []string{"clean", "smudge", "process", "required"}

// filterConfigLimit bounds the filter listing read before each git read. A
// listing that does not fit is refused: a truncated one could omit a driver.
const filterConfigLimit = 1 << 20

// repositoryFilterOverrides returns -c arguments that neutralize every filter
// driver the repository itself defines.
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
func repositoryFilterOverrides(ctx context.Context, git, root string) ([]string, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, git, "-C", root, "config", "--show-scope", "-z", "--get-regexp", `^filter\.`)
	ConfigureGroup(cmd)
	cmd.Env = append(os.Environ(), "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0")
	out := &CappedBuffer{Limit: filterConfigLimit}
	var stderr bytes.Buffer
	cmd.Stdout = out
	cmd.Stderr = &stderr
	cmd.WaitDelay = 2 * time.Second
	err, timedOut := RunBounded(ctx, cmd.Run)
	if timedOut {
		return nil, fmt.Errorf("reading the repository's filter config did not finish: %w", ctx.Err())
	}
	if err != nil {
		var exit *exec.ExitError
		if errors.As(err, &exit) && (exit.ExitCode() == 1 || exit.ExitCode() == 128) {
			// 1: no filter defined anywhere. 128: git cannot enter root,
			// so there is no repository config; the read reports it.
			return nil, nil
		}
		return nil, fmt.Errorf("reading the repository's filter config: %w: %s", err, strings.TrimSpace(stderr.String()))
	}
	if out.Overflowed() {
		return nil, fmt.Errorf("the repository's filter config exceeds %d bytes; refusing to read with filters it could not check", filterConfigLimit)
	}
	return filterOverrides(out.Bytes())
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
