package policy

import (
	"fmt"
	"path/filepath"
	"strings"
)

// readsOnlyCommands are the programs whose operands are files they only read,
// so a path they name needs no judgement beyond the credential rung's. Reading
// a file outside the tree or inside .git is how an agent orients, and the
// harness does not refuse it. The table is deliberately small: every program
// not on it, and not otherwise modelled, is asked about each operand.
//
// sort -o and uniq's second operand write; commandWrites reads those.
var readsOnlyCommands = map[string]bool{
	"cat": true, "ls": true, "head": true, "tail": true, "rg": true, "grep": true,
	"wc": true, "diff": true, "stat": true, "file": true, "sort": true, "uniq": true, "cut": true,
}

// inertCommands print, test or set shell state; their operands are words, not
// files, so `echo ../x` names no path.
var inertCommands = map[string]bool{
	"echo": true, "printf": true, "true": true, "false": true, "test": true, "[": true, "[[": true,
	":": true, "export": true, "unset": true, "set": true, "read": true, "pwd": true, "sleep": true,
	"date": true, "which": true, "whoami": true, "uname": true, "hostname": true, "id": true,
	"seq": true, "expr": true, "basename": true, "dirname": true, "type": true, "alias": true,
	"wait": true, "exit": true, "return": true, "shift": true, "local": true, "declare": true,
}

// pathWritingCommands are the programs commandWrites reads the files of: the
// write rung judges each file they write, so their operands are not asked
// about twice. git has its own rungs.
var pathWritingCommands = map[string]bool{
	"sed": true, "tee": true, "cp": true, "mv": true, "rm": true, "rmdir": true, "unlink": true,
	"shred": true, "mkdir": true, "chmod": true, "chown": true, "chgrp": true, "curl": true,
	"wget": true, "ruby": true, "perl": true, "truncate": true, "touch": true, "install": true,
	"ln": true, "rsync": true, "patch": true, "dd": true, "xargs": true, "find": true,
}

// unverifiableOperand reports why a clause runs a program the gate does not
// model on a path the hard rules protect, or "" when it does not.
//
// The argument rung names what the programs it knows write. Every other
// program — a compiler, a database shell, a formatter, a tool installed
// yesterday — can write or delete any path it is handed, and the gate cannot
// tell a read from a write there. So each operand is matched against the hard
// rules the write gate applies, and a program handed a path outside the root
// or inside .git, .devcouncil or an agent's settings directory is refused as
// unverifiable. A credential path is refused by secretOperandViolation first.
//
// Operands are the clause's non-option words and the value of `--opt=value`
// and `KEY=value` words. A word carrying an expansion is not judged here (the
// substitution rungs own those), nor is a URL, a stream device or text with
// whitespace in it, which is prose rather than a path.
func unverifiableOperand(clause, root string, mt matching) string {
	words := shellWords(clause)
	at := argumentCommandWord(words)
	if at < 0 || isGitWord(words[at].text) {
		return ""
	}
	name := filepath.Base(words[at].text)
	if pathWritingCommands[name] || readsOnlyCommands[name] || inertCommands[name] || shellReservedPrefixes[name] {
		return ""
	}
	for i := at + 1; i < len(words); i++ {
		text := words[i].text
		if !words[i].quotedHead {
			if operand, isRedirect := redirectionPrefix(text); isRedirect {
				if operand {
					i++
				}
				continue
			}
		}
		candidates := []string{text}
		if strings.HasPrefix(text, "-") {
			candidates = nil
		}
		if _, value, ok := strings.Cut(text, "="); ok {
			candidates = append(candidates, value)
		}
		for _, candidate := range candidates {
			if where := protectedOperand(candidate, root, mt); where != "" {
				return fmt.Sprintf("`%s` is handed %s, and the gate has no model of what it does to a path, "+
					"so it cannot tell a read from a write. Run it on paths inside the repository that are "+
					"not protected, or use a program the gate understands.", name, where)
			}
		}
	}
	return ""
}

// protectedOperand describes why an operand is a path the hard rules refuse to
// let a write reach, or "".
func protectedOperand(operand, root string, mt matching) string {
	if operand == "" || operand == "-" || operand == "--" || hasExpansion(operand) ||
		strings.Contains(operand, "://") || strings.IndexFunc(operand, isSpace) >= 0 ||
		IsStreamDevice(operand) {
		return ""
	}
	// Go's package pattern `./...` is a wildcard, not a path component that
	// ends in dots (which the path rules refuse); judge the directory it hangs
	// from.
	operand = strings.TrimSuffix(strings.TrimSuffix(operand, "..."), "/")
	if operand == "" {
		operand = "."
	}
	normalized, outside := operand, false
	if root != "" {
		normalized, outside = NormalizeRepoPath(root, operand)
	} else {
		outside = classifyReadOperand("", operand) == ReadOutsideRoot
	}
	if outside && normalized == "" && root != "" && namesRoot(root, operand) {
		return "" // the tree itself: `go test .`, `tool /path/to/this/repo`
	}
	switch {
	case outside:
		return fmt.Sprintf("a path outside the repository (`%s`)", operand)
	case matchesRestricted(mt, strings.TrimPrefix(normalizeSlashes(normalized), "./")):
		return fmt.Sprintf("a protected repository path (`%s`)", operand)
	}
	return ""
}

func isSpace(r rune) bool { return r == ' ' || r == '\t' || r == '\n' || r == '\r' }

// namesRoot reports whether an operand resolves to the repository root itself,
// which NormalizeRepoPath answers as "outside" because a write to the root
// directory is never a file write.
func namesRoot(root, operand string) bool {
	rootResolved, ok := resolveExisting(root)
	if !ok {
		return false
	}
	candidate := operand
	if !filepath.IsAbs(candidate) {
		candidate = filepath.Join(rootResolved, candidate)
	}
	resolved, ok := resolveExisting(candidate)
	return ok && resolved == rootResolved
}
