package policy

import (
	"encoding/json"
	"fmt"
	"path/filepath"
	"sort"
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
// and `KEY=value` words, and every string a JSON object or array among them
// carries (see jsonOperandCandidates). A word carrying an expansion is not
// judged here (the substitution rungs own those), nor is a URL, a stream
// device or text with whitespace in it, which is prose rather than a path.
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
		candidates, complete := jsonOperandCandidates(candidates)
		if !complete {
			return fmt.Sprintf("`%s` is handed an operand that opens like a JSON document and is larger or "+
				"more deeply nested than the gate examines, so the paths it may carry were not judged and the "+
				"gate cannot tell a read from a write. Pass a smaller document, or use a program the gate "+
				"understands.", name)
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

// The bounds on a JSON operand the gate opens. Past any of them the operand is
// refused as unverifiable rather than judged by the part that was read: a
// document the gate stopped reading is not a document it found clean.
const (
	maxJSONOperandBytes   = 64 << 10
	maxJSONOperandStrings = 4096
	maxJSONOperandDepth   = 64
)

// jsonOperandCandidates adds to a word's candidates every string a JSON object
// or array among them carries — keys and values at any depth, the value of a
// `--opt=value` string, and the strings of a JSON document encoded inside a
// string. A program handed `{"command":"/opt/x"}` is handed /opt/x as surely
// as one handed /opt/x itself; as a single word the document reads as a
// relative file name inside the root, and pretty-printed it has whitespace and
// reads as prose. Each string keeps the prose exemption, so a note in a JSON
// field is still a note. complete is false when a document is past the bounds.
func jsonOperandCandidates(candidates []string) (out []string, complete bool) {
	queue := append([]string(nil), candidates...)
	for len(queue) > 0 {
		candidate := queue[0]
		queue = queue[1:]
		out = append(out, candidate)
		strs, isJSON, ok := jsonOperandStrings(candidate)
		if !ok {
			return out, false
		}
		if !isJSON {
			continue
		}
		for _, s := range strs {
			queue = append(queue, s)
			if _, value, found := strings.Cut(s, "="); found {
				queue = append(queue, value)
			}
		}
		if len(out)+len(queue) > maxJSONOperandStrings {
			return out, false
		}
	}
	return out, true
}

// jsonOperandStrings returns the strings of a JSON object or array, keys in
// sorted order so the path a refusal names does not depend on map order.
// isJSON is false for anything else, which is judged as the word it is. ok is
// false when the document is past the bounds.
func jsonOperandStrings(text string) (strs []string, isJSON, ok bool) {
	trimmed := strings.TrimSpace(text)
	if trimmed == "" || (trimmed[0] != '{' && trimmed[0] != '[') {
		return nil, false, true
	}
	// Bounded before it is read at all: an operand this large that opens like
	// a document is refused whether or not it would have parsed.
	if len(trimmed) > maxJSONOperandBytes {
		return nil, true, false
	}
	if !json.Valid([]byte(trimmed)) {
		return nil, false, true
	}
	var doc any
	if err := json.Unmarshal([]byte(trimmed), &doc); err != nil {
		return nil, false, true
	}
	if !collectJSONStrings(doc, 0, &strs) {
		return nil, true, false
	}
	return strs, true, true
}

func collectJSONStrings(v any, depth int, out *[]string) bool {
	if depth > maxJSONOperandDepth {
		return false
	}
	switch v := v.(type) {
	case string:
		if len(*out) >= maxJSONOperandStrings {
			return false
		}
		*out = append(*out, v)
	case []any:
		for _, item := range v {
			if !collectJSONStrings(item, depth+1, out) {
				return false
			}
		}
	case map[string]any:
		keys := make([]string, 0, len(v))
		for key := range v {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		for _, key := range keys {
			if !collectJSONStrings(key, depth+1, out) || !collectJSONStrings(v[key], depth+1, out) {
				return false
			}
		}
	}
	return true
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
	if outside && root != "" && tempWrite(root, normalized) {
		// A temporary directory takes writes (see tempWrite), so whether the
		// program reads or writes it no longer matters: `go build -o /tmp/x`.
		return ""
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
