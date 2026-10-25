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

// safeGitConfigKeys are the `-c` keys a read-only invocation may set: they
// change how output is laid out, never which program runs, which file is read
// or which tree is looked at. Matched case-insensitively, as git matches them.
// An `-c` naming any other key is refused, so a key git grows later is refused
// until someone decides it is safe — diff.external, core.sshCommand,
// core.fsmonitor and alias.* all run programs.
var safeGitConfigKeys = map[string]bool{
	"core.quotepath": true, "core.abbrev": true, "core.precomposeunicode": true,
	"log.decorate": true, "log.date": true, "log.abbrevcommit": true, "log.follow": true,
	"log.mailmap": true, "log.showroot": true,
	"diff.noprefix": true, "diff.mnemonicprefix": true, "diff.renames": true,
	"diff.relative": true, "diff.algorithm": true, "diff.colormoved": true,
	"diff.indentheuristic": true, "diff.context": true, "diff.interhunkcontext": true,
	"diff.statgraphwidth": true, "diff.suppressblankempty": true,
	"status.short": true, "status.branch": true, "status.showuntrackedfiles": true,
	"status.renames": true, "i18n.logoutputencoding": true,
	// Skipping the repository's own hooks is the person's call (owner
	// decision, 2026-10-24), as --no-verify is; see gitSafetyVariant.
	"core.hookspath": true,
}

// safeGitConfigPrefixes are the key families that only affect presentation.
var safeGitConfigPrefixes = []string{"color.", "advice.", "column."}

// safePagers are the pager values that page and nothing else. A pager is a
// program, so `core.pager=x` runs x; these are the ones agents use to turn
// paging off.
var safePagers = map[string]bool{
	"": true, "cat": true, "less": true, "more": true, "less -R": true,
	"less -FR": true, "less -FRX": true, "true": true, "false": true,
}

// safeGitConfig reports whether one `-c key=value` is safe on a read-only
// invocation. A key with no `=` is set to true, as git reads it.
func safeGitConfig(setting string) bool {
	if hasExpansion(setting) {
		return false
	}
	key, value, _ := strings.Cut(setting, "=")
	key = strings.ToLower(key)
	if key == "core.pager" || strings.HasPrefix(key, "pager.") {
		return safePagers[value]
	}
	if safeGitConfigKeys[key] {
		return true
	}
	for _, prefix := range safeGitConfigPrefixes {
		if strings.HasPrefix(key, prefix) {
			return true
		}
	}
	return false
}

// safeGitEnvironment reports whether a GIT_* assignment ahead of a read-only
// invocation only changes presentation or locking, or points git at an index
// file inside the repository that is not a credential path.
func safeGitEnvironment(name, value, root string) bool {
	if hasExpansion(value) {
		return false
	}
	switch name {
	case "GIT_PAGER":
		return safePagers[value]
	case "GIT_INDEX_FILE":
		return value != "" && classifyReadOperand(root, value) == ReadAllowed
	case "GIT_OPTIONAL_LOCKS", "GIT_TERMINAL_PROMPT", "GIT_CONFIG_NOSYSTEM", "GIT_FLUSH",
		"GIT_LITERAL_PATHSPECS", "GIT_GLOB_PATHSPECS", "GIT_NOGLOB_PATHSPECS",
		"GIT_ICASE_PATHSPECS", "GIT_NO_REPLACE_OBJECTS":
		return true
	}
	return false
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
		safe := safeGitGlobalOptions[name] && !hasValue
		if name == "-c" && !hasValue && i+1 < len(words) {
			safe = safeGitConfig(words[i+1].text)
		}
		if !isDirectory && !safe {
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
		if name, value, ok := strings.Cut(w.text, "="); ok && strings.HasPrefix(name, "GIT_") && isAssignmentWord(w.text) &&
			!safeGitEnvironment(name, value, root) {
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

// carriesSubstitution reports whether a word holds text only running a command
// can produce: a $( … ) or backtick span, or the placeholder the argument scan
// leaves where one was.
//
// It is the narrower question the operand rungs ask. A parameter expansion the
// line does not bind ($OUT, ${dir}) is no longer refused on sight (owner
// decision, 2026-10-24): expandBindings has already rewritten every variable
// the line or the well-known environment names, so what is left is a value
// the gate cannot know, and refusing every such operand blocked the loops and
// scratch variables agents write constantly. Such a word is judged by its
// literal text instead — `$X/.env` still names a credential file — and the
// cost, a parent environment pointing a variable somewhere the gate did not
// see, is the one the owner accepted. A substitution stays refused: it runs a
// command to choose the path, and that is not a value at all.
func carriesSubstitution(word string) bool {
	return strings.Contains(word, "$(") || strings.Contains(word, "`")
}

// readOnlyOperandViolation judges one non-option word of a read-only git
// command. It is a revision, a pathspec or `rev:path`, and the gate cannot tell
// which, so every reading of it is judged: the word itself, and each piece of
// it around ":" with any pathspec magic (`(top)`, `!`, `^`) removed. A revision
// that is also a plausible path (`origin/main`) is judged and passes, because a
// path inside the root that is not secret is readable.
func readOnlyOperandViolation(word, root string) string {
	if carriesSubstitution(word) {
		return "it carries a command substitution, so the path it names cannot be judged."
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
