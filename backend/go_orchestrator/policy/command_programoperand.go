package policy

import "strings"

// programTool describes a command whose first operand is the program it runs —
// a regex, a sed script, an awk program, a jq filter — rather than a file it
// reads. `grep -o 'gh:.*'` names no file: `.*` is a regex, and expanding it as
// a wildcard against the tree refused it as a read of `.env.local`.
//
// The tables are what each tool's own manual says (BSD and GNU grep, ugrep,
// ripgrep 15, the_silver_searcher, BSD and GNU sed, BSD awk, jq 1.7, git grep).
// Where the implementations disagree about whether an option takes a value —
// grep -C, sed -i/-I/-l, ag -A/-B/-C — it is listed as a flag, because that
// reading judges more words: a value-taking option misread as a flag leaves its
// value to be taken for the program (an over-refusal), while a flag misread as
// value-taking would make the next word, possibly a file, the program.
type programTool struct {
	// shortValue are the option letters that take a value; shortFlag the ones
	// that do not.
	shortValue, shortFlag string
	// longArity maps a long option to how many words follow it.
	longArity map[string]int
	// longFlag are long options with no value. `--no-NAME` is known when NAME
	// is a known option.
	longFlag []string
	// programOpts supply the program as their value (`-e PATTERN`), and
	// programFileOpts read it from a file (`-f FILE`): with either present
	// there is no program operand, and every operand is a file.
	programOpts, programFileOpts []string
	// noProgramOpts run without a program at all (`rg --files`).
	noProgramOpts []string
}

func arity1(names ...string) map[string]int {
	m := make(map[string]int, len(names))
	for _, n := range names {
		m[n] = 1
	}
	return m
}

var grepTool = programTool{
	shortValue: "ABDdefm",
	shortFlag:  "abcCEFGHhIiJLlMnOopqRrSsTUuVvwXxyZzP0123456789",
	longArity: arity1("after-context", "before-context", "devices", "directories", "regexp", "file",
		"max-count", "exclude", "include", "exclude-dir", "include-dir", "exclude-from"),
	longFlag: []string{"extended-regexp", "fixed-strings", "basic-regexp", "perl-regexp", "ignore-case",
		"word-regexp", "line-regexp", "count", "color", "colour", "files-with-matches",
		"files-without-match", "only-matching", "quiet", "silent", "messages", "byte-offset",
		"with-filename", "filename", "line-number", "initial-tab", "null", "null-data", "text",
		"recursive", "dereference-recursive", "invert-match", "version", "help", "line-buffered",
		"binary", "unix-byte-offsets", "mmap", "bz2decompress", "decompress", "lzma", "xz"},
	programOpts:     []string{"-e", "--regexp"},
	programFileOpts: []string{"-f", "--file"},
}

var programTools = map[string]programTool{
	"grep":  grepTool,
	"egrep": grepTool,
	"fgrep": grepTool,
	"rg": {
		shortValue: "ABCEMTdefgjmrt",
		shortFlag:  "zsFivxUPSawL.ubhnN0opqHIclV",
		longArity: arity1("regexp", "file", "pre", "pre-glob", "dfa-size-limit", "encoding", "engine",
			"max-count", "regex-size-limit", "threads", "glob", "iglob", "ignore-file", "max-depth",
			"max-filesize", "type", "type-not", "type-add", "type-clear", "after-context",
			"before-context", "color", "colors", "context", "context-separator",
			"field-context-separator", "field-match-separator", "hostname-bin", "hyperlink-format",
			"max-columns", "path-separator", "replace", "sort", "sortr", "generate"),
		longFlag: []string{"search-zip", "case-sensitive", "crlf", "fixed-strings", "ignore-case",
			"invert-match", "line-regexp", "mmap", "multiline", "multiline-dotall", "unicode",
			"null-data", "pcre2", "smart-case", "stop-on-nonmatch", "text", "word-regexp",
			"auto-hybrid-regex", "pcre2-unicode", "binary", "follow", "glob-case-insensitive",
			"hidden", "ignore-file-case-insensitive", "ignore", "ignore-dot", "ignore-exclude",
			"ignore-files", "ignore-global", "ignore-parent", "ignore-vcs", "require-git",
			"one-file-system", "unrestricted", "block-buffered", "byte-offset", "column", "heading",
			"help", "include-zero", "line-buffered", "line-number", "max-columns-preview", "null",
			"only-matching", "passthru", "pretty", "quiet", "trim", "vimgrep", "with-filename",
			"filename", "sort-files", "count", "count-matches", "files-with-matches",
			"files-without-match", "json", "debug", "ignore-messages", "messages", "stats", "trace",
			"files", "config", "pcre2-version", "type-list", "version"},
		programOpts:     []string{"-e", "--regexp"},
		programFileOpts: []string{"-f", "--file"},
		noProgramOpts:   []string{"--files", "--type-list", "--generate", "--pcre2-version"},
	},
	"ag": {
		shortValue: "GWgmp",
		shortFlag:  "aABCcDfFHilLnoQrsStuUvVwz0",
		longArity: arity1("file-search-regex", "max-count", "path-to-ignore", "depth", "ignore",
			"ignore-dir", "pager", "workers", "width"),
		longFlag: []string{"ackmate", "affinity", "noaffinity", "all-types", "after", "before", "break",
			"nobreak", "count", "color", "nocolor", "column", "context", "debug", "filename",
			"nofilename", "follow", "nofollow", "fixed-strings", "group", "nogroup", "heading",
			"noheading", "hidden", "ignore-case", "files-with-matches", "files-without-matches",
			"list-file-types", "mmap", "nommap", "multiline", "nomultiline", "norecurse", "numbers",
			"nonumbers", "only-matching", "one-device", "parallel", "print-long-lines", "passthrough",
			"passthru", "literal", "recurse", "case-sensitive", "smart-case", "search-binary",
			"silent", "stats", "stats-only", "all-text", "unrestricted", "skip-vcs-ignores",
			"invert-match", "version", "vimgrep", "word-regexp", "search-zip", "null", "print0",
			"nopager"},
		programOpts:   []string{"-g"},
		noProgramOpts: []string{"--list-file-types"},
	},
	"sed": {
		shortValue: "ef",
		shortFlag:  "EHalnruiIsz",
		longArity:  arity1("expression", "file", "line-length"),
		longFlag: []string{"quiet", "silent", "regexp-extended", "separate", "unbuffered", "null-data",
			"debug", "posix", "sandbox", "follow-symlinks", "in-place", "version", "help", "binary"},
		programOpts:     []string{"-e", "--expression"},
		programFileOpts: []string{"-f", "--file"},
	},
	"awk": {
		shortValue:      "Ffv",
		programFileOpts: []string{"-f"},
	},
	"jq": {
		shortValue: "fL",
		shortFlag:  "nRscrjaSCMehV",
		longArity: map[string]int{"from-file": 1, "indent": 1, "arg": 2, "argjson": 2,
			"slurpfile": 2, "rawfile": 2},
		longFlag: []string{"null-input", "raw-input", "slurp", "compact-output", "raw-output",
			"raw-output0", "join-output", "ascii-output", "sort-keys", "color-output",
			"monochrome-output", "tab", "unbuffered", "stream", "stream-errors", "seq", "exit-status",
			"version", "build-configuration", "help", "args", "jsonargs"},
		programFileOpts: []string{"-f", "--from-file"},
	},
}

// gitGrepTool is `git grep`: its operands after the pattern are revisions
// and, after `--`, paths.
var gitGrepTool = programTool{
	shortValue: "ABCefm",
	shortFlag:  "viwarEGFPnhHlLzocpWqOI",
	longArity: arity1("max-depth", "context", "before-context", "after-context", "threads",
		"max-count"),
	longFlag: []string{"cached", "index", "untracked", "exclude-standard", "recurse-submodules",
		"invert-match", "ignore-case", "word-regexp", "text", "textconv", "recursive",
		"extended-regexp", "basic-regexp", "fixed-strings", "perl-regexp", "line-number", "column",
		"full-name", "files-with-matches", "name-only", "files-without-match", "null",
		"only-matching", "count", "color", "break", "heading", "show-function", "function-context",
		"quiet", "all-match", "open-files-in-pager", "ext-grep", "and", "or", "not"},
	programOpts:     []string{"-e"},
	programFileOpts: []string{"-f"},
}

// knows reports whether an option occurrence is one the tool's table names,
// as a flag or with the arity it was parsed with.
func (t programTool) knows(o optOccurrence) bool {
	if long, ok := strings.CutPrefix(o.name, "--"); ok {
		known := func(n string) bool { return t.longArity[n] > 0 || containsString(t.longFlag, n) }
		if known(long) {
			return true
		}
		// A value after `=` on a flag (`--color=never`) is fine; `--no-NAME`
		// negates a known option.
		if neg, ok := strings.CutPrefix(long, "no-"); ok && known(neg) {
			return true
		}
		return false
	}
	letter := o.name[1:]
	return strings.Contains(t.shortValue, letter) || strings.Contains(t.shortFlag, letter)
}

// programOperands is what programOperandsOf finds in one clause.
type programOperands struct {
	// exempt are indexes into the clause's words that hold the program, which
	// is not a file the command reads.
	exempt map[int]bool
	// attached are option values written in the same word as a short option
	// (`-f.env`, `-g.env*`), other than the program's own. The operand loop
	// passes over option words, so these are judged separately.
	attached []string
	// literal are exempt words that are still judged as a path, only without
	// wildcard expansion. They are first operands of a tool that can take
	// options from somewhere other than the command line (rg's config file,
	// GREP_OPTIONS), so the gate cannot know the word is the program: a
	// config line `-e TOKEN` turns `rg .env.local` into a read of .env.local.
	// A regex is never expanded against the tree, which is what refused
	// `grep -o 'gh:.*'`, but a word that spells a credential path is.
	literal map[int]bool
}

// offLineOptions are the tools that read options from outside the command
// line, and the option that turns that off (`""` when nothing does). rg reads
// the file named by RIPGREP_CONFIG_PATH unless --no-config is given; BSD grep
// prepends GREP_OPTIONS. Where the variable comes from (an assignment, an
// export, the profile) is not visible to the gate, so it is not looked for.
var offLineOptions = map[string]string{
	"rg":    "--no-config",
	"grep":  "",
	"egrep": "",
	"fgrep": "",
}

// programOpensFiles are the tools whose program can itself open a file: sed's
// `r`, `R`, `w` and `e` commands, awk's `getline <` and redirections. Every
// program word of theirs, the first operand and each -e value, is judged as a
// literal path, which is how main refused `sed 'r ./.env.local' README.md`.
// A name the program builds at run time cannot be judged from the text, here
// or on main.
var programOpensFiles = map[string]bool{"sed": true, "awk": true}

// programOperandsOf finds the program words of the command at words[at] — the
// first operand, or the values of -e/--regexp — for the tools in programTools
// and `git grep`. Every other word stays an operand to judge, option values
// included, so `rg x -g '.env*'` and `grep -f .env x` are still reads.
//
// It fails closed:
//   - An option the tool's table does not know, anywhere in the clause, exempts
//     nothing. It may take a value, and GNU tools permute arguments and accept
//     abbreviations, so `grep .env.local --reg TOKEN` makes .env.local a file.
//   - A word the shell rewrites (an unquoted `.*` or `.env*`) is never
//     exempt: the shell expands it into file names before the tool runs.
//   - For a tool in offLineOptions, the first operand is exempt from wildcard
//     expansion only, and still judged as a literal path (see literal).
func programOperandsOf(command string, words []shellWord, at int) programOperands {
	out := programOperands{exempt: map[int]bool{}, literal: map[int]bool{}}
	if at < 0 {
		return out
	}
	tool, ok := programTools[command]
	start := at + 1
	if command == "git" {
		sub := gitSubcommandAt(words[at+1:])
		if sub < 0 || words[at+1+sub].text != "grep" {
			return out
		}
		tool, ok, start = gitGrepTool, true, at+1+sub+1
	}
	if !ok {
		return out
	}

	// The tool's own arguments, without redirections, and where each stands
	// in words.
	var args []shellWord
	var index []int
	for k := start; k < len(words); k++ {
		if !words[k].quotedHead {
			if operand, isRedirect := redirectionPrefix(words[k].text); isRedirect {
				if operand {
					k++
				}
				continue
			}
		}
		args = append(args, words[k])
		index = append(index, k)
	}
	p := parseOptArgsArity(args, tool.shortValue, tool.longArity)

	// An option the table does not know, anywhere, exempts nothing.
	limit := len(args)
	for _, o := range p.seen {
		if !tool.knows(o) {
			limit = 0
			break
		}
	}
	exempt := func(argAt int) bool {
		if argAt < 0 || argAt >= limit || args[argAt].expands {
			return false
		}
		out.exempt[index[argAt]] = true
		if programOpensFiles[command] {
			out.literal[index[argAt]] = true
		}
		return true
	}

	program := func(o optOccurrence) bool { return containsString(tool.programOpts, o.name) }
	for _, o := range p.seen {
		if o.valueAt == o.at && !strings.HasPrefix(o.name, "--") && !program(o) && o.value != "" {
			out.attached = append(out.attached, o.value)
		}
	}
	if p.has(tool.noProgramOpts...) {
		return out
	}
	given := false
	for _, o := range p.seen {
		if program(o) || containsString(tool.programFileOpts, o.name) {
			given = true
		}
		if program(o) {
			exempt(o.valueAt)
		}
	}
	if !given && len(p.operandAt) > 0 && exempt(p.operandAt[0]) {
		if off, reads := offLineOptions[command]; reads && (off == "" || !p.has(off)) {
			out.literal[index[p.operandAt[0]]] = true
		}
	}
	return out
}
