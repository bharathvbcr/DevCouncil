package policy

import (
	"path"
	"strings"
)

// The commands only the harness may run. See RuleCommandHarnessOnly.
//
// Judged word by word rather than by pattern, because the shell decides what
// a word becomes and a pattern over the text does not: `review {record,}`,
// `review $'record'`, `review rec?rd` and `review "$(echo record)"` all run
// `review record`, and none of them contains it. So the rule is an allowlist
// over what follows the binary, not a denylist of one spelling:
//
//   - wherever a word names the host binary (dev, devcouncil, any path to
//     them, any letter case — macOS filesystems ignore it), the word after it
//     must be a literal subcommand; after `review`, it must be one of the
//     read subcommands (subject, list) or a help flag;
//   - wherever a word names dcstore, no later word may be one the shell
//     expands, and none may be review-append.
//
// "Wherever" because a wrapper (`env X=1 …`, `nice`, `xargs`, `sh -c '…'`)
// puts the binary later in the clause. A word that the shell would expand is
// recognised by its characters, after quotes and backslashes are removed:
// `$`, a backtick, `{`, `}`, `*`, `?`, `[`. That over-refuses a literal like
// `'rec?rd'`, which is not a subcommand anyway. What the rule cannot see is
// code it never reads — a script file, an interpreter's argument — which is
// the boundary every rung that reads the line shares; an allowlist that
// admits such programs admits whatever they do.
var reviewReadSubcommands = map[string]bool{
	"subject": true, "list": true, "-h": true, "--help": true, "help": true,
}

// harnessOnlyViolation reports why a clause runs a command only the harness
// may run, or "" when it does not.
func harnessOnlyViolation(clause string) string {
	words := harnessWords(clause)
	for i, word := range words {
		switch binaryOf(word) {
		case "host":
			if reason := hostReviewWrite(words[i+1:]); reason != "" {
				return reason
			}
		case "store":
			if reason := storeReviewWrite(words[i+1:]); reason != "" {
				return reason
			}
		}
	}
	return ""
}

func hostReviewWrite(args []string) string {
	if len(args) == 0 {
		return ""
	}
	sub := bareWord(args[0])
	if expands(sub) {
		return "the host subcommand `" + args[0] + "` is expanded by the shell, so which host command " +
			"runs — possibly `devcouncil review record`, which only the harness may run — cannot be " +
			"read from the line. Spell the subcommand out."
	}
	if sub != "review" || len(args) < 2 {
		return ""
	}
	action := bareWord(args[1])
	if reviewReadSubcommands[action] && !expands(action) {
		return ""
	}
	return "`devcouncil review " + args[1] + "` is refused: only `review subject` and `review list` may run " +
		"in an agent's shell. `devcouncil review record` writes the independent review an llm_review " +
		"acceptance criterion is discharged by; it is run by the harness that ran the reviewers, or by " +
		"a person, never by the agent whose work was reviewed — an agent that could record its own " +
		"review could discharge the criterion by asserting it. Finish the change and ask for the review."
}

func storeReviewWrite(args []string) string {
	for _, arg := range args {
		w := bareWord(arg)
		if strings.EqualFold(w, "review-append") {
			return "`dcstore review-append` writes the independent review an llm_review acceptance " +
				"criterion is discharged by, which only the harness that ran the reviewers may record."
		}
		if expands(w) {
			return "a dcstore argument (`" + arg + "`) is expanded by the shell, so whether it writes a " +
				"review — which only the harness may record — cannot be read from the line."
		}
	}
	return ""
}

// binaryOf names which guarded binary a word runs, if any. A backslash is
// read both ways — as a Windows path separator (`C:\bin\devcouncil.exe`) and
// as a shell escape (`de\vcouncil`) — and either reading counts.
func binaryOf(word string) string {
	unquoted := strings.NewReplacer(`'`, "", `"`, "").Replace(word)
	for _, form := range []string{strings.ReplaceAll(unquoted, `\`, "/"), strings.ReplaceAll(unquoted, `\`, "")} {
		switch strings.TrimSuffix(strings.ToLower(path.Base(form)), ".exe") {
		case "dev", "devcouncil":
			return "host"
		case "dcstore":
			return "store"
		}
	}
	return ""
}

// harnessWords splits a clause into words at whitespace and at the operators
// that end a word without a space (`x;devcouncil`, `(dcstore`). It ignores
// quoting on purpose, unlike shellWords: a command handed to `sh -c` as one
// quoted word is still a command, and this rule must see its words.
func harnessWords(clause string) []string {
	return strings.FieldsFunc(clause, func(r rune) bool {
		switch r {
		case ' ', '\t', '\n', '\r', ';', '&', '|', '(', ')', '<', '>':
			return true
		}
		return false
	})
}

// bareWord is word with the shell's quoting removed — what the program would
// see, short of expansion.
func bareWord(word string) string {
	return strings.NewReplacer(`'`, "", `"`, "", `\`, "").Replace(word)
}

// expands reports whether the shell would rewrite this word before the
// program sees it.
func expands(word string) bool {
	return strings.ContainsAny(word, "$`{}*?[")
}
