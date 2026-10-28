package verify

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// Tool state is secret-scanned, and nothing else about it is judged.
//
// policy.ToolStateDirs are the directories DevCouncil's own tools write into.
// The diff verification judges leaves them out, because a file the harness wrote
// is not work the agent did and blaming it told an obedient agent to revert the
// task store. That left a credential written there in no scan at all, although
// it is committed with the rest of the change. Owner decision (2026-10-28):
// the secret gate reads them.
//
// They are read into a second diff, ToolStateDiff, and never into Diff or
// Changed, so the scope, stub, coverage and substance gates, the subject hash
// and every other reader of the change see exactly what they saw before. Only
// secret_scan findings from the second pass are kept (see runRigorGates).
//
// WHAT IS READ is what git sees: tracked files (`git diff HEAD`) and untracked
// files git does not ignore (`ls-files --others --exclude-standard`).
//
// THE LIMIT, NAMED: a file git ignores is not read. In this repository
// .devcouncil/ and .devmap/ are in .gitignore, so a credential written to an
// ignored path under them (an operator's secrets file beside secrets.env.example,
// say) stays unscanned by design: reading ignored state would put the task store
// and the repo map in front of the scanner on every verify, and would block on
// the operator's own credentials. Moving that line is the owner's decision and
// TestToolStateIgnoredByGitIsOutOfScope holds it where it is.
//
// What cannot be read as text is named in ToolStateUnscanned, which Run carries
// into degraded_reasons: a binary file, a symlink (never followed), a file that
// is not regular or cannot be read, and a diff past gitDiffLimit.

// toolStatePathspecs selects policy.ToolStateDirs from the top of the tree. The
// match is case-insensitive because policy.IsToolState folds case, so a
// `.DevCouncil/` the policy calls tool state is read, too.
func toolStatePathspecs() []string {
	spec := []string{"--"}
	for _, dir := range policy.ToolStateDirs {
		spec = append(spec, ":(top,icase)"+dir)
	}
	return spec
}

// maxToolStateReasons bounds the unscanned paths listed one by one. The rest are
// counted in one more reason, so a tree of ten thousand binary files is neither
// a payload that size nor a list that looks complete.
const maxToolStateReasons = 100

type toolStateReasons struct {
	list  []string
	extra int
}

func (r *toolStateReasons) add(reason string) {
	for _, have := range r.list {
		if have == reason {
			return
		}
	}
	if len(r.list) >= maxToolStateReasons {
		r.extra++
		return
	}
	r.list = append(r.list, reason)
}

func (r *toolStateReasons) all() []string {
	if r.extra > 0 {
		return append(r.list, fmt.Sprintf("tool state not secret-scanned: %d further path(s) not listed", r.extra))
	}
	return r.list
}

// toolStateCapReason is the reason for tool state that does not fit in its own
// diff: what was collected is scanned, and the rest is said not to have been.
func toolStateCapReason() string {
	return fmt.Sprintf("tool state not secret-scanned: its diff exceeds %d bytes", gitDiffLimit)
}

// toolStatePath spells a path for a reason: withheld when it is spelled like a
// credential file, C-quoted when it holds a control character.
func toolStatePath(rel string) string {
	if policy.SecretPathSpelling(rel) {
		return policy.WithheldSecretPath
	}
	return quoteDiffPath("", rel)
}

// collectToolState builds ToolStateDiff and the reasons it is incomplete.
//
// untracked is the superproject's untracked, non-ignored listing, already read
// by the caller. A read of git that fails is an error, so a blocking
// gitStateGap, like every other read of the collection; a file that cannot be
// read is a reason, because a symlink or a socket under a tool-state directory
// is not a reason to refuse the whole change.
//
// The second diff has its own gitDiffLimit: tool state never takes room from the
// diff the other gates judge.
func collectToolState(ctx context.Context, git *proc.GitSession, rootFS *os.Root, untracked []string, headless bool) (diff string, unscanned []string, err error) {
	var b strings.Builder
	var reasons toolStateReasons
	spec := toolStatePathspecs()
	names := map[string]struct{}{}
	for _, name := range untracked {
		if policy.IsToolState(name) {
			names[name] = struct{}{}
		}
	}
	if headless {
		// No HEAD to diff against: every tool-state file in the index is new,
		// and is read from disk like an untracked one.
		listing, err := runGit(ctx, git, gitListingLimit, append([]string{"ls-files", "-z", "--cached"}, spec...)...)
		if err != nil {
			return "", nil, err
		}
		for _, name := range strings.Split(string(listing), "\x00") {
			if name != "" && policy.IsToolState(name) {
				names[name] = struct{}{}
			}
		}
	} else {
		if err := trackedToolState(ctx, git, spec, &b, &reasons); err != nil {
			return "", nil, err
		}
	}
	sorted := make([]string, 0, len(names))
	for name := range names {
		sorted = append(sorted, name)
	}
	sort.Strings(sorted)
	for _, name := range sorted {
		if err := ctx.Err(); err != nil {
			return "", nil, err
		}
		problem, binary, err := appendToolStateFile(ctx, rootFS, name, &b)
		var tooLarge diffTooLargeError
		if errors.As(err, &tooLarge) {
			reasons.add(toolStateCapReason())
			break
		}
		switch {
		case err != nil && ctx.Err() != nil:
			return "", nil, ctx.Err()
		case problem != "":
			reasons.add("tool state not secret-scanned: " + toolStatePath(name) + " (" + problem + ")")
		case binary:
			reasons.add("tool state not text-scanned (binary): " + toolStatePath(name))
		}
	}
	return b.String(), reasons.all(), nil
}

// trackedToolState adds the diff of the tracked tool-state files, and names the binary
// ones. A diff past the cap is not an error: what fits is not collected, and
// the reason says the rest was not scanned.
func trackedToolState(ctx context.Context, git *proc.GitSession, spec []string, b *strings.Builder, reasons *toolStateReasons) error {
	numstat, err := runGit(ctx, git, gitListingLimit, append([]string{"diff", "HEAD", "--numstat", "-z", "--no-renames"}, spec...)...)
	switch {
	case errors.Is(err, ErrGitOutputTruncated):
		reasons.add(toolStateCapReason())
		return nil
	case err != nil:
		return err
	}
	for _, record := range bytes.Split(numstat, []byte{0}) {
		if len(record) == 0 {
			continue
		}
		parts := strings.SplitN(string(record), "\t", 3)
		if len(parts) != 3 || parts[2] == "" {
			return fmt.Errorf("git diff --numstat named a tool-state change it could not be read as: refusing to read it as a shorter list")
		}
		if parts[0] == "-" && parts[1] == "-" {
			reasons.add("tool state not text-scanned (binary): " + toolStatePath(parts[2]))
		}
	}
	diffBytes, err := runGit(ctx, git, gitDiffLimit, append([]string{"-c", "core.quotePath=false", "diff", "HEAD"}, spec...)...)
	switch {
	case errors.Is(err, ErrGitOutputTruncated):
		reasons.add(toolStateCapReason())
		return nil
	case err != nil:
		return err
	}
	b.Write(diffBytes)
	return nil
}

// appendToolStateFile adds one untracked tool-state file to the second diff.
// problem is why it could not be read as a regular file ("" when it could), and
// binary is a file whose section names it and carries none of its bytes. A diff
// past the cap is the returned error, a diffTooLargeError.
func appendToolStateFile(ctx context.Context, rootFS *os.Root, rel string, b *strings.Builder) (problem string, binary bool, err error) {
	info, err := rootFS.Lstat(rel)
	switch {
	case errors.Is(err, fs.ErrNotExist):
		// Listed and gone: a race with the tool that owns it, with nothing to read.
		return "", false, nil
	case err != nil:
		return "unreadable", false, err
	case info.Mode()&os.ModeSymlink != 0:
		return "symlink", false, nil
	case !info.Mode().IsRegular():
		return "not a regular file", false, nil
	}
	if b.Len() > 0 && b.String()[b.Len()-1] != '\n' {
		b.WriteByte('\n')
	}
	binary, err = appendRegularFile(ctx, rootFS, rel, b)
	if err != nil {
		var tooLarge diffTooLargeError
		if errors.As(err, &tooLarge) {
			return "", false, err
		}
		return "unreadable", false, err
	}
	return "", binary, nil
}
