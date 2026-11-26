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

// wrapperSpec says how to step over one program wrapper's own options to the
// command it runs. A wrapper in commandWrappers without a spec (xargs, flock,
// watch, find -exec …) is classified only: its operand grammar is not one
// option table, and every word after it stays a candidate.
type wrapperSpec struct {
	// value and flags are short option letters that do and do not take a
	// value (attached, or the next word). digits allows nice's legacy -N.
	value, flags string
	digits       bool
	// long maps a long option to whether it takes a value in the next word
	// (--name=value always carries its own).
	long map[string]bool
	// operands are the words after the options that belong to the wrapper
	// (timeout's duration).
	operands int
	// assigns steps over NAME=value words (env, sudo).
	assigns bool
	// noRun lists short options after which nothing is run (command -v).
	noRun string
	// The options below are spelled with their dashes ("-C", "--chdir") and
	// must also be described by value/long. chdir options move where the
	// wrapped command's relative paths resolve (refused like cd); writes
	// options name a file the wrapper itself writes (time -o); edit options
	// make the wrapper an editor of its operands (sudo -e is sudoedit);
	// split options carry a command line the wrapper parses itself
	// (env -S).
	chdir, writes, edit, split []string
}

// wrapperSpecs is the option table of the wrappers argumentCommandWord peels.
// Its keys are a subset of commandWrappers. A wrapper option missing here is
// not guessed at: see peelCommand.
var wrapperSpecs = map[string]wrapperSpec{
	"command": {flags: "p", noRun: "vV"},
	"builtin": {},
	"exec":    {value: "a", flags: "cl"},
	"nohup":   {},
	"time": {value: "fo", flags: "pvalqV", writes: []string{"-o", "--output"},
		long: map[string]bool{"format": true, "output": true, "portability": false, "append": false, "verbose": false, "quiet": false}},
	"env": {value: "uCPS", flags: "i0v", assigns: true, chdir: []string{"-C", "--chdir"}, split: []string{"-S", "--split-string"},
		long: map[string]bool{"ignore-environment": false, "null": false, "unset": true, "chdir": true, "debug": false, "split-string": true}},
	"sudo": {value: "ugCDhprRtTUc", flags: "EHnSbkKAPBvlLsie", assigns: true, chdir: []string{"-D", "--chdir", "-R", "--chroot"},
		edit: []string{"-e", "--edit"}, long: map[string]bool{
			"user": true, "group": true, "chdir": true, "preserve-env": false, "set-home": false, "non-interactive": false,
			"stdin": false, "login": false, "shell": false, "background": false, "reset-timestamp": false,
			"remove-timestamp": false, "askpass": false, "close-from": true, "host": true, "prompt": true, "role": true,
			"type": true, "command-timeout": true, "other-user": true, "chroot": true, "login-class": true, "edit": false}},
	"doas":    {value: "uC", flags: "nsL"},
	"nice":    {value: "n", digits: true, long: map[string]bool{"adjustment": true}},
	"ionice":  {value: "cnpPu", flags: "ti", long: map[string]bool{"class": true, "classdata": true, "pid": true, "pgid": true, "uid": true, "ignore": false}},
	"timeout": {value: "sk", flags: "pvf", operands: 1, long: map[string]bool{"signal": true, "kill-after": true, "foreground": false, "preserve-status": false, "verbose": false}},
	"stdbuf":  {value: "ioe", long: map[string]bool{"input": true, "output": true, "error": true}},
	"setsid":  {flags: "cfwv", long: map[string]bool{"ctty": false, "fork": false, "wait": false, "verbose": false}},
}

// commandPeel is what stepping through a clause's wrappers found.
type commandPeel struct {
	// at is the index of the command word, or -1. With unknown set it is the
	// best guess readerBehindWrapper-style: the first later word naming a
	// program a rung knows (over-refusing is safe, skipping is not).
	at int
	// unknown is the first wrapper option wrapperSpecs does not describe.
	unknown string
	// chdir is a wrapper option that moves the working directory or root.
	chdir string
	// writes are files the wrappers themselves write (time -o FILE, sudo -e
	// FILE).
	writes []string
	// reparse is a command line a wrapper parses itself (env -S 'cat .env'),
	// with the words after it; the rungs judge it as a clause of its own.
	reparse     string
	reparseRest []shellWord
}

// reparseLine is the command line a wrapper that parses one itself runs: its
// own text, then the words after it, each quoted as one word.
func reparseLine(line string, rest []shellWord) string {
	var b strings.Builder
	b.WriteString(line)
	for _, w := range rest {
		b.WriteString(" '" + strings.ReplaceAll(w.text, "'", `'\''`) + "'")
	}
	return b.String()
}

func containsOption(set []string, key string) bool {
	for _, k := range set {
		if k == key {
			return true
		}
	}
	return false
}

// peelWrapper steps over the options of the wrapper at words[i] and returns
// the index of the command it runs, or -1 when nothing runs (the line ends, an
// option such as `command -v` says so, or p.unknown / p.reparse / an edit says
// the rest is not a command to peel to).
func peelWrapper(words []shellWord, i int, spec wrapperSpec, p *commandPeel) int {
	j := i + 1
	operands := spec.operands
	// note records what an option means; it returns false when the rest of
	// the line is not a command to peel to.
	note := func(key, value string, valueAt int) bool {
		switch {
		case containsOption(spec.chdir, key):
			if p.chdir == "" {
				p.chdir = key
			}
		case containsOption(spec.writes, key):
			p.writes = append(p.writes, value)
		case containsOption(spec.edit, key):
			for k := j + 1; k < len(words); k++ {
				if !strings.HasPrefix(words[k].text, "-") {
					p.writes = append(p.writes, words[k].text)
				}
			}
			return false
		case containsOption(spec.split, key):
			p.reparse = value
			if valueAt+1 < len(words) {
				p.reparseRest = words[valueAt+1:]
			}
			return false
		}
		return true
	}
	for ; j < len(words); j++ {
		text := words[j].text
		switch {
		case text == "--":
			j++
			goto afterOptions
		case spec.assigns && isAssignmentWord(text):
		case strings.HasPrefix(text, "--"):
			name, attached, hasValue := strings.Cut(text[2:], "=")
			takes, known := spec.long[name]
			if !known {
				p.unknown = text
				return -1
			}
			valueAt := j
			if takes && !hasValue {
				j++
				attached = ""
				if j < len(words) {
					attached = words[j].text
				}
				valueAt = j
			}
			if !note("--"+name, attached, valueAt) {
				return -1
			}
		case strings.HasPrefix(text, "-") && text != "-":
			cluster := text[1:]
			for c := 0; c < len(cluster); c++ {
				letter := cluster[c]
				key := "-" + string(letter)
				switch {
				case strings.IndexByte(spec.noRun, letter) >= 0:
					return -1
				case strings.IndexByte(spec.value, letter) >= 0:
					value, valueAt := cluster[c+1:], j
					if value == "" && j+1 < len(words) {
						j++
						value, valueAt = words[j].text, j
					}
					if !note(key, value, valueAt) {
						return -1
					}
					c = len(cluster)
				case strings.IndexByte(spec.flags, letter) >= 0:
					if !note(key, "", j) {
						return -1
					}
				case spec.digits && letter >= '0' && letter <= '9':
				default:
					p.unknown = text
					return -1
				}
			}
		case text == "-" && spec.assigns:
		default:
			goto afterOptions
		}
	}
afterOptions:
	for ; operands > 0 && j < len(words); operands-- {
		j++
	}
	if j >= len(words) {
		return -1
	}
	return j
}

// peelCommand finds the command word of one clause: assignments,
// redirections, reserved words and the wrappers of wrapperSpecs with their own
// options are stepped over, and what the options mean is reported.
func peelCommand(words []shellWord) commandPeel {
	p := commandPeel{at: -1}
	for i := 0; i < len(words); i++ {
		word := words[i]
		if word.quotedHead {
			p.at = i
			return p
		}
		if isAssignmentWord(word.text) || shellReservedPrefixes[word.text] {
			continue
		}
		if operand, isRedirect := redirectionPrefix(word.text); isRedirect {
			if operand {
				i++
			}
			continue
		}
		if spec, ok := wrapperSpecs[wrapperName(word.text)]; ok {
			next := peelWrapper(words, i, spec, &p)
			if next < 0 {
				if p.unknown != "" {
					p.at = guessCommandWord(words, i+1)
				}
				return p
			}
			i = next - 1
			continue
		}
		p.at = i
		return p
	}
	return p
}

// guessCommandWord is the command word of a clause whose wrapper options
// could not be read: the first word from `from` that names a program some rung
// models. It can only over-refuse; skipping the clause would not.
func guessCommandWord(words []shellWord, from int) int {
	for j := from; j < len(words); j++ {
		name := wrapperName(words[j].text)
		if pathWritingCommands[name] || fileReaders[name] || readsOnlyCommands[name] || isGitWord(words[j].text) ||
			name == "sort" || name == "uniq" {
			return j
		}
		if _, ok := wrapperSpecs[name]; ok {
			return j
		}
	}
	return -1
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
	commandPosition, wrapped := true, false
	runner := false
	for i, word := range words {
		base := strings.ToLower(path.Base(strings.ReplaceAll(strings.TrimLeft(word.text, "({"), `\`, "/")))
		// Inside a quoted command line (depth > 0) the binary is looked for in
		// every position: that text may be code in another language
		// (`os.system('devcouncil …')`) or a shell line flattened by codeWords,
		// where command position cannot be read. At the top level only
		// command position counts, so `rg dcstore` is not a run of dcstore.
		if commandPosition || wrapped || depth > 0 {
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
		// What is in command position next. Shell grammar keeps it: an
		// assignment, a reserved word or grouping (`(`, `{`, `!`, `then`,
		// `do`, …), a function head `name()`. A program wrapper (`sudo`,
		// `timeout`, `env`, …) takes options whose arity this rule cannot
		// know — `sudo -u x`, `timeout -s KILL 5`, `env -u HOME` — so after
		// one, every remaining word of the command is a candidate.
		switch {
		case commandPosition && isAssignmentWord(word.text):
		case commandPosition && (shellGrammar[word.text] || strings.HasSuffix(word.text, "()")):
		case commandWrappers[strings.TrimSuffix(base, ".exe")]:
			commandPosition, wrapped = true, true
		case wrapped:
		default:
			commandPosition = false
		}
	}
	return ""
}

// shellGrammar are the reserved words and grouping tokens after which the
// next word is still a command: `( devcouncil …)`, `! devcouncil …`,
// `if …; then devcouncil …`, `coproc devcouncil …`.
var shellGrammar = map[string]bool{
	"(": true, "{": true, "!": true, "if": true, "then": true, "else": true, "elif": true,
	"do": true, "while": true, "until": true, "coproc": true, "function": true,
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
