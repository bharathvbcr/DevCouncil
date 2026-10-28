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
	// notices are reasons about the scan itself, not about one path, and are
	// always listed: they must not be the thing the list cap swallows.
	notices []string
}

func (r *toolStateReasons) notice(reason string) { r.notices = append(r.notices, reason) }

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
	out := r.list
	if r.extra > 0 {
		out = append(out, fmt.Sprintf("tool state not secret-scanned: %d further path(s) not listed", r.extra))
	}
	return append(out, r.notices...)
}

// toolStateCapReason is the reason for tool state that does not fit in its own
// diff: what was collected is scanned, and the rest is said not to have been.
func toolStateCapReason() string {
	return fmt.Sprintf("tool state not secret-scanned: its diff exceeds %d bytes", gitDiffLimit)
}

// toolStateBinaryReason names a binary tool-state file. The wording is true
// whether dcverify read its bytes from the working tree (a tracked or untracked
// file, for a path it was told is in scope) or the host wrote them from the
// index: the secret scan ran on the raw bytes, and no text gate (stub, assert,
// skipped test) read them as text.
func toolStateBinaryReason(rel string) string {
	return "tool state is binary (secret scan ran on raw bytes; no text gates): " + toolStatePath(rel)
}

// toolStatePath spells a path for a reason: withheld when it is spelled like a
// credential file, C-quoted when it holds a control character.
func toolStatePath(rel string) string {
	if policy.SecretPathSpelling(rel) {
		return policy.WithheldSecretPath
	}
	return quoteDiffPath("", rel)
}

// toolStateScan is what collectToolState hands back.
type toolStateScan struct {
	// Diff is the worktree's view: `git diff HEAD` over the tracked files, and the
	// untracked ones (and, in a repository with no HEAD, the staged ones) read
	// from disk.
	Diff string
	// IndexDiff is the index's view of the paths where it is not what the
	// worktree holds. The worktree's diff says nothing about a secret that was
	// staged and then reverted or deleted on disk, but it is in the index, and the
	// next commit.
	IndexDiff string
	// BinaryPaths are the tool-state files dcverify must read from the working
	// tree because their diff section is a binary marker with no bytes: the only
	// paths it needs to be told are in scope. At most maxToolStateBinaryPaths; the
	// rest are named in Unscanned.
	BinaryPaths []string
	// Unscanned are the degraded reasons.
	Unscanned []string
}

// collectToolState builds the tool-state diffs and the reasons they are
// incomplete.
//
// untracked is the superproject's untracked, non-ignored listing, already read
// by the caller. A read of git that fails is an error, so a blocking
// gitStateGap, like every other read of the collection; a file that cannot be
// read is a reason, because a symlink or a socket under a tool-state directory
// is not a reason to refuse the whole change.
//
// The two diffs share one gitDiffLimit between them, so a path read in both
// views is not twice the room: tool state never takes room from the diff the
// other gates judge, and never takes more than one cap.
func collectToolState(ctx context.Context, git *proc.GitSession, rootFS *os.Root, untracked []string, headless bool) (scan toolStateScan, err error) {
	var b strings.Builder
	var reasons toolStateReasons
	spec := toolStatePathspecs()
	names := map[string]struct{}{}
	binaryPaths := map[string]struct{}{}
	for _, name := range untracked {
		if policy.IsToolState(name) {
			names[name] = struct{}{}
		}
	}
	if headless {
		// No HEAD to diff against: every tool-state file in the index is new,
		// and its worktree version is read from disk like an untracked one. Its
		// index version is read below, if the two differ.
		listing, err := runGit(ctx, git, gitListingLimit, append([]string{"ls-files", "-z", "--cached"}, spec...)...)
		if err != nil {
			return toolStateScan{}, err
		}
		for _, name := range strings.Split(string(listing), "\x00") {
			if name != "" && policy.IsToolState(name) {
				names[name] = struct{}{}
			}
		}
	} else {
		if err := trackedToolState(ctx, git, spec, &b, &reasons, binaryPaths); err != nil {
			return toolStateScan{}, err
		}
	}
	sorted := make([]string, 0, len(names))
	for name := range names {
		sorted = append(sorted, name)
	}
	sort.Strings(sorted)
	capped := false
	for _, name := range sorted {
		if err := ctx.Err(); err != nil {
			return toolStateScan{}, err
		}
		problem, binary, err := appendToolStateFile(ctx, rootFS, name, &b)
		var tooLarge diffTooLargeError
		if errors.As(err, &tooLarge) {
			reasons.add(toolStateCapReason())
			capped = true
			break
		}
		switch {
		case err != nil && ctx.Err() != nil:
			return toolStateScan{}, ctx.Err()
		case problem != "":
			reasons.add("tool state not secret-scanned: " + toolStatePath(name) + " (" + problem + ")")
		case binary:
			binaryPaths[name] = struct{}{}
			reasons.add(toolStateBinaryReason(name))
		}
	}
	var index strings.Builder
	if !capped {
		if err := indexToolState(ctx, git, spec, gitDiffLimit-b.Len(), &index, &reasons); err != nil {
			return toolStateScan{}, err
		}
	}
	scan = toolStateScan{Diff: b.String(), IndexDiff: index.String()}
	for path := range binaryPaths {
		scan.BinaryPaths = append(scan.BinaryPaths, path)
	}
	sort.Strings(scan.BinaryPaths)
	if extra := len(scan.BinaryPaths) - maxToolStateBinaryPaths; extra > 0 {
		// dcverify is told the first maxToolStateBinaryPaths: every planned path
		// is one more byte on the command line, and a large tree must not put the
		// run past the OS limit. The rest are not read as bytes, and say so.
		reasons.notice(fmt.Sprintf("tool state not secret-scanned: %d binary path(s) past the first %d were not read as bytes", extra, maxToolStateBinaryPaths))
		scan.BinaryPaths = scan.BinaryPaths[:maxToolStateBinaryPaths]
	}
	scan.Unscanned = reasons.all()
	return scan, nil
}

// maxToolStateBinaryPaths bounds the binary paths handed to dcverify as Planned.
const maxToolStateBinaryPaths = maxToolStateReasons

// maxToolStateIndexPaths bounds how many index versions are read. The rest are
// counted in a reason, not skipped quietly.
const maxToolStateIndexPaths = 1000

// nameSet runs a NUL-delimited name listing and returns its tool-state names.
func nameSet(ctx context.Context, git *proc.GitSession, reasons *toolStateReasons, args ...string) (map[string]struct{}, bool, error) {
	out, err := runGit(ctx, git, gitListingLimit, args...)
	switch {
	case errors.Is(err, ErrGitOutputTruncated):
		reasons.add(toolStateCapReason())
		return nil, false, nil
	case err != nil:
		return nil, false, err
	}
	set := map[string]struct{}{}
	for _, name := range strings.Split(string(out), "\x00") {
		if name != "" && policy.IsToolState(name) {
			set[name] = struct{}{}
		}
	}
	return set, true, nil
}

// literalSpec is a pathspec that names exactly the given paths, whatever the
// environment says about globs.
func literalSpec(names []string, exclude bool) []string {
	prefix := ":(top,literal)"
	if exclude {
		prefix = ":(top,exclude,literal)"
	}
	spec := []string{"--"}
	for _, name := range names {
		spec = append(spec, prefix+name)
	}
	return spec
}

// indexToolState adds the index's version of each tool-state path that is staged
// and that the worktree no longer holds.
//
// The worktree's diff is `git diff HEAD`, which compares the worktree to HEAD, so
// a secret staged and then reverted or deleted on disk is in no diff although it
// is in the index and goes in the next commit. A path is read here when the
// index differs from HEAD (`diff --cached`) and from the worktree (`diff`): where
// the index is HEAD there is nothing staged, and where it is the worktree the
// path is already read once, in the worktree's view, and is not read twice.
//
// Text is read as `git diff --cached` writes it, hunks and all, so that only what
// was staged is scanned. A binary path has no hunks to read, and no worktree file
// for dcverify to read its bytes from, so its blob is read with `git cat-file`
// and written as a section of added lines whatever it holds. Every read goes
// through the guarded session.
//
// budget is what is left of the one cap the two diffs share.
func indexToolState(ctx context.Context, git *proc.GitSession, spec []string, budget int, b *strings.Builder, reasons *toolStateReasons) error {
	staged, ok, err := nameSet(ctx, git, reasons, append([]string{"diff", "--cached", "--name-only", "-z", "--no-renames"}, spec...)...)
	if err != nil || !ok || len(staged) == 0 {
		return err
	}
	differ, ok, err := nameSet(ctx, git, reasons, append([]string{"diff", "--name-only", "-z", "--no-renames"}, spec...)...)
	if err != nil || !ok {
		return err
	}
	var want []string
	for name := range staged {
		if _, both := differ[name]; both {
			want = append(want, name)
		}
	}
	sort.Strings(want)
	if len(want) > maxToolStateIndexPaths {
		reasons.add("tool state not secret-scanned: " + fmt.Sprint(len(want)-maxToolStateIndexPaths) +
			" staged path(s) past the first " + fmt.Sprint(maxToolStateIndexPaths) + " were not read")
		want = want[:maxToolStateIndexPaths]
	}
	if len(want) == 0 {
		return nil
	}
	numstat, err := runGit(ctx, git, gitListingLimit, append([]string{"diff", "--cached", "--numstat", "-z", "--no-renames"}, literalSpec(want, false)...)...)
	switch {
	case errors.Is(err, ErrGitOutputTruncated):
		reasons.add(toolStateCapReason())
		return nil
	case err != nil:
		return err
	}
	var text, binary []string
	isBinary := map[string]bool{}
	for _, record := range bytes.Split(numstat, []byte{0}) {
		if len(record) == 0 {
			continue
		}
		parts := strings.SplitN(string(record), "\t", 3)
		if len(parts) != 3 || parts[2] == "" {
			return fmt.Errorf("git diff --cached --numstat named a tool-state change it could not be read as: refusing to read it as a shorter list")
		}
		isBinary[parts[2]] = parts[0] == "-" && parts[1] == "-"
	}
	for _, name := range want {
		if isBinary[name] {
			binary = append(binary, name)
		} else {
			text = append(text, name)
		}
	}
	if len(text) > 0 {
		remaining := budget - b.Len()
		if remaining <= 0 {
			reasons.add(toolStateCapReason())
			return nil
		}
		diff, err := runGit(ctx, git, remaining, append([]string{"-c", "core.quotePath=false", "diff", "--cached"}, literalSpec(text, false)...)...)
		switch {
		case errors.Is(err, ErrGitOutputTruncated):
			reasons.add(toolStateCapReason())
			return nil
		case err != nil:
			return err
		}
		b.Write(diff)
	}
	return indexBinaryToolState(ctx, git, binary, budget, b, reasons)
}

// indexBinaryToolState writes the staged blob of each binary tool-state path as a
// section of added lines.
func indexBinaryToolState(ctx context.Context, git *proc.GitSession, names []string, budget int, b *strings.Builder, reasons *toolStateReasons) error {
	if len(names) == 0 {
		return nil
	}
	listing, err := runGit(ctx, git, gitListingLimit, append([]string{"ls-files", "-z", "--stage"}, literalSpec(names, false)...)...)
	switch {
	case errors.Is(err, ErrGitOutputTruncated):
		reasons.add(toolStateCapReason())
		return nil
	case err != nil:
		return err
	}
	type entry struct{ mode, id, stage string }
	entries := map[string][]entry{}
	for _, record := range strings.Split(string(listing), "\x00") {
		if record == "" {
			continue
		}
		meta, name, ok := strings.Cut(record, "\t")
		fields := strings.Fields(meta)
		if !ok || len(fields) != 3 {
			return fmt.Errorf("git ls-files --stage named a tool-state entry it could not be read as: refusing to read it as a shorter list")
		}
		entries[name] = append(entries[name], entry{fields[0], fields[1], fields[2]})
	}
	for _, name := range names {
		if err := ctx.Err(); err != nil {
			return err
		}
		have := entries[name]
		switch {
		case len(have) == 0:
			// Staged for deletion: no bytes are introduced.
			continue
		case len(have) > 1 || have[0].stage != "0":
			reasons.add("tool state not secret-scanned: " + toolStatePath(name) + " (unmerged)")
			continue
		case have[0].mode == "160000":
			continue
		}
		reasons.add(toolStateBinaryReason(name))
		remaining := budget - b.Len()
		if remaining <= 0 {
			reasons.add(toolStateCapReason())
			return nil
		}
		blob, err := runGit(ctx, git, remaining, "cat-file", "blob", have[0].id)
		switch {
		case errors.Is(err, ErrGitOutputTruncated):
			reasons.add(toolStateCapReason())
			return nil
		case err != nil:
			return err
		}
		fragment := syntheticDiff(name, blob, false)
		if !strings.HasSuffix(fragment, "\n") {
			fragment += "\n"
		}
		if b.Len() > 0 && b.String()[b.Len()-1] != '\n' {
			b.WriteByte('\n')
		}
		if b.Len()+len(fragment) > budget {
			reasons.add(toolStateCapReason())
			return nil
		}
		b.WriteString(fragment)
	}
	return nil
}

// trackedToolState adds the diff of the tracked tool-state files, and names the binary
// ones. A diff past the cap is not an error: what fits is not collected, and
// the reason says the rest was not scanned.
func trackedToolState(ctx context.Context, git *proc.GitSession, spec []string, b *strings.Builder, reasons *toolStateReasons, binaryPaths map[string]struct{}) error {
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
			binaryPaths[parts[2]] = struct{}{}
			reasons.add(toolStateBinaryReason(parts[2]))
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
