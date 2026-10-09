package proc

import (
	"context"
	"os"
	"os/exec"
	"strings"
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
	cmd := exec.CommandContext(ctx, git, GitArgs(args)...)
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
// Not covered here: filter drivers (filter.<name>.clean runs on diff, and no
// flag disables it) — tracked in ft-ff2153d9 for the owner's decision.
func GitArgs(args []string) []string {
	out := make([]string, 0, len(args)+12)
	out = append(out,
		"-c", "core.fsmonitor=false",
		"-c", "core.hooksPath="+os.DevNull,
		"-c", "log.showSignature=false",
	)
	i := 0
	for i < len(args) && strings.HasPrefix(args[i], "-") {
		if args[i] == "-c" || args[i] == "-C" {
			i++
		}
		i++
	}
	i = min(i, len(args))
	out = append(out, args[:i]...)
	if i < len(args) {
		out = append(out, args[i])
		out = append(out, subcommandGuards[args[i]]...)
		out = append(out, args[i+1:]...)
	}
	return out
}

var subcommandGuards = map[string][]string{
	"diff":  {"--no-ext-diff", "--no-textconv"},
	"log":   {"--no-ext-diff", "--no-textconv"},
	"show":  {"--no-ext-diff", "--no-textconv"},
	"blame": {"--no-textconv"},
}
