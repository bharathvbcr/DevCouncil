package policy

import (
	"os"
	"strings"
)

// maxBindingVariants bounds the readings of one line expandBindings returns,
// and maxBindingValues the values one name collects. Each reading is judged in
// full, so together they bound how much more work a line with loops costs the
// gate than the same line without them.
const (
	maxBindingVariants = 16
	maxBindingValues   = 16
)

// expandBindings returns the lines the ladder judges in place of command:
// command with every variable it can name rewritten to its value.
//
// What it can name: a leading tilde, $HOME and $TMPDIR, from this process's
// environment, which the shell running the command inherits; and every
// variable the line itself gives a literal value — `f=README.md; head $f`,
// `export W=/tmp/x`, and each item of a `for` loop's list, every one of which
// is a reading of its own. A name assigned more than once collects every value
// and each is judged, so the order the line assigns and uses it in can add a
// reading but never hide one. Text sh does not expand — single quotes, $'…',
// an escaped `$` — is left alone.
//
// It exists because the operand rungs used to refuse any word with a `$` or a
// leading `~` in it, which refused the loops and scratch variables agents
// write on almost every task, and `cat ~/x` while `ls ~/x` passed. A variable
// it cannot bind is left in the text and judged by its spelling
// (carriesSubstitution); a reading past maxBindingVariants is not produced, and
// leaves its names unbound in the same way — no worse than a line that never
// assigned them.
//
// A rendering longer than maxCommandBytes is replaced by the line as written,
// so a long value repeated many times cannot make the gate judge more text than
// the length rung allows.
func expandBindings(command string) []string {
	if !strings.ContainsAny(command, "$~") {
		return []string{command}
	}
	values := lineBindings(command)
	segments, refs := bindingTemplate(command, values)
	if refs == 0 {
		return []string{command}
	}
	return renderBindings(command, segments, values)
}

// bindingSegment is one piece of a line's template: literal text, or a
// reference to a bound name.
type bindingSegment struct {
	text string
	name string
}

// lineBindings collects the values each name can take in command: the
// well-known environment, then every pure assignment clause and `for` list.
func lineBindings(command string) map[string][]string {
	values := map[string][]string{}
	home, _ := os.UserHomeDir()
	add := func(name, value string) {
		if home != "" && (value == "~" || strings.HasPrefix(value, "~/")) {
			value = home + value[1:]
		}
		if !literalBindingValue(value) || len(values[name]) >= maxBindingValues {
			return
		}
		for _, have := range values[name] {
			if have == value {
				return
			}
		}
		values[name] = append(values[name], value)
	}
	if home != "" {
		add("HOME", home)
	}
	if tmp := os.Getenv("TMPDIR"); tmp != "" {
		add("TMPDIR", tmp)
	}
	for _, clause := range SplitCommandChain(command) {
		words := shellWords(clause)
		// The splitter breaks at `;`, so a nested loop's `for` arrives behind
		// the `do` that opened the outer one.
		for len(words) > 0 && compoundKeywords[words[0].text] {
			words = words[1:]
		}
		if len(words) == 0 {
			continue
		}
		if words[0].text == "for" && len(words) >= 3 && words[2].text == "in" && bindingName(words[1].text) {
			for _, w := range words[3:] {
				if w.text == "do" {
					break
				}
				add(words[1].text, w.text)
			}
			continue
		}
		start := 0
		switch words[0].text {
		case "export", "local", "readonly", "declare", "typeset":
			start = 1
		}
		pure := start < len(words)
		for _, w := range words[start:] {
			if !isAssignmentWord(w.text) && !(start > 0 && strings.HasPrefix(w.text, "-")) {
				pure = false
				break
			}
		}
		if !pure {
			// `FOO=x cmd $FOO` sets FOO for cmd's environment only; sh has
			// already expanded $FOO by then, so the assignment binds nothing.
			continue
		}
		for _, w := range words[start:] {
			if name, value, ok := strings.Cut(w.text, "="); ok && isAssignmentWord(w.text) {
				add(name, value)
			}
		}
	}
	values["~"] = values["HOME"]
	return values
}

// compoundKeywords open a list inside a compound command, ahead of the clause
// that list starts with.
var compoundKeywords = map[string]bool{"do": true, "then": true, "else": true, "{": true, "!": true}

// literalBindingValue reports whether a value can be written into the line in
// place of a reference without changing what else the line means: it holds no
// expansion, quote, escape or operator of its own.
func literalBindingValue(value string) bool {
	return !strings.ContainsAny(value, "$`\\\"'\n\r;|&<>()")
}

// bindingName reports whether s is a shell variable name.
func bindingName(s string) bool {
	if s == "" {
		return false
	}
	for i, r := range s {
		if r == '_' || (r >= 'A' && r <= 'Z') || (r >= 'a' && r <= 'z') || (i > 0 && r >= '0' && r <= '9') {
			continue
		}
		return false
	}
	return true
}

// bindingTemplate splits command into literal text and references to the
// names values binds, and counts the references. Quoting is read through
// shellQuoteStep, so what sh would not expand is never a reference.
func bindingTemplate(command string, values map[string][]string) ([]bindingSegment, int) {
	runes := []rune(command)
	n := len(runes)
	var segments []bindingSegment
	var lit strings.Builder
	refs := 0
	reference := func(name string) {
		if lit.Len() > 0 {
			segments = append(segments, bindingSegment{text: lit.String()})
			lit.Reset()
		}
		segments = append(segments, bindingSegment{name: name})
		refs++
	}
	quote := rune(0)
	wordStart := true
	// A $( … ) span is a command line of its own, quoted afresh: in
	// `echo "$(git diff > ~/x)"` the tilde is unquoted to the shell that runs
	// git, though the span sits inside double quotes. Each open paren pushes
	// the quote state it interrupted — a span's, to restore at its close, or
	// none for a plain subshell — so the two spellings of one write read alike.
	var opened []rune
	const plainParen = rune(-1)
	for i := 0; i < n; {
		r := runes[i]
		if quote == 0 && r == '~' && wordStart && len(values["~"]) > 0 &&
			(i+1 == n || strings.ContainsRune("/ \t\n;|&)", runes[i+1])) {
			reference("~")
			i++
			wordStart = false
			continue
		}
		if next, nq, handled := shellQuoteStep(runes, i, quote); handled {
			lit.WriteString(string(runes[i:next]))
			quote = nq
			i = next
			wordStart = false
			continue
		}
		if r == '$' && i+1 < n && runes[i+1] == '(' {
			opened = append(opened, quote)
			quote = 0
			lit.WriteString("$(")
			i += 2
			wordStart = true
			continue
		}
		if r == '$' {
			if name, width := parameterReference(runes[i+1:]); width > 0 && len(values[name]) > 0 {
				reference(name)
				i += 1 + width
				wordStart = false
				continue
			}
		}
		if quote == 0 && r == '(' {
			opened = append(opened, plainParen)
		}
		if quote == 0 && r == ')' && len(opened) > 0 {
			if top := opened[len(opened)-1]; top != plainParen {
				quote = top
			}
			opened = opened[:len(opened)-1]
		}
		lit.WriteRune(r)
		wordStart = quote == 0 && strings.ContainsRune(" \t\n;|&(=", r)
		i++
	}
	if lit.Len() > 0 {
		segments = append(segments, bindingSegment{text: lit.String()})
	}
	return segments, refs
}

// parameterReference reads the name after a `$`: `NAME` or `${NAME}`. It
// returns the name and how many runes it spans, or a zero width for anything
// else — a positional or special parameter, or a `${…}` with an operator in it.
func parameterReference(rs []rune) (string, int) {
	if len(rs) > 0 && rs[0] == '{' {
		end := -1
		for j := 1; j < len(rs); j++ {
			if rs[j] == '}' {
				end = j
				break
			}
		}
		if end > 1 && bindingName(string(rs[1:end])) {
			return string(rs[1:end]), end + 1
		}
		return "", 0
	}
	j := 0
	for j < len(rs) && bindingName(string(rs[:j+1])) {
		j++
	}
	return string(rs[:j]), j
}

// renderBindings writes every reading of the template, up to
// maxBindingVariants, each name taking one of its values per reading.
func renderBindings(command string, segments []bindingSegment, values map[string][]string) []string {
	var names []string
	index := map[string]int{}
	total := 1
	for _, s := range segments {
		if s.name == "" {
			continue
		}
		if _, seen := index[s.name]; seen {
			continue
		}
		index[s.name] = len(names)
		names = append(names, s.name)
		if total < maxBindingVariants {
			total *= len(values[s.name])
		}
	}
	if total > maxBindingVariants {
		total = maxBindingVariants
	}
	readings := make([]string, 0, total)
	seen := map[string]bool{}
	choice := make([]int, len(names))
	for k := 0; k < total; k++ {
		rest := k
		for i, name := range names {
			choice[i] = rest % len(values[name])
			rest /= len(values[name])
		}
		var b strings.Builder
		for _, s := range segments {
			if s.name == "" {
				b.WriteString(s.text)
			} else {
				b.WriteString(values[s.name][choice[index[s.name]]])
			}
		}
		line := b.String()
		if len(line) > maxCommandBytes {
			line = command
		}
		if !seen[line] {
			seen[line] = true
			readings = append(readings, line)
		}
	}
	return readings
}

// mostImportant is the refusal a gate reports for a set of decisions about
// one line — its clauses, or its readings: the first hard refusal for what
// something does, else the first refusal for what the gate could not read
// (IsUnreadableRule), else the first soft refusal, else the first warning. ok
// is false when every decision allowed.
func mostImportant(decisions []Decision) (Decision, bool) {
	var unreadable, soft, warned *Decision
	for i := range decisions {
		d := &decisions[i]
		switch {
		case d.Action == Deny && IsUnreadableRule(d.Rule):
			if unreadable == nil {
				unreadable = d
			}
		case d.Action == Deny && d.Severity == Hard:
			return *d, true
		case d.Action == Deny:
			if soft == nil {
				soft = d
			}
		case d.Action == Warn:
			if warned == nil {
				warned = d
			}
		}
	}
	for _, d := range []*Decision{unreadable, soft, warned} {
		if d != nil {
			return *d, true
		}
	}
	return Decision{}, false
}

// refusesForWhatItDoes reports whether a decision is a hard refusal no other
// decision about the same line can outrank, so judging the rest can stop.
func refusesForWhatItDoes(d Decision) bool {
	return d.Action == Deny && d.Severity == Hard && !IsUnreadableRule(d.Rule)
}
