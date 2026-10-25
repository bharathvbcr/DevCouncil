package policy

import (
	"path"
	"strings"
)

// The commands only the harness may run. See RuleCommandHarnessOnly.
//
// Judged word by word rather than by pattern, because the shell decides what
// a word becomes and a pattern over the text does not: `review {record,}`,
// `review $'record'`, `review rec?rd`, `review "$(echo record)"` and
// `review\<newline> record` all run `review record`, and none of them
// contains it. So the rule is an allowlist over what follows the binary, not
// a denylist of one spelling:
//
//   - where a command runs the host binary (dev, devcouncil, any path to
//     them, any letter case — macOS filesystems ignore it), the word after it
//     must be a literal subcommand; after `review`, it must be one of the read
//     subcommands (subject, list) or a help flag;
//   - where a command runs dcstore, no later word may be one the shell
//     expands, and none may be review-append or serve (whose requests arrive
//     on stdin, where no word of the line shows them).
//
// "Where a command runs" is command position: the first word of a command,
// past NAME=value assignments and past wrappers that run their argument
// (`env`, `nice -n 5`, `xargs`, `sudo`, `timeout 5`, `find … -exec`, …). An
// argument that merely names the binary — `rg dcstore`, `npm run dev $ARGS` —
// is not a command. A quoted argument is read as command lines of its own
// when something before it may run one: a shell or runner, a program named
// by a variable (`$SHELL -c '…'`), or a code flag (`-c`, `-lc`, `-e`,
// `--command`, `--eval`); inside it, `;`, `&`, `|` and newlines separate
// commands and code punctuation separates words, so an interpreter's
// `os.system('devcouncil review record')` is read too. A word the shell would
// expand is one shellWords marks `expands`: an expansion character outside
// single quotes. What the rule cannot see is code it never reads — a script
// file — the boundary every rung that reads the line shares; an allowlist
// that admits such programs admits whatever they do.
var reviewReadSubcommands = map[string]bool{
	"subject": true, "list": true, "-h": true, "--help": true, "help": true,
}

// commandRunners hand an argument to a shell as a command line.
var commandRunners = map[string]bool{
	"sh": true, "bash": true, "zsh": true, "dash": true, "ksh": true, "fish": true, "busybox": true,
	"sudo": true, "su": true, "doas": true, "runuser": true, "watch": true, "script": true,
	"parallel": true, "env": true, "nohup": true, "setsid": true,
}

// commandWrappers run the command that follows them (after their own
// options), so the word after them is in command position.
var commandWrappers = map[string]bool{
	"env": true, "nice": true, "ionice": true, "time": true, "command": true, "exec": true,
	"builtin": true, "nohup": true, "setsid": true, "sudo": true, "doas": true, "xargs": true,
	"timeout": true, "stdbuf": true, "flock": true, "chronic": true, "caffeinate": true,
	"watch": true, "parallel": true, "-exec": true, "-execdir": true, "-ok": true, "-okdir": true,
	"chrt": true, "taskset": true, "unbuffer": true, "time.exe": true,
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

func harnessOnlyIn(text string, depth int) string {
	for _, segment := range commandSegments(text) {
		if reason := harnessOnlyInCommand(shellWords(segment), depth); reason != "" {
			return reason
		}
	}
	return ""
}

func harnessOnlyInCommand(words []shellWord, depth int) string {
	commandPosition := true
	runner := false
	for i, word := range words {
		base := strings.ToLower(path.Base(strings.ReplaceAll(strings.TrimLeft(word.text, "({"), `\`, "/")))
		// Inside a quoted command line (depth > 0) the binary is looked for in
		// every position: that text may be code in another language
		// (`os.system('devcouncil …')`) or a shell line flattened by codeWords,
		// where command position cannot be read. At the top level only
		// command position counts, so `rg dcstore` is not a run of dcstore.
		if commandPosition || depth > 0 {
			switch binaryOf(word.text) {
			case "host":
				return hostReviewWrite(words[i+1:])
			case "store":
				return storeReviewWrite(words[i+1:])
			}
		}
		// A quoted multi-word argument is read as command lines when
		// something before it may run one.
		if i > 0 && (words[0].expands || codeFlag(words[i-1].text)) {
			runner = true
		}
		if runner && strings.ContainsAny(word.text, " \t\n;&|") {
			if depth >= maxHarnessDepth {
				return "a command line nested more than four levels deep cannot be read to the end, so " +
					"whether it records a review — which only the harness may — cannot be told."
			}
			line := word.text
			// `--split-string='…'`: the command line is the option's value.
			if strings.HasPrefix(line, "-") {
				if _, value, ok := strings.Cut(line, "="); ok {
					line = value
				}
			}
			if reason := harnessOnlyIn(codeWords(line), depth+1); reason != "" {
				return reason
			}
		}
		if commandRunners[strings.TrimSuffix(base, ".exe")] {
			runner = true
		}
		// What is in command position next: still, after an assignment, a
		// wrapper, or a wrapper's own option or numeric argument; otherwise
		// the command word has been passed and the rest are its arguments.
		switch {
		case commandPosition && isAssignmentWord(word.text):
		case commandWrappers[strings.TrimSuffix(base, ".exe")]:
			commandPosition = true
		case commandPosition && i > 0 && wrapperArgument(word.text):
		default:
			commandPosition = commandWrappers[strings.TrimSuffix(base, ".exe")]
		}
	}
	return ""
}

// wrapperArgument reports whether a word is the kind a wrapper takes before
// the command it runs: an option, a number or duration, or a lock path for
// flock — anything but a bare program name is treated as one, so the program
// that follows is still in command position. A bare word ends the wrapper's
// arguments.
func wrapperArgument(word string) bool {
	if strings.HasPrefix(word, "-") {
		return true
	}
	if word == "" {
		return false
	}
	trimmed := strings.TrimRight(word, "smhd")
	for _, c := range trimmed {
		if !(c >= '0' && c <= '9' || c == '.') {
			return strings.ContainsAny(word, "/") && !isProgramPath(word)
		}
	}
	return true
}

// isProgramPath reports whether a word with a slash names one of the guarded
// programs, so it is the command, not a wrapper's path argument.
func isProgramPath(word string) bool {
	return binaryOf(word) != ""
}

// commandSegments splits a command line at the operators that separate
// commands — `;`, `&`, `|` and newlines — outside quotes.
func commandSegments(text string) []string {
	var out []string
	var cur strings.Builder
	quote := rune(0)
	runes := []rune(text)
	for i := 0; i < len(runes); i++ {
		r := runes[i]
		switch {
		case quote == 0 && r == '\\' && i+1 < len(runes):
			cur.WriteRune(r)
			cur.WriteRune(runes[i+1])
			i++
			continue
		case quote == 0 && (r == '\'' || r == '"'):
			quote = r
		case quote != 0 && r == quote:
			quote = 0
		case quote == 0 && (r == ';' || r == '&' || r == '|' || r == '\n'):
			out = append(out, cur.String())
			cur.Reset()
			continue
		}
		cur.WriteRune(r)
	}
	return append(out, cur.String())
}

func hostReviewWrite(args []shellWord) string {
	if len(args) == 0 {
		return ""
	}
	sub := args[0]
	if sub.expands {
		return "the host subcommand `" + sub.text + "` is expanded by the shell, so which host command " +
			"runs — possibly `devcouncil review record`, which only the harness may run — cannot be " +
			"read from the line. Spell the subcommand out."
	}
	if sub.text != "review" || len(args) < 2 {
		return ""
	}
	action := args[1]
	if reviewReadSubcommands[action.text] && !action.expands {
		return ""
	}
	return "`devcouncil review " + action.text + "` is refused: only `review subject` and `review list` may run " +
		"in an agent's shell. `devcouncil review record` writes the independent review an llm_review " +
		"acceptance criterion is discharged by; it is run by the harness that ran the reviewers, or by " +
		"a person, never by the agent whose work was reviewed — an agent that could record its own " +
		"review could discharge the criterion by asserting it. Finish the change and ask for the review."
}

func storeReviewWrite(args []shellWord) string {
	for _, arg := range args {
		switch {
		case strings.EqualFold(arg.text, "review-append"):
			return "`dcstore review-append` writes the independent review an llm_review acceptance " +
				"criterion is discharged by, which only the harness that ran the reviewers may record."
		case strings.EqualFold(arg.text, "serve"):
			return "`dcstore serve` takes its requests on stdin, where no word of this line shows " +
				"whether one records a review — which only the harness that ran the reviewers may do."
		case arg.expands:
			return "a dcstore argument (`" + arg.text + "`) is expanded by the shell, so whether it writes a " +
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

// codeFlag reports whether a word is the flag that hands the next argument to
// a shell or interpreter as code: a short-option cluster holding c or e in
// either case (`-c`, `-lc`, `-ec`, `-e`, `-E`, `-Command`), or --command /
// --eval / --exec.
func codeFlag(word string) bool {
	switch strings.ToLower(word) {
	case "--command", "--eval", "--exec", "-command", "-encodedcommand", "--split-string":
		return true
	}
	if strings.HasPrefix(strings.ToLower(word), "--split-string=") {
		return true
	}
	if len(word) < 2 || word[0] != '-' || word[1] == '-' {
		return false
	}
	for _, c := range word[1:] {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z') {
			return false
		}
	}
	return strings.ContainsAny(strings.ToLower(word[1:]), "ce")
}

// codeWords reads a command line that may be code in another language: the
// punctuation that separates a call from its arguments there (`(`, `)`, `,`,
// `[`, `]`, quotes) separates words here, so
// `os.system('devcouncil review record')` reads as `devcouncil review record`.
// `;` stays, to separate commands.
func codeWords(text string) string {
	return strings.Map(func(r rune) rune {
		switch r {
		case '(', ')', ',', '[', ']', '\'', '"', '`':
			return ' '
		}
		return r
	}, text)
}
