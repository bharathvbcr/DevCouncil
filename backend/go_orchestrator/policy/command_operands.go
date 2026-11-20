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
	"jq": true, "tr": true, "cmp": true,
}

// inertCommands print, test or set shell state; their operands are words, not
// files, so `echo ../x` names no path. date and hostname are not here: GNU
// `date -f FILE` and `hostname -F FILE` read a file.
var inertCommands = map[string]bool{
	"echo": true, "printf": true, "true": true, "false": true, "test": true, "[": true, "[[": true,
	":": true, "export": true, "unset": true, "set": true, "read": true, "pwd": true, "sleep": true,
	"which": true, "whoami": true, "uname": true, "id": true,
	"seq": true, "expr": true, "basename": true, "dirname": true, "type": true, "alias": true,
	"wait": true, "kill": true, "exit": true, "return": true, "shift": true, "local": true, "declare": true,
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

// numericOptions are the options of well-known programs that REQUIRE a value
// which is a count, a duration or a number of jobs, never a path:
// `make -j${N:-4}`, `go test -count=${C:-1}`, `head -n ${N:-5} README.md`. A value
// an expansion computes for one of them is not a file operand. An option is
// spelled with its dashes; a program not listed here has none. Options that
// take no value or an optional one (`grep --count`, `uniq --count`) are not here:
// the next word after them is an operand.
var numericOptions = map[string]map[string]bool{
	"make":   {"-j": true, "--jobs": true},
	"gmake":  {"-j": true, "--jobs": true},
	"ninja":  {"-j": true},
	"cargo":  {"-j": true, "--jobs": true},
	"head":   {"-n": true, "-c": true, "--lines": true, "--bytes": true},
	"tail":   {"-n": true, "-c": true, "--lines": true, "--bytes": true},
	"pytest": {"-n": true, "--maxfail": true},
	"go": {"-count": true, "-p": true, "-parallel": true, "-cpu": true, "-timeout": true,
		"-benchtime": true, "-shuffle": true, "-fuzztime": true},
}

// gitCountOptions require a value in the subcommands that walk history. They
// are not options of blame (where -n is boolean) or grep.
var gitCountOptions = map[string]map[string]bool{
	"log":         {"-n": true, "--max-count": true, "--skip": true},
	"show":        {"-n": true, "--max-count": true, "--skip": true},
	"whatchanged": {"-n": true, "--max-count": true, "--skip": true},
}

// gitAttachedOnlyOptions take their value only when attached (`-U3`,
// `--unified=3`, `--abbrev=8`): the next word is a revision or pathspec.
var gitAttachedOnlyOptions = map[string]bool{"-U": true, "--unified": true, "--abbrev": true}

// computedOptionValue reports whether words[i] is the value of a numeric option
// of command (and, for git, subcommand sub): the next word after an option that
// requires one (`-n ${N:-5}`), or attached to it (`-j${N:-4}`,
// `--max-count=${N:-5}`, `-U${N:-3}`), and only when the whole expansion is inside
// double quotes (an unquoted one word-splits). Nothing after a bare "--" is a value:
// there every word is an operand. A computed word anywhere else stays a
// computed operand.
func computedOptionValue(command, sub string, words []shellWord, i int) bool {
	if words[i].bareExpands {
		// An unquoted expansion word-splits: `head -n ${x:-1 .env}` is head -n 1 .env.
		return false
	}
	for _, w := range words[:i] {
		if w.text == "--" {
			return false
		}
	}
	required := func(opt string) bool {
		if command == "git" {
			return gitCountOptions[sub][opt]
		}
		return numericOptions[command][opt]
	}
	plain := func(opt string) bool { return !strings.ContainsAny(opt, "$`{~*?[") }
	text := words[i].text
	if i > 0 && !words[i-1].quotedHead && plain(words[i-1].text) && required(words[i-1].text) &&
		!strings.HasPrefix(text, "-") {
		return true
	}
	if name, _, ok := strings.Cut(text, "="); ok && strings.HasPrefix(name, "-") && plain(name) &&
		(required(name) || (command == "git" && (name == "--unified" || name == "--abbrev"))) {
		return true
	}
	if len(text) > 2 && text[0] == '-' && text[1] != '-' && plain(text[:2]) &&
		(required(text[:2]) || (command == "git" && gitAttachedOnlyOptions[text[:2]])) {
		return true
	}
	return false
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
	if isBoundedShellCompoundFraming(clause) {
		return ""
	}
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
		if computedOperand(words[i]) && !computedOptionValue(name, "", words, i) {
			return fmt.Sprintf("`%s` is handed an operand (`%s`) holding an expansion the gate cannot resolve "+
				"to a value (a parameter operator, a special parameter, a brace expansion or ANSI-C quoting), so "+
				"the path it names was not judged and the gate cannot tell a read from a write. Spell the operand out.", name, text)
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
				"gate cannot tell a read from a write. Pass a smaller document, or use a judged program such as "+
				"`jq`, `cat`, `head`, `sed -n`, or the file editing tools instead.", name)
		}
		for _, candidate := range candidates {
			if where := protectedOperand(candidate, root, mt); where != "" {
				return fmt.Sprintf("`%s` is handed %s, and the gate has no model of what it does to a path, "+
					"so it cannot tell a read from a write. Use `cat`, `head`, `jq`, or `sed -n` for reads, "+
					"file editing tools for writes, or run it on paths inside the repository that are not protected.", name, where)
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
	case IsPromptInput(normalized) || lexicalPromptInput(root, operand):
		return fmt.Sprintf("a prompt input, a project instruction, skill or MCP declaration file (`%s`)", operand)
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
