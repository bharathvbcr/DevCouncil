package policy

import (
	"fmt"
	"strings"
)

// readOnlyGitSubcommands are the git subcommands whose purpose is to look. They
// are the ones a no-lease allowlist (`git status`, `git diff *`) or a task's
// `git log *` admits by their leading words alone, so what follows those words
// is the whole of what the command can do — and this rung is what reads it.
var readOnlyGitSubcommands = map[string]bool{
	"diff": true, "log": true, "show": true, "status": true, "whatchanged": true,
}

// safeGitGlobalOptions are the global options a read-only invocation may carry:
// presentation and locking only. Every other one is refused, so an option git
// grows later is refused until someone decides it is safe. The ones that matter
// today are -c and --config-env (they set diff.external, core.pager, aliases),
// and --exec-path.
var safeGitGlobalOptions = map[string]bool{
	"--no-pager": true, "--no-optional-locks": true, "-P": true,
	"--no-replace-objects": true, "--literal-pathspecs": true,
	"--glob-pathspecs": true, "--noglob-pathspecs": true, "--icase-pathspecs": true,
}

// gitUnsafeLongOptions are the long options of diff, log and show that write a
// file, read a file the repository does not hold, or run a program. A word
// refuses when it is a prefix of one of these: git (and argparse) accept any
// unambiguous abbreviation, so `--outp=x` writes x exactly as `--output=x`
// does, and an abbreviation that git would reject as ambiguous is refused too,
// which costs nothing because it would not run.
var gitUnsafeLongOptions = []struct{ name, why string }{
	{"--output", "writes the diff to a file"},
	{"--no-index", "compares files outside the repository, bypassing the path rules"},
	{"--ext-diff", "runs an external diff program"},
	{"--textconv", "runs the configured textconv programs"},
	{"--orderfile", "reads a file outside the path rules"},
}

// gitShortOptionsWithValue are the short options of diff, log and show whose
// value is the rest of the cluster (or the next word): `-pO../x` is -p and
// -O ../x, and the S in `-Sfoo` is a search string, not a flag.
const gitShortOptionsWithValue = "SGUMCBlLIOXn"

// readOnlyArgumentViolation reports why a clause that runs a read-only git
// subcommand does more than read the working tree, or "" when it does not.
//
// An allowlist entry like `git diff *` says which command may run; it says
// nothing about the arguments, and git's arguments reach well beyond "show me
// the change": --output writes a file, --no-index reads any two paths on disk
// (git also switches to it by itself when a path argument lies outside the
// repository), `-c diff.external=…` and GIT_EXTERNAL_DIFF run a program, and a
// pathspec or `rev:path` can name .env. None of that went through the file
// rules, which is what this rung is for: it reads the clause's own words —
// dequoted by shellWords, the splitter the rest of the ladder uses — and
// refuses anything that is not plain orientation. It fails closed: a word it
// cannot resolve to a literal (an expansion) is refused, not guessed at.
//
// root may be empty. With a root, paths are judged by ClassifyRead, which also
// follows symlinks; without one, by their text alone.
func readOnlyArgumentViolation(clause, root string) string {
	words := shellWords(clause)
	at := argumentCommandWord(words)
	if at < 0 || !isGitWord(words[at].text) {
		return ""
	}
	// Global options sit between `git` and the subcommand. Whether the
	// invocation is a read-only one is only known once the subcommand is found,
	// so the first unsafe global is remembered and reported then.
	unsafeGlobal := ""
	i := at + 1
	for ; i < len(words) && strings.HasPrefix(words[i].text, "-"); i++ {
		opt := words[i].text
		name, _, hasValue := strings.Cut(opt, "=")
		// The directory options keep their own refusal, command.directory_change,
		// which names them for what they do; they are stepped over here.
		isDirectory := name == "-C" || name == "--git-dir" || name == "--work-tree"
		if !isDirectory && (!safeGitGlobalOptions[name] || hasValue) {
			if unsafeGlobal == "" {
				unsafeGlobal = opt
			}
		}
		if !hasValue && gitGlobalOptionsWithValue[name] {
			i++ // the value is not the subcommand
		}
	}
	if i >= len(words) || !readOnlyGitSubcommands[words[i].text] {
		return ""
	}
	sub := words[i].text
	if unsafeGlobal != "" {
		return fmt.Sprintf("`git %s` here runs with the global option `%s`, which can point git at "+
			"other configuration, programs or trees and is not part of read-only orientation.", sub, unsafeGlobal)
	}
	for _, w := range words[:at] {
		if name, _, ok := strings.Cut(w.text, "="); ok && strings.HasPrefix(name, "GIT_") && isAssignmentWord(w.text) {
			return fmt.Sprintf("`git %s` here runs with %s set, which redirects git to other "+
				"programs, configuration or trees than the ones the gate judged.", sub, name)
		}
	}

	endOfOptions := false
	for _, w := range words[i+1:] {
		text := w.text
		switch {
		case !endOfOptions && text == "--":
			endOfOptions = true
		case !endOfOptions && strings.HasPrefix(text, "--"):
			name, _, _ := strings.Cut(text, "=")
			if hasExpansion(name) {
				return fmt.Sprintf("`git %s` has an option (`%s`) that only the shell can spell out, so the "+
					"gate cannot tell what it is.", sub, text)
			}
			for _, bad := range gitUnsafeLongOptions {
				if strings.HasPrefix(bad.name, name) {
					return fmt.Sprintf("`git %s %s` is refused: %s (%s).", sub, text, bad.why, bad.name)
				}
			}
		case !endOfOptions && len(text) > 1 && text[0] == '-':
			if hasExpansion(text[:2]) {
				return fmt.Sprintf("`git %s` has an option (`%s`) that only the shell can spell out.", sub, text)
			}
			for _, c := range text[1:] {
				if c == 'O' {
					return fmt.Sprintf("`git %s %s` is refused: -O reads an order file outside the path rules.", sub, text)
				}
				if strings.ContainsRune(gitShortOptionsWithValue, c) {
					break // the rest of the cluster is this option's value
				}
			}
		default:
			if reason := readOnlyOperandViolation(text, root); reason != "" {
				return fmt.Sprintf("`git %s %s` is refused: %s", sub, text, reason)
			}
		}
	}
	return ""
}

// hasExpansion reports whether a word carries a parameter expansion, a
// substitution or a leading tilde — text whose value only the shell knows.
// A tilde elsewhere is a revision suffix (`HEAD~1`), not an expansion.
func hasExpansion(word string) bool {
	return strings.ContainsAny(word, "$`") || strings.HasPrefix(word, "~")
}

// readOnlyOperandViolation judges one non-option word of a read-only git
// command. It is a revision, a pathspec or `rev:path`, and the gate cannot tell
// which, so every reading of it is judged: the word itself, and each piece of
// it around ":" with any pathspec magic (`(top)`, `!`, `^`) removed. A revision
// that is also a plausible path (`origin/main`) is judged and passes, because a
// path inside the root that is not secret is readable.
func readOnlyOperandViolation(word, root string) string {
	if hasExpansion(word) {
		return "it carries an expansion only the shell can resolve, so the path it names cannot be judged."
	}
	pieces := []string{word}
	for _, piece := range strings.Split(word, ":") {
		piece = strings.TrimLeft(piece, "!^")
		if strings.HasPrefix(piece, "(") {
			if end := strings.IndexByte(piece, ')'); end >= 0 {
				piece = piece[end+1:]
			}
		}
		pieces = append(pieces, piece)
	}
	for _, piece := range pieces {
		if piece == "" {
			continue
		}
		if class := classifyReadOperand(root, piece); class == ReadOutsideRoot {
			return "it names a path outside the repository (git compares such a path as files on disk)."
		} else if class == ReadSecretPath {
			return "it names a path that holds credentials, whose contents would enter the model's context."
		}
	}
	return ""
}

// classifyReadOperand is ClassifyRead with a fallback for a gate that has no
// root: text alone decides, so an absolute path or a `..` component is outside
// and a credential name is secret.
func classifyReadOperand(root, operand string) ReadClass {
	if root != "" {
		return ClassifyRead(root, operand)
	}
	slashed := normalizeSlashes(operand)
	if strings.HasPrefix(slashed, "/") || strings.HasPrefix(slashed, "\\") || strings.Contains(operand, `:\`) {
		return ReadOutsideRoot
	}
	for _, part := range strings.Split(slashed, "/") {
		if part == ".." {
			return ReadOutsideRoot
		}
	}
	if SecretPathSpelling(operand) {
		return ReadSecretPath
	}
	return ReadAllowed
}
