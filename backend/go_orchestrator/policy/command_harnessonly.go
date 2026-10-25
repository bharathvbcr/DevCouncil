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
//     expands, and none may be review-append or serve (whose requests arrive
//     on stdin, where no word of the line shows them).
//
// "Wherever" because a wrapper (`env X=1 …`, `nice`, `xargs`) puts the binary
// later in the clause. Words are read as the shell splits them (shellWords),
// so a quoted argument is one word and text in a commit message or an echo is
// not a command. A quoted word is read as a command line of its own only when
// an earlier word runs one: a shell (`sh -c '…'`) or a program that hands its
// argument to one (sudo, su, watch, script, env -S, …). A word the shell would
// expand is recognised by its characters once quoting is gone: `$`, a
// backtick, `{`, `}`, `*`, `?`, `[`. That over-refuses a literal like
// `'rec?rd'`, which is not a subcommand anyway. What the rule cannot see is
// code it never reads — a script file, an interpreter's own argument — which
// is the boundary every rung that reads the line shares; an allowlist that
// admits such programs admits whatever they do.
var reviewReadSubcommands = map[string]bool{
	"subject": true, "list": true, "-h": true, "--help": true, "help": true,
}

// commandRunners hand an argument to a shell as a command line.
var commandRunners = map[string]bool{
	"sh": true, "bash": true, "zsh": true, "dash": true, "ksh": true, "fish": true, "busybox": true,
	"sudo": true, "su": true, "doas": true, "runuser": true, "watch": true, "script": true,
	"parallel": true, "env": true, "nohup": true, "setsid": true,
}

// maxHarnessDepth bounds the recursion into quoted command lines.
const maxHarnessDepth = 4

// harnessOnlyViolation reports why a clause runs a command only the harness
// may run, or "" when it does not.
//
// The clause is read twice: as the shell splits it, and with every backslash
// read as a path separator, because shellWords consumes a backslash as an
// escape and an unquoted Windows path (`C:\bin\devcouncil.exe`) would lose
// the separators that make it a path to the host.
func harnessOnlyViolation(clause string) string {
	if reason := harnessOnlyIn(clause, 0); reason != "" {
		return reason
	}
	if strings.Contains(clause, `\`) {
		return harnessOnlyIn(strings.ReplaceAll(clause, `\`, "/"), 0)
	}
	return ""
}

func harnessOnlyIn(clause string, depth int) string {
	words := shellWords(clause)
	runner := false
	for i, word := range words {
		switch binaryOf(word.text) {
		case "host":
			if reason := hostReviewWrite(words[i+1:]); reason != "" {
				return reason
			}
		case "store":
			if reason := storeReviewWrite(words[i+1:]); reason != "" {
				return reason
			}
		}
		if runner && strings.ContainsAny(word.text, " \t\n") {
			if depth >= maxHarnessDepth {
				return "a command line nested more than four levels deep cannot be read to the end, so " +
					"whether it records a review — which only the harness may — cannot be told."
			}
			if reason := harnessOnlyIn(word.text, depth+1); reason != "" {
				return reason
			}
		}
		if commandRunners[strings.TrimSuffix(strings.ToLower(path.Base(word.text)), ".exe")] {
			runner = true
		}
	}
	return ""
}

func hostReviewWrite(args []shellWord) string {
	if len(args) == 0 {
		return ""
	}
	sub := args[0].text
	if expands(sub) {
		return "the host subcommand `" + sub + "` is expanded by the shell, so which host command " +
			"runs — possibly `devcouncil review record`, which only the harness may run — cannot be " +
			"read from the line. Spell the subcommand out."
	}
	if sub != "review" || len(args) < 2 {
		return ""
	}
	action := args[1].text
	if reviewReadSubcommands[action] && !expands(action) {
		return ""
	}
	return "`devcouncil review " + action + "` is refused: only `review subject` and `review list` may run " +
		"in an agent's shell. `devcouncil review record` writes the independent review an llm_review " +
		"acceptance criterion is discharged by; it is run by the harness that ran the reviewers, or by " +
		"a person, never by the agent whose work was reviewed — an agent that could record its own " +
		"review could discharge the criterion by asserting it. Finish the change and ask for the review."
}

func storeReviewWrite(args []shellWord) string {
	for _, arg := range args {
		w := arg.text
		switch {
		case strings.EqualFold(w, "review-append"):
			return "`dcstore review-append` writes the independent review an llm_review acceptance " +
				"criterion is discharged by, which only the harness that ran the reviewers may record."
		case strings.EqualFold(w, "serve"):
			return "`dcstore serve` takes its requests on stdin, where no word of this line shows " +
				"whether one records a review — which only the harness that ran the reviewers may do."
		case expands(w):
			return "a dcstore argument (`" + w + "`) is expanded by the shell, so whether it writes a " +
				"review — which only the harness may record — cannot be read from the line."
		}
	}
	return ""
}

// binaryOf names which guarded binary a word runs, if any. A backslash left
// in the word is read as a Windows path separator (`C:\bin\devcouncil.exe`);
// shellWords has already consumed the ones that escaped a character.
func binaryOf(word string) string {
	word = strings.TrimLeft(word, "({")
	base := strings.ToLower(path.Base(strings.ReplaceAll(word, `\`, "/")))
	switch strings.TrimSuffix(base, ".exe") {
	case "dev", "devcouncil":
		return "host"
	case "dcstore":
		return "store"
	}
	return ""
}

// expands reports whether the shell would rewrite this word before the
// program sees it.
func expands(word string) bool {
	return strings.ContainsAny(word, "$`{}*?[")
}
