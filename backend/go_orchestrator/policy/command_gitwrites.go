package policy

import "strings"

// gitWrites reads the git subcommands that change working-tree files named on
// their command line: checkout and restore overwrite them, rm and clean delete
// them, mv moves them. `git apply` changes files named inside a patch, which is
// the unjudged case.
//
// `git checkout` takes either a branch or paths, and without `--` the words do
// not say which. A word after the first is judged as a path only when its
// spelling is one the hard rules refuse; judging every word as a write would
// refuse `git checkout main` for a task whose scope does not list "main".
func gitWrites(args []shellWord) argWrites {
	at := gitSubcommandAt(args)
	if at < 0 {
		return argWrites{}
	}
	sub, rest := args[at].text, args[at+1:]
	switch sub {
	case "apply":
		return argWrites{unjudged: gitApplyUnjudged(args)}
	case "checkout":
		return gitCheckoutWrites(rest)
	case "restore":
		return gitRestoreWrites(rest)
	case "rm":
		p := parseOptArgs(rest, "", "pathspec-from-file")
		if p.has("--cached", "-n", "--dry-run") {
			return argWrites{}
		}
		w := literalDeletes(p.operands)
		if p.has("--pathspec-from-file") {
			w.unjudged = "`git rm --pathspec-from-file` takes its paths from a file"
		}
		return w
	case "mv":
		targets, opaque := copyWrites(rest, true)
		return argWrites{targets: targets, opaque: opaque}
	case "clean":
		p := parseOptArgs(rest, "e", "exclude")
		if p.has("-n", "--dry-run") {
			return argWrites{}
		}
		if len(p.operands) == 0 {
			return argWrites{unjudged: "`git clean` removes every untracked file under the working directory"}
		}
		return literalDeletes(p.operands)
	}
	return argWrites{}
}

// siblingGitReadOnly are the git subcommands that only read: none of them
// changes a file, the index, or a ref of the worktree it is run in.
var siblingGitReadOnly = map[string]bool{
	"status": true, "diff": true, "log": true, "show": true, "whatchanged": true,
	"rev-parse": true, "rev-list": true, "merge-base": true, "ls-files": true,
	"ls-tree": true, "cat-file": true, "blame": true, "describe": true,
	"for-each-ref": true, "show-ref": true, "shortlog": true,
	// `fetch` updates remote-tracking refs, never the working tree or a local branch.
	"fetch": true,
	// `add` and `commit` record into the index and the current branch; neither
	// rewrites or discards a file in the working tree. `commit --amend` rewrites
	// a commit and is refused below.
	"add": true, "commit": true,
}

// siblingGitUnjudged says why a git command run in ANOTHER session's registered
// worktree (`git -C <sibling> ...`) may not run, or "" when it may.
//
// A write there is the other session's work. The modelled writes (checkout,
// restore, rm, mv, clean) name files the sibling evaluator can judge against the
// hard rules, but the hard rules are all it checks: it knows nothing of the
// sibling's plan or its lease holder, so a modelled `checkout -- .` would
// discard another session's edits and still pass. And no write rule models
// `reset`, `stash`, `switch`, `merge`, `rebase`, `pull`, `am`, `read-tree`,
// `update-ref`, `branch -D` or `worktree remove`, so a deny-list could not be
// complete. The rule is therefore a closed allowlist: git may look at a sibling
// and record into it, and everything else is refused with the reason below.
// Dry runs and listings are looked at, not run.
func siblingGitUnjudged(args []shellWord) string {
	at := gitSubcommandAt(args)
	if at < 0 {
		return ""
	}
	sub, rest := args[at].text, args[at+1:]
	switch sub {
	case "commit":
		if parseOptArgs(rest, "mCcF", "message", "file", "author", "date", "reuse-message", "reedit-message", "fixup", "squash", "template", "cleanup", "trailer").has("--amend") {
			return siblingGitRefusal(sub+" --amend", "rewrites a commit")
		}
		return ""
	case "branch":
		p := parseOptArgs(rest, "", "contains", "no-contains", "merged", "no-merged", "points-at", "format", "sort", "color", "column", "abbrev")
		if p.has("-d", "-D", "--delete", "-m", "-M", "--move", "-c", "-C", "--copy", "-f", "--force",
			"-u", "--set-upstream-to", "--unset-upstream", "--edit-description", "--track", "-t", "--no-track") {
			return siblingGitRefusal("branch", "creates, moves or deletes a branch")
		}
		if len(p.operands) > 0 && !p.has("-l", "--list", "--contains", "--no-contains", "--merged", "--no-merged", "--points-at") {
			return siblingGitRefusal("branch", "creates a branch")
		}
		return ""
	case "worktree":
		if first := firstOperand(rest); first == "list" {
			return ""
		}
		return siblingGitRefusal("worktree", "adds, moves or removes a worktree")
	case "stash":
		if first := firstOperand(rest); first == "list" || first == "show" {
			return ""
		}
		return siblingGitRefusal("stash", "moves another session's uncommitted work out of its working tree")
	case "apply":
		if gitApplyUnjudged(args) == "" {
			return ""
		}
		return siblingGitRefusal("apply", "changes the files named inside the patch")
	}
	if siblingGitReadOnly[sub] {
		return ""
	}
	return siblingGitRefusal(sub, "can rewrite or discard files, the index or refs in that worktree")
}

// firstOperand is the first word of a git subcommand's arguments that is not an
// option, or "".
func firstOperand(words []shellWord) string {
	for _, w := range words {
		if !strings.HasPrefix(w.text, "-") {
			return w.text
		}
	}
	return ""
}

func siblingGitRefusal(sub, why string) string {
	return "`git " + sub + "` " + why + " in another session's worktree; there git may only read " +
		"(status, diff, log, show, rev-parse, ls-files, blame, a branch or worktree or stash listing, " +
		"`apply --check`) or record (add, commit, fetch)"
}

// gitSubcommandAt is the index of the subcommand among the words after `git`,
// stepping over global options and unquoted redirections, or -1.
func gitSubcommandAt(words []shellWord) int {
	for i := 0; i < len(words); {
		w := words[i]
		if !w.quotedHead {
			if targetNext, isRedirect := redirectionPrefix(w.text); isRedirect {
				i++
				if targetNext && i < len(words) {
					i++
				}
				continue
			}
		}
		if w.text == "--" || w.text == "-" || !strings.HasPrefix(w.text, "-") {
			return i
		}
		_, _, _, next := stepGitGlobalOption(words, i)
		i = next
	}
	return -1
}

func gitCheckoutWrites(args []shellWord) argWrites {
	var words []string
	dashdash := -1
	for _, a := range args {
		text := a.text
		if text == "--" && dashdash < 0 {
			dashdash = len(words)
			continue
		}
		if dashdash < 0 && strings.HasPrefix(text, "-") && text != "-" {
			name, _, _ := strings.Cut(text, "=")
			switch name {
			case "-b", "-B", "--orphan":
				return argWrites{} // creates a branch; what follows is a start point
			case "--pathspec-from-file":
				return argWrites{unjudged: "`git checkout --pathspec-from-file` takes its paths from a file"}
			}
			continue
		}
		words = append(words, text)
	}
	if dashdash >= 0 {
		return literalWrites(words[dashdash:])
	}
	// No `--`: the first word may be a tree-ish and the rest can only be
	// paths, but a lone word is a branch or a path and the line does not say.
	// A word is judged when it is not the first, or when its spelling is one
	// the hard rules refuse whichever it is.
	var written []string
	for i, word := range words {
		if i > 0 || protectedSpelling(word) {
			written = append(written, word)
		}
	}
	return literalWrites(written)
}

func gitRestoreWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, "s", "source", "pathspec-from-file", "conflict", "unmerged")
	if p.has("--pathspec-from-file") {
		return argWrites{unjudged: "`git restore --pathspec-from-file` takes its paths from a file"}
	}
	// --staged alone restores the index, not the working tree.
	if (p.has("-S", "--staged")) && !p.has("-W", "--worktree") {
		return argWrites{}
	}
	return literalWrites(p.operands)
}

// protectedSpelling reports whether a word, read as a repository path by its
// text alone, is one the hard rules refuse to let anything write: a credential
// path, a protected repository path, or one outside the root.
func protectedSpelling(word string) bool {
	if word == "" || hasExpansion(word) {
		return false
	}
	if classifyReadOperand("", word) != ReadAllowed {
		return true
	}
	cleaned := strings.TrimPrefix(normalizeSlashes(word), "./")
	return matchesRestricted(matcherOf(nil), cleaned) || IsPromptInput(cleaned)
}
