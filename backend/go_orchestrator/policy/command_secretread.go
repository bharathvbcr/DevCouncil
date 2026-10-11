package policy

import (
	"fmt"
	"os"
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
// copy. An operand of theirs that only the shell can resolve (`cat $F`) cannot
// be judged, and is refused rather than guessed at: the rest of the ladder
// treats an unreadable construct the same way.
var fileReaders = map[string]bool{
	"cat": true, "head": true, "tail": true, "less": true, "more": true,
	"tac": true, "nl": true, "base64": true, "xxd": true, "od": true,
	"hexdump": true, "strings": true, "cp": true, "dd": true, "source": true, ".": true,
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
func secretOperandViolation(clause, root string) string {
	words := shellWords(clause)
	at := argumentCommandWord(words)

	command, gitSub := "", ""
	if at >= 0 {
		command = filepath.Base(words[at].text)
		if isGitWord(words[at].text) {
			command = "git"
			gitSub = gitSubcommand(words[at+1:])
		}
	}

	// Operands the command writes and does not read. cp's destination, tee's
	// files and sed -i's targets change a file without printing it; the write
	// rungs judge them under path.secret, and calling them a read would both
	// misname the refusal and displace the write rule that is the right one.
	writtenList := argumentWriteTargets(clause).targets
	written := make(map[string]bool, len(writtenList))
	for _, w := range writtenList {
		written[w] = true
	}

	judged := func(operand string) string {
		return secretOperandReason(operand, root)
	}

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
				if reason := judged(operand); reason != "" {
					return fmt.Sprintf("an input redirection names %s", reason)
				}
			}
			continue
		}
		if at < 0 || i <= at {
			continue
		}
		reads := !nonReadingCommands[command]
		if command == "git" {
			reads = gitContentSubcommands[gitSub]
		}
		if !reads {
			continue
		}
		if strings.HasPrefix(text, "-") && !strings.Contains(text, "=") {
			continue
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
			if hasExpansion(operand) {
				if fileReaders[command] {
					return fmt.Sprintf("`%s` has an operand (`%s`) that only the shell can resolve, so "+
						"the file it reads cannot be judged.", command, text)
				}
				continue
			}
			if reason := judged(operand); reason != "" {
				return fmt.Sprintf("`%s` names %s", command, reason)
			}
		}
	}
	return ""
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
	candidates := []string{operand}
	if strings.Contains(operand, ":") {
		candidates = append(candidates, strings.Split(operand, ":")...)
	}
	for _, candidate := range candidates {
		candidate = strings.TrimLeft(candidate, "!^")
		if candidate == "" {
			continue
		}
		if classifyReadOperand(root, candidate) == ReadSecretPath {
			return fmt.Sprintf("a path that holds credentials (`%s`), whose contents would enter the "+
				"model's context. Read access to credential files is refused for the shell as it is for "+
				"devcouncil_read_file.", candidate)
		}
		if root != "" && strings.ContainsAny(candidate, "*?[") {
			if matched := globMatchesSecret(root, candidate); matched != "" {
				return fmt.Sprintf("a wildcard (`%s`) that matches a path holding credentials (`%s`), "+
					"whose contents would enter the model's context.", candidate, matched)
			}
		}
	}
	return ""
}

// globMatchesSecret expands a wildcard operand against the tree and returns the
// first match that holds credentials. Bounded: a pattern that expands to more
// than maxGlobMatchesJudged is judged on the first that many.
func globMatchesSecret(root, pattern string) string {
	if filepath.IsAbs(pattern) {
		return ""
	}
	matches, err := filepath.Glob(filepath.Join(root, filepath.FromSlash(pattern)))
	if err != nil {
		return ""
	}
	if len(matches) > maxGlobMatchesJudged {
		matches = matches[:maxGlobMatchesJudged]
	}
	for _, m := range matches {
		rel, err := filepath.Rel(root, m)
		if err != nil {
			continue
		}
		if _, statErr := os.Lstat(m); statErr != nil {
			continue
		}
		if ClassifyRead(root, filepath.ToSlash(rel)) == ReadSecretPath {
			return filepath.ToSlash(rel)
		}
	}
	return ""
}
