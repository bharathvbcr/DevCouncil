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

// gitSubcommandAt is the index of the subcommand among the words after `git`,
// stepping over the global options that take a value, or -1.
func gitSubcommandAt(words []shellWord) int {
	for i := 0; i < len(words); i++ {
		text := words[i].text
		if !strings.HasPrefix(text, "-") {
			return i
		}
		name, _, hasValue := strings.Cut(text, "=")
		if !hasValue && gitGlobalOptionsWithValue[name] {
			i++
		}
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
