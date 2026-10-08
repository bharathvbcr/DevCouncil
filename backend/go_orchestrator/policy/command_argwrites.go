package policy

import (
	"path"
	"strings"
)

// argumentWriteTargets returns the files the commands in line write through
// their arguments, and whether any of those files could not be named.
//
// line is the masked text redirectTargets builds: substitutions, output
// redirections and subshell parentheses are already gone, so each clause is the
// command's own words. Clauses are split by SplitCommandChain and words by
// shellWords — the ladder's own splitters, not a second reading of the grammar.
//
// The commands are the ones that write a file named in their arguments as
// their ordinary purpose:
//
//   - sed with -i/-I/--in-place in any spelling writes every file operand, and
//     a backup beside each when a suffix is given. Without in-place it writes
//     nothing. The script (the first operand, unless -e/-f supplied it) is not
//     a file.
//   - tee writes every file operand.
//   - cp writes its destination — or DIR/basename(source) for each source when
//     the destination is a directory: named by -t/--target-directory, written
//     with a trailing slash, or implied by more than one source.
//   - mv writes the same paths as cp, and changes every source too: it removes
//     it.
//
// A destination given without a trailing slash is judged as the path written,
// even though an existing directory there would receive DIR/basename instead;
// the filesystem is not consulted. An operand only the shell can resolve
// reports opacity, as an unresolvable redirection target does. A glob is
// judged as the literal path, as a glob in a redirection target is.
func argumentWriteTargets(line string) ([]string, bool) {
	var targets []string
	opaque := false
	for _, clause := range SplitCommandChain(line) {
		words := shellWords(clause)
		at := argumentCommandWord(words)
		if at < 0 {
			continue
		}
		var args []shellWord
		for k := at + 1; k < len(words); k++ {
			// Input redirections are still in the text (only output ones were
			// masked); `tee < in f` reads in and writes f.
			if !words[k].quotedHead {
				if operand, isRedirect := redirectionPrefix(words[k].text); isRedirect {
					if operand {
						k++
					}
					continue
				}
			}
			args = append(args, words[k])
		}
		var written []string
		var unresolved bool
		switch path.Base(words[at].text) {
		case "sed":
			written, unresolved = sedWrites(args)
		case "tee":
			written, unresolved = teeWrites(args)
		case "cp":
			written, unresolved = copyWrites(args, false)
		case "mv":
			written, unresolved = copyWrites(args, true)
		}
		targets = append(targets, written...)
		opaque = opaque || unresolved
	}
	return targets, opaque
}

// argumentWrappers run the command named by the following words. Stepped over
// when looking for the command word, like command/builtin in
// reparsingCommandWord. A wrapper followed by an option stops the search: its
// options may take arguments, and guessing which word is the command would
// name a write that does not happen or miss one that does.
var argumentWrappers = map[string]bool{
	"command": true, "builtin": true, "exec": true, "env": true,
	"sudo": true, "nohup": true, "time": true,
}

// shellReservedPrefixes are the reserved words that may stand before a simple
// command in a clause the chain splitter produced: `if tee f; then …`,
// `then tee f`, `{ tee f; }`, `! tee f`.
var shellReservedPrefixes = map[string]bool{
	"if": true, "then": true, "else": true, "elif": true, "do": true,
	"while": true, "until": true, "!": true, "{": true,
}

// argumentCommandWord returns the index of the word sh would run in one
// clause, or -1. Assignments and redirections before it are stepped over as in
// reparsingCommandWord; so are reserved words and argumentWrappers.
func argumentCommandWord(words []shellWord) int {
	for i := 0; i < len(words); i++ {
		word := words[i]
		if word.quotedHead {
			return i
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
		if argumentWrappers[word.text] {
			if i+1 < len(words) && strings.HasPrefix(words[i+1].text, "-") {
				return -1
			}
			continue
		}
		return i
	}
	return -1
}

// sedWrites reads a sed argument list the way GNU sed does — options may
// follow operands, and -i takes its suffix only attached — with one
// concession to BSD sed, where -i always takes a suffix: an empty word or one
// starting with "." after a bare -i is that suffix (macOS writes an empty
// quoted suffix to mean "no backup", and `sed -i .bak …` to keep one). No sed script begins with ".", and an empty script is no
// script, so neither reading can mistake a GNU script for it.
func sedWrites(args []shellWord) ([]string, bool) {
	inPlace, haveScript, endOfOptions := false, false, false
	suffix := ""
	var operands []string
	for k := 0; k < len(args); k++ {
		w := args[k].text
		if endOfOptions || w == "-" || !strings.HasPrefix(w, "-") {
			operands = append(operands, w)
			continue
		}
		if w == "--" {
			endOfOptions = true
			continue
		}
		if long, ok := strings.CutPrefix(w, "--"); ok {
			name, value, hasValue := strings.Cut(long, "=")
			switch name {
			case "in-place":
				inPlace = true
				if hasValue {
					suffix = value
				}
			case "expression", "file":
				haveScript = true
				if !hasValue {
					k++
				}
			case "line-length":
				if !hasValue {
					k++
				}
			}
			continue
		}
		cluster := w[1:]
		for c := 0; c < len(cluster); c++ {
			switch cluster[c] {
			case 'i', 'I':
				inPlace = true
				if rest := cluster[c+1:]; rest != "" {
					suffix = rest
				} else if k+1 < len(args) && (args[k+1].text == "" || strings.HasPrefix(args[k+1].text, ".")) {
					suffix = args[k+1].text
					k++
				}
				c = len(cluster)
			case 'e', 'f', 'l':
				// The option's argument is the rest of the cluster, or the
				// next word when the cluster ends here.
				if cluster[c] != 'l' {
					haveScript = true
				}
				if c+1 == len(cluster) {
					k++
				}
				c = len(cluster)
			}
		}
	}
	if !inPlace {
		return nil, false
	}
	if !haveScript && len(operands) > 0 {
		operands = operands[1:]
	}
	var targets []string
	opaque := suffix != "" && unresolvableWord(suffix)
	for _, file := range operands {
		if file == "-" {
			continue // standard input, which sed refuses to edit in place
		}
		if unresolvableWord(file) {
			opaque = true
			continue
		}
		targets = append(targets, file)
		if suffix != "" && !opaque {
			targets = append(targets, sedBackupName(file, suffix))
		}
	}
	return targets, opaque
}

// sedBackupName is the file sed -i keeps the original in: the name with the
// suffix appended, or — when the suffix holds `*` — the suffix with each `*`
// replaced by the file's base name, in the file's directory.
func sedBackupName(file, suffix string) string {
	dir, base := path.Split(file)
	if !strings.Contains(suffix, "*") {
		return file + suffix
	}
	return dir + strings.ReplaceAll(suffix, "*", base)
}

// teeWrites returns every file operand of tee. Its options take no arguments
// except --output-error, whose mode is attached with =.
func teeWrites(args []shellWord) ([]string, bool) {
	var targets []string
	opaque, endOfOptions := false, false
	for _, arg := range args {
		w := arg.text
		if !endOfOptions && w == "--" {
			endOfOptions = true
			continue
		}
		if !endOfOptions && len(w) > 1 && strings.HasPrefix(w, "-") {
			continue
		}
		if w == "-" {
			continue // standard output, not a file
		}
		if unresolvableWord(w) {
			opaque = true
			continue
		}
		targets = append(targets, w)
	}
	return targets, opaque
}

// copyWrites returns the paths cp (or, with move, mv) writes. Options that take
// a separate argument are -t/--target-directory and -S/--suffix; the rest are
// flags or carry their value after =.
func copyWrites(args []shellWord, move bool) ([]string, bool) {
	targetDir, haveTargetDir := "", false
	noTargetDir, endOfOptions := false, false
	var operands []string
	for k := 0; k < len(args); k++ {
		w := args[k].text
		if endOfOptions || w == "-" || !strings.HasPrefix(w, "-") {
			operands = append(operands, w)
			continue
		}
		if w == "--" {
			endOfOptions = true
			continue
		}
		if long, ok := strings.CutPrefix(w, "--"); ok {
			name, value, hasValue := strings.Cut(long, "=")
			switch name {
			case "target-directory":
				if !hasValue {
					if k+1 >= len(args) {
						return nil, false // cp/mv refuse a missing argument and write nothing
					}
					k++
					value = args[k].text
				}
				haveTargetDir, targetDir = true, value
			case "suffix":
				if !hasValue {
					k++
				}
			case "no-target-directory":
				noTargetDir = true
			}
			continue
		}
		cluster := w[1:]
		for c := 0; c < len(cluster); c++ {
			switch cluster[c] {
			case 'T':
				noTargetDir = true
			case 't', 'S':
				value := cluster[c+1:]
				if value == "" {
					if k+1 >= len(args) {
						return nil, false // cp/mv refuse a missing argument and write nothing
					}
					k++
					value = args[k].text
				}
				if cluster[c] == 't' {
					haveTargetDir, targetDir = true, value
				}
				c = len(cluster)
			}
		}
	}

	var sources []string
	dir := ""
	switch {
	case haveTargetDir:
		sources, dir = operands, targetDir
	case len(operands) < 2:
		return nil, false // nothing to copy, or nowhere to copy it
	default:
		sources = operands[:len(operands)-1]
		dest := operands[len(operands)-1]
		if noTargetDir || (len(sources) == 1 && !strings.HasSuffix(dest, "/")) {
			if unresolvableWord(dest) {
				return nil, true
			}
			targets := []string{dest}
			if move {
				return appendResolvable(targets, sources)
			}
			return targets, false
		}
		dir = dest
	}
	if unresolvableWord(dir) {
		return nil, true
	}
	var targets []string
	opaque := false
	for _, source := range sources {
		base := path.Base(source)
		if unresolvableWord(base) {
			opaque = true
			continue
		}
		targets = append(targets, strings.TrimRight(dir, "/")+"/"+base)
	}
	if move {
		var moveOpaque bool
		targets, moveOpaque = appendResolvable(targets, sources)
		opaque = opaque || moveOpaque
	}
	return targets, opaque
}

// appendResolvable appends each path that can be named and reports whether
// any could not.
func appendResolvable(targets, paths []string) ([]string, bool) {
	opaque := false
	for _, p := range paths {
		if unresolvableWord(p) {
			opaque = true
			continue
		}
		targets = append(targets, p)
	}
	return targets, opaque
}
