package policy

import (
	"fmt"
	"os"
	"path"
	"path/filepath"
	"strings"
)

// nonReadingCommands are command words that take a path without reading its
// contents: they print it, list it, test it, or change its metadata. Naming
// `.env` to one of them — `echo .env >> .gitignore` — is not a read.
var nonReadingCommands = map[string]bool{
	"echo": true, "printf": true, "ls": true, "stat": true, "test": true,
	"[": true, "[[": true, "rm": true, "mkdir": true, "touch": true,
	"chmod": true, "chown": true, "true": true, "false": true,
}

// gitContentSubcommands are the git subcommands, beyond diff/log/show (which
// readOnlyArgumentViolation judges), that print file contents. Every other
// subcommand moves, stages or lists paths, and staging has its own refusal.
var gitContentSubcommands = map[string]bool{
	"cat-file": true, "grep": true, "blame": true, "annotate": true,
	"archive": true, "format-patch": true, "difftool": true,
}

// fileReaders are the commands whose operands are always files they read or
// copy. An operand of theirs that a command substitution chooses (`cat
// $(pick)`) cannot be judged, and is refused rather than guessed at. A
// variable (`cat $F`) is judged by its literal text once expandBindings has
// substituted what it can; see carriesSubstitution.
var fileReaders = map[string]bool{
	"cat": true, "head": true, "tail": true, "less": true, "more": true,
	"tac": true, "nl": true, "base64": true, "xxd": true, "od": true,
	"hexdump": true, "strings": true, "cp": true, "dd": true, "source": true, ".": true,
	"jq": true, "cmp": true, "sed": true,
}

// maxGlobMatchesJudged bounds what one wildcard operand is expanded to.
const maxGlobMatchesJudged = 1024

// secretOperandViolation reports why a clause names a credential file as
// something to read, or "" when it does not.
//
// It reads the clause the way every rung here does — shellWords, so quotes are
// removed as sh removes them — and judges each operand, the value of a
// `--opt=value` or `of=value` word, and the target of an input redirection
// (`< .env`). An operand is judged by classifyReadOperand, which follows
// symlinks inside the root, and a wildcard is expanded against the working tree
// so `cat .e*` is judged as the file it matches. Output redirections are not
// reads and are left to the write rungs.
//
// What it cannot see is the same boundary the other rungs have: a script on
// disk, an interpreter's own argument (`python -c "open('.env')"`), an
// expansion outside fileReaders. It is a rung against the plain commands, which
// is where the credential was actually read from.
//
// unverifiable is true when the reason is not a credential spelling but an
// operand the gate cannot resolve (see unboundOperand): a credential refusal
// elsewhere in the clause takes precedence over it.
func secretOperandViolation(clause, root string) (reason string, unverifiable bool) {
	words := shellWords(clause)
	peel := peelCommand(words)
	if peel.reparse != "" {
		return secretOperandViolation(reparseLine(peel.reparse, peel.reparseRest), root)
	}
	// An option the wrapper table does not describe leaves the command behind
	// it unknowable. The clause is then read as before the table existed
	// (readerBehindWrapper from the first wrapper, every later word judged), so
	// a spelled credential is still the hard refusal, and the unknown option is
	// only the unverifiable reason when nothing harder fires.
	commandAt, unknownReason := peel.at, ""
	if peel.unknown != "" {
		commandAt = -1
		unknownReason = fmt.Sprintf("a wrapper is handed an option (`%s`) the gate cannot step over to the command it runs, "+
			"so that command's operands were not judged. Spell the command without the wrapper, or use an option "+
			"the gate knows.", peel.unknown)
	}
	at, judgedFrom := readerBehindWrapper(words, commandAt)
	framing := isBoundedShellCompoundFraming(clause)

	command, gitSub := "", ""
	effectiveGitRoot := root
	if at >= 0 {
		command = filepath.Base(words[at].text)
		if isGitWord(words[at].text) {
			command = "git"
			gitSub = gitSubcommand(words[at+1:])
			if gitRoot, _, escaped := resolveGitClauseWorktree(words, at, root); !escaped && gitRoot != "" {
				effectiveGitRoot = gitRoot
			}
		}
	}

	// Operands the command writes and does not read. cp's destination, tee's
	// files and sed -i's targets change a file without printing it; the write
	// rungs judge them under path.secret, and calling them a read would both
	// misname the refusal and displace the write rule that is the right one.
	argWrites := argumentWriteTargetsInRoot(clause, root)
	written := make(map[string]bool, len(argWrites.targets))
	for _, w := range argWrites.targets {
		written[w] = true
	}
	if command == "git" {
		for _, sib := range argWrites.siblings {
			if sameWorktreePath(sib.root, effectiveGitRoot) {
				for _, w := range sib.writes.targets {
					written[w] = true
				}
			}
		}
	}

	operandRoot := root
	if command == "git" {
		operandRoot = effectiveGitRoot
	}

	// The program a search or text tool runs (`grep 'a:.*'`, `sed 's/:.*//'`,
	// `jq .env`) is not a file it reads; its other operands and option values
	// are. See programOperandsOf.
	program := programOperands{}
	if !framing {
		program = programOperandsOf(command, words, at)
	}
	for _, value := range program.attached {
		if reason := secretOperandReason(value, operandRoot); reason != "" {
			return fmt.Sprintf("`%s` names %s", command, reason), false
		}
	}

	unbound := unknownReason
	for i := 0; i < len(words); i++ {
		text := words[i].text
		if target, isRedirect := redirectionPrefix(text); isRedirect {
			// The operand of the operator is the next word when the operator
			// stands alone, else the rest of this one.
			operand := ""
			stripped := strings.TrimLeft(text, "0123456789&<>|")
			switch {
			case stripped != "":
				operand = stripped
			case target && i+1 < len(words):
				operand = words[i+1].text
				i++
			}
			if strings.Contains(text, "<") && !strings.Contains(text, "<<") && operand != "" {
				// words[i] is the operand's word: the operator's own when the
				// operand is attached, else the one just stepped onto.
				if unbound == "" && unboundOperand(words[i]) {
					unbound = fmt.Sprintf("an input redirection names an operand (`%s`) holding an expansion the gate "+
						"cannot resolve to a value, so the file it reads was not judged. Spell the operand out.", operand)
				}
				if reason := secretOperandReason(operand, root); reason != "" {
					return fmt.Sprintf("an input redirection names %s", reason), false
				}
			}
			continue
		}
		// Every word after the command word as written is judged, whatever
		// program it is handed to: the reader only classifies the command.
		if framing || at < 0 || i <= judgedFrom {
			continue
		}
		reads := !nonReadingCommands[command]
		if command == "git" {
			reads = gitContentSubcommands[gitSub]
		}
		if !reads {
			continue
		}
		if program.exempt[i] {
			if program.literal[i] {
				if reason := secretOperandReasonLiteral(text, operandRoot); reason != "" {
					return fmt.Sprintf("`%s` names %s", command, reason), false
				}
			}
			continue
		}
		if strings.HasPrefix(text, "-") && !strings.Contains(text, "=") && !words[i].bareExpands {
			continue
		}
		if unbound == "" && !written[text] && operandFromFile(command, words[i]) {
			unbound = fmt.Sprintf("`%s` has an operand (`%s`) holding an expansion the gate cannot resolve to a "+
				"value (a variable nothing binds, a parameter operator or a brace expansion), so the file it "+
				"names was not judged. Spell the operand out.", command, text)
		}
		operands := []string{text}
		if _, value, ok := strings.Cut(text, "="); ok {
			// `--file=.env`, `if=.env`: the value is the operand.
			operands = append(operands, value)
		}
		for _, operand := range operands {
			if operand == "" || written[operand] {
				continue
			}
			if carriesSubstitution(operand) {
				if fileReaders[command] {
					return fmt.Sprintf("`%s` has an operand (`%s`) that a command substitution chooses, so "+
						"the file it reads cannot be judged.", command, text), false
				}
				continue
			}
			if reason := secretOperandReason(operand, operandRoot); reason != "" {
				return fmt.Sprintf("`%s` names %s", command, reason), false
			}
		}
	}
	return unbound, unbound != ""
}

// operandFromFile reports whether w is an operand of command that sh expands
// to a value the gate cannot name, for a command whose operands are files it
// reads. A file reader (cat, head, cp …) is refused on any such expansion,
// including a plain unbound variable: its operand is the file and nothing else.
// Every other reading program, and git, keep the owner's 2026-10-24 decision to
// judge a bare unbound variable by its spelling and refuse only the computed
// forms (computedOperand). Programs that are neither — a harness command, eval,
// an unmodelled tool — have their own rungs and are not asked here.
func operandFromFile(command string, w shellWord) bool {
	if fileReaders[command] {
		return unboundOperand(w)
	}
	return computedOperand(w)
}

// readerBehindWrapper classifies the command a wrapper runs. It moves the
// command word past the program wrappers (commandWrappers, the one list the
// harness-only rung reads) to the reading program they run: `nice -n 5 cat $zz`
// hands $zz to cat. A wrapper's own options have an arity the gate does not
// know, so the reader is the first later word that names one; with none, the
// wrapper stays the command word.
//
// reader only CLASSIFIES the command. judgedFrom is the index of the command
// word as written (the first wrapper when argumentCommandWord stopped at a
// wrapper followed by an option, -1 when there is none) and bounds which words
// are judged: every word after it, including the ones between a wrapper and the
// reader (`nice awk 1 .env cat`, `xargs -a .env cat`).
func readerBehindWrapper(words []shellWord, at int) (reader, judgedFrom int) {
	judgedFrom = at
	start := at
	if start < 0 {
		for i, w := range words {
			if !w.quotedHead && commandWrappers[wrapperName(w.text)] {
				start = i
				judgedFrom = i
				break
			}
		}
		if start < 0 {
			return at, at
		}
	}
	for start < len(words) && !words[start].quotedHead && commandWrappers[wrapperName(words[start].text)] {
		next := -1
		for j := start + 1; j < len(words); j++ {
			name := wrapperName(words[j].text)
			if fileReaders[name] || readsOnlyCommands[name] || isGitWord(words[j].text) || commandWrappers[name] {
				next = j
				break
			}
		}
		if next < 0 {
			return at, judgedFrom
		}
		start = next
	}
	return start, judgedFrom
}

func wrapperName(word string) string {
	return strings.TrimSuffix(strings.ToLower(path.Base(word)), ".exe")
}

// gitSubcommand is the first non-option word after `git`, stepping over the
// global options that take a value.
func gitSubcommand(words []shellWord) string {
	if at := gitSubcommandAt(words); at >= 0 {
		return words[at].text
	}
	return ""
}

// secretOperandReason judges one operand, and each piece of a `rev:path` around
// the colon. It returns the clause for the refusal, or "".
func secretOperandReason(operand, root string) string {
	return secretOperandReasonMode(operand, root, true)
}

// secretOperandReasonLiteral is secretOperandReason without wildcard
// expansion: the operand is judged as the path it spells. It is for a word
// that is a program unless options from outside the command line say
// otherwise, where `.*` is a regex but `.env.local` may be a file.
func secretOperandReasonLiteral(operand, root string) string {
	return secretOperandReasonMode(operand, root, false)
}

func secretOperandReasonMode(operand, root string, expandGlobs bool) string {
	candidates := []string{operand}
	if strings.Contains(operand, ":") {
		candidates = append(candidates, strings.Split(operand, ":")...)
	}
	for _, candidate := range candidates {
		candidate = strings.TrimLeft(candidate, "!^")
		if candidate == "" {
			continue
		}
		class := classifyReadOperand(root, candidate)
		if class == ReadSecretPath {
			return fmt.Sprintf("a path that holds credentials (`%s`), whose contents would enter the "+
				"model's context. Read access to credential files is refused for the shell as it is for "+
				"devcouncil_read_file.", candidate)
		}
		if class == ReadOutsideRoot {
			candPath := filepath.FromSlash(normalizeSlashes(candidate))
			if root != "" && !filepath.IsAbs(candPath) {
				candPath = filepath.Join(root, candPath)
			}
			if SecretPathSpelling(candidate) || isOutsideCredentialTarget(candPath) {
				return fmt.Sprintf("a path that holds credentials (`%s`), whose contents would enter the "+
					"model's context. Read access to credential files is refused for the shell as it is for "+
					"devcouncil_read_file.", candidate)
			}
		}
		if expandGlobs && root != "" && strings.ContainsAny(candidate, "*?[") {
			if matched := globMatchesSecret(root, candidate); matched != "" {
				return fmt.Sprintf("a wildcard (`%s`) that matches a path holding credentials (`%s`), "+
					"whose contents would enter the model's context.", candidate, matched)
			}
		}
	}
	return ""
}

func isOutsideCredentialTarget(candPath string) bool {
	slashed := filepath.ToSlash(candPath)
	if SecretPathSpelling(slashed) || SecretPathSpelling(path.Clean(slashed)) {
		return true
	}
	resolved, err := filepath.EvalSymlinks(candPath)
	if err != nil {
		return false
	}
	resolvedSlash := filepath.ToSlash(resolved)
	return SecretPathSpelling(resolvedSlash) || SecretPathSpelling(path.Clean(resolvedSlash))
}

// globMatchesSecret expands a wildcard operand against the tree (or outside
// root when prefixed with `..` or an absolute path) and returns the first match
// that holds credentials. Bounded: a pattern that expands to more than
// maxGlobMatchesJudged is judged on the first that many.
func globMatchesSecret(root, pattern string) string {
	globPat := filepath.FromSlash(normalizeSlashes(pattern))
	if !filepath.IsAbs(globPat) {
		globPat = filepath.Join(root, globPat)
	}
	matches, err := filepath.Glob(globPat)
	if err != nil {
		return ""
	}
	if len(matches) > maxGlobMatchesJudged {
		matches = matches[:maxGlobMatchesJudged]
	}
	for _, m := range matches {
		if _, statErr := os.Lstat(m); statErr != nil {
			continue
		}
		if rel, err := filepath.Rel(root, m); err == nil {
			relSlash := filepath.ToSlash(rel)
			if ClassifyRead(root, relSlash) == ReadSecretPath {
				return relSlash
			}
		}
		if isOutsideCredentialTarget(m) {
			return filepath.ToSlash(m)
		}
	}
	return ""
}
