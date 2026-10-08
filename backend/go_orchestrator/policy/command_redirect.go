package policy

import (
	"fmt"
	"path"
	"strings"
)

// RedirectTargets returns every file this command line would write that the
// gate can find in its text: the targets of output redirections (>, >>, >|,
// &>, >& and their fd-prefixed forms), and the files the line's commands write
// through their arguments — sed -i, tee, cp and mv; see argumentWriteTargets.
// Input redirections (<) are reads, which this ladder does not gate, and a
// heredoc introducer (<<) is not a path at all; neither is returned.
//
// The name predates the argument half. Both halves answer one question — which
// files does this line change — and they share one dedupe, one stream-device
// exemption and one bound, because the caller judges the combined list as one
// set of writes.
//
// The second return value reports whether a target could not be resolved to a
// literal path — it carries an expansion ($HOME, ${VAR}, ~, a substitution)
// whose value only the shell knows, or it sits inside a construct this scanner
// could not read to its end. A target the gate cannot name is a write it
// cannot judge, so the caller must treat that as a refusal rather than skip
// the check.
//
// Matching strips trailing redirections so patterns like "dev map *" stay
// single-clause, which is exactly why the executed form has to be re-read
// here: the string that matched is not the string that runs.
//
// Command substitutions are descended into rather than skipped. Their contents
// are recursed through the policy ladder, and that ladder has no redirect rung
// of its own — the rung lives above it, in the caller of this function — so a
// substitution the scanner stepped over was a write nothing ever judged:
// `echo $(git diff > ~/.ssh/authorized_keys)` was allowed while the same
// redirect on its own was a hard denial. Backticks and <( … ) happened to be
// caught before, by the scanner not recognising them at all and stumbling onto
// the `>` inside; they are now found the same principled way as $( … ), so the
// three cannot diverge again.
func RedirectTargets(command string) ([]string, bool, error) {
	targets, opaque, err := redirectTargets(command, 0)
	if err != nil {
		return nil, false, err
	}
	// Deduplicated, and bounded. Both are about what the caller does with this
	// list: it puts every entry through the write gate, so the length of the
	// list is a multiplier on the cost of judging one command line.
	//
	// `a > f && a > f && …` names one file however many times it is repeated,
	// and re-judging it once per clause turned a 360 KiB command line into
	// eight seconds of gate time — the same verdict, twenty thousand times.
	// Distinctness is the honest unit anyway: the question the gate answers is
	// "may this file be written", and it has one answer per file.
	seen := make(map[string]struct{}, len(targets))
	distinct := make([]string, 0, len(targets))
	for _, target := range targets {
		if _, dup := seen[target]; dup {
			continue
		}
		// A stream device is not a file the command writes; see IsStreamDevice.
		if IsStreamDevice(target) {
			continue
		}
		if len(distinct) >= maxRedirectTargets {
			// Past the cap the enumeration is incomplete, and incomplete is
			// exactly what opacity means — the caller refuses rather than
			// judging a prefix of the writes and calling it the whole set.
			return distinct, true, nil
		}
		seen[target] = struct{}{}
		distinct = append(distinct, target)
	}
	return distinct, opaque, nil
}

// streamDevices are the device paths a redirection writes to without creating
// or changing any file: a sink that keeps nothing, or a stream the command
// already owns.
var streamDevices = map[string]bool{
	"/dev/null":   true,
	"/dev/zero":   true,
	"/dev/stdout": true,
	"/dev/stderr": true,
	"/dev/tty":    true,
}

// IsStreamDevice reports whether a redirection target is a stream rather than a
// file, so that it is not a write for the write gate to judge.
//
// It used to be judged as one, and refused as path.outside_root — a hard,
// ungrantable denial for `ls 2>/dev/null` and `cmd >/dev/null 2>&1`, which
// appear in nearly every shell line an agent writes, while the same command
// without the redirect ran. A redirect into /dev/fd/N writes to a descriptor
// that is either inherited from the caller or was opened by an earlier
// redirection on the same line, and that earlier redirection is itself a target
// judged here.
//
// The match is exact after cleaning, which is how the kernel resolves the path
// too: `/dev//null` is the sink, while `/dev/null.txt`, `/dev/fd/2x` and
// `/dev/fd/../../etc/passwd` are ordinary paths and are judged as the writes
// they are.
func IsStreamDevice(target string) bool {
	if !strings.HasPrefix(target, "/dev/") {
		return false
	}
	cleaned := path.Clean(target)
	if streamDevices[cleaned] {
		return true
	}
	fd, ok := strings.CutPrefix(cleaned, "/dev/fd/")
	if !ok || fd == "" || len(fd) > 4 {
		return false
	}
	for _, r := range fd {
		if r < '0' || r > '9' {
			return false
		}
	}
	return true
}

// maxRedirectTargets bounds how many distinct files one command line may be
// judged to write.
//
// It is a backstop rather than a tuning knob. A real command redirects into a
// handful of files; a line naming more than this is not a command someone
// wrote, and enumerating it without limit hands an unbounded amount of gate
// work to whoever composed the string. Exceeding it is reported as an
// incomplete enumeration, so the caller fails closed instead of judging the
// first sixty-four writes and ignoring the rest.
const maxRedirectTargets = 64

func redirectTargets(command string, depth int) ([]string, bool, error) {
	// Exhausting the bound reports opacity, not an error. Both mean "there may
	// be a write in here that I did not resolve", but they travel differently:
	// opacity becomes a recorded command.substitution denial the run log counts
	// and Report() can account for, while an error unwinds past the decision
	// entirely and is refused by whoever catches it, if anyone does. It is also
	// the answer this function already gives for the other unresolvable target
	// — `> $VAR` — and one predicate should not have two spellings of "I could
	// not look". What must never happen is the third answer: an unsearched span
	// reported as "no targets", which is how "approved" comes to mean
	// "unexamined".
	if depth > maxSubstitutionDepth {
		return nil, true, nil
	}
	s := redirectScan{runes: []rune(command), depth: depth}
	if err := s.scanOperators(); err != nil {
		return nil, false, err
	}
	s.addArgumentWrites()
	s.reconcileWithSubstitutions(command)
	return s.targets, s.opaque, nil
}

// redirectScan is one command line's walk in redirectTargets: the line, the
// recursion depth it was reached at, and what the walk has found so far.
type redirectScan struct {
	runes []rune
	depth int

	targets []string
	opaque  bool

	// masked is this command line as the argument scan reads it: every
	// substitution span scanOperators descends into is replaced by
	// substitutionPlaceholder (processSubstitutionPlaceholder for <( … ) and
	// >( … )), every output redirection it reads is replaced by a space, and
	// every unquoted ( or ) — subshell syntax, never part of a word — becomes a
	// space. What is left is the line's own words, split into clauses by the
	// same splitter the ladder uses, with nothing in it that scanOperators has
	// already accounted for. The spans themselves are scanned by descend, which
	// runs all of redirectTargets, argument scan included, on their text.
	masked strings.Builder
}

// scanOperators is the first stage: one pass over the line that descends into
// every substitution span, reads every output redirection's target, and builds
// masked for the argument stage.
func (s *redirectScan) scanOperators() error {
	runes := s.runes
	n := len(runes)
	quote := rune(0)
	i := 0
	for i < n {
		if next, nq, handled := shellQuoteStep(runes, i, quote); handled {
			s.masked.WriteString(string(runes[i:next]))
			quote = nq
			i = next
			continue
		}
		r := runes[i]
		var (
			next int
			err  error
		)
		switch {
		case r == '`':
			next, err = s.backtickSubstitution(i)
		case r == '$' && i+2 < n && runes[i+1] == '(' && runes[i+2] == '(':
			next, err = s.arithmeticExpansion(i)
		case r == '$' && i+1 < n && runes[i+1] == '(':
			next, err = s.commandSubstitution(i)
		case quote == 0 && (r == '<' || r == '>') && i+1 < n && runes[i+1] == '(':
			next, err = s.processSubstitution(i)
		case quote != 0:
			// Only the "…" state reaches here. The substitutions above still
			// execute inside it; a bare > does not — it is literal text.
			s.masked.WriteRune(r)
			next = i + 1
		case r == '<' && i+1 < n && runes[i+1] == '<':
			// Heredoc introducer or herestring; not an output path.
			s.masked.WriteString("<<")
			next = i + 2
		case r == '>' || (r >= '0' && r <= '9' && i+1 < n && runes[i+1] == '>'):
			next, err = s.outputRedirection(i)
		case r == '(' || r == ')':
			// Unquoted, a parenthesis is subshell or grouping syntax and
			// never part of a word: `(tee f)` runs tee on f, not on "f)".
			s.masked.WriteRune(' ')
			next = i + 1
		default:
			s.masked.WriteRune(r)
			next = i + 1
		}
		if err != nil {
			return err
		}
		i = next
	}
	return nil
}

// descend judges the redirections inside a substitution as the writes they
// are. Its findings merge into this command's, because sh performs them
// whether or not the surrounding line has a redirect of its own.
func (s *redirectScan) descend(inner string) error {
	innerTargets, innerOpaque, err := redirectTargets(inner, s.depth+1)
	if err != nil {
		return err
	}
	s.targets = append(s.targets, innerTargets...)
	s.opaque = s.opaque || innerOpaque
	return nil
}

// backtickSubstitution handles a legacy `…` substitution opening at i.
func (s *redirectScan) backtickSubstitution(i int) (int, error) {
	text, next, err := scanBacktickSpan(s.runes, i)
	if err != nil {
		return 0, err
	}
	if err := s.descend(text); err != nil {
		return 0, err
	}
	s.masked.WriteString(substitutionPlaceholder)
	return next, nil
}

// arithmeticExpansion handles a $(( … )) opening at i.
func (s *redirectScan) arithmeticExpansion(i int) (int, error) {
	inner, next, ok := scanArithmetic(s.runes, i)
	if !ok {
		return 0, fmt.Errorf("unterminated arithmetic expansion")
	}
	// The inner text is NOT scanned as a redirect context. Inside
	// arithmetic `>` and `<` are comparison operators: `echo $((3 > 2))`
	// writes no file, and reading that `>` as a redirection invents a
	// write to a file named 2. What is real in here is a nested command
	// substitution — an ordinary command, whose redirections write
	// ordinary files — so those are extracted and descended into, and
	// nothing else is.
	nested, err := liveSubstitutions(inner)
	if err != nil {
		return 0, err
	}
	for _, span := range nested {
		if err := s.descend(span); err != nil {
			return 0, err
		}
	}
	s.masked.WriteString(substitutionPlaceholder)
	return next, nil
}

// commandSubstitution handles a $( … ) opening at i.
func (s *redirectScan) commandSubstitution(i int) (int, error) {
	text, next, err := scanParenSpan(s.runes, i+1)
	if err != nil {
		return 0, err
	}
	if err := s.descend(text); err != nil {
		return 0, err
	}
	s.masked.WriteString(substitutionPlaceholder)
	return next, nil
}

// processSubstitution handles an unquoted <( … ) or >( … ) opening at i.
func (s *redirectScan) processSubstitution(i int) (int, error) {
	// Process substitution: the code inside runs, and its redirections
	// write files, exactly like $( … ) — but only unquoted. Inside
	// double quotes it is literal text, which is why this carries the
	// same guard as its counterpart in liveSubstitutions; see the note
	// there for the measurements.
	text, next, err := scanParenSpan(s.runes, i+1)
	if err != nil {
		return 0, err
	}
	if err := s.descend(text); err != nil {
		return 0, err
	}
	// Unlike $( … ), the word this leaves behind is not computed
	// text: sh replaces it with a /dev/fd path to the pipe, which is
	// a stream rather than a file. `tee >(gzip > out.gz) f` writes
	// out.gz inside the span and f outside it, and nothing else.
	s.masked.WriteString(processSubstitutionPlaceholder)
	return next, nil
}

// outputRedirection handles an unquoted >, or a run of digits that may be the
// descriptor prefix of one, starting at i. It records the target the operator
// names, or opacity when the target cannot be named.
func (s *redirectScan) outputRedirection(i int) (int, error) {
	runes := s.runes
	n := len(runes)
	fdStart := i
	for i < n && runes[i] >= '0' && runes[i] <= '9' {
		i++
	}
	if i >= n || runes[i] != '>' {
		s.masked.WriteRune(runes[fdStart])
		return fdStart + 1, nil
	}
	// sh reads leading digits as a descriptor only when they are the
	// whole word before the operator: `tee a2>f` writes a2 and f. This
	// scan has always read the 2 as a descriptor, which is harmless
	// for the redirect target, but the argument scan must still see
	// the word a2.
	if fdStart > 0 && !strings.ContainsRune(" \t\n\r;|&()", runes[fdStart-1]) {
		s.masked.WriteString(string(runes[fdStart:i]))
	}
	s.masked.WriteRune(' ')
	i++                           // consume '>'
	if i < n && runes[i] == '>' { // append form >>
		i++
	} else if i < n && runes[i] == '|' {
		// >| is > with noclobber overridden. It names a path exactly
		// as > does; read as an unresolvable target it reported
		// opacity, which refused the command for the wrong reason and
		// — because the refusal never reached the write gate — let the
		// path itself go unjudged.
		i++
	} else if i < n && runes[i] == '&' { // >&N duplicates descriptors
		i++
		for i < n && runes[i] >= '0' && runes[i] <= '9' {
			i++
		}
		return i, nil
	}
	target, next, err := readRedirectTarget(runes, i)
	if err != nil {
		return 0, err
	}
	if unresolvableWord(target) {
		s.opaque = true
	} else if target != "" {
		s.targets = append(s.targets, target)
	} else {
		s.opaque = true
	}
	return next, nil
}

// readRedirectTarget reads the word an output redirection names, starting at
// start (leading spaces skipped). It returns the dequoted target and the index
// one past it; an empty target with no error is a dup to a descriptor (>&N,
// &N), which names no path.
func readRedirectTarget(runes []rune, start int) (string, int, error) {
	n := len(runes)
	j := start
	for j < n && runes[j] == ' ' {
		j++
	}
	if j >= n {
		return "", 0, fmt.Errorf("redirection with no target")
	}
	if runes[j] == '&' || (runes[j] >= '0' && runes[j] <= '9') && j+1 < n && runes[j+1] == '&' {
		// >&N / &N — dup to a descriptor, not a path.
		for j < n && runes[j] != ' ' {
			j++
		}
		return "", j, nil
	}
	var b strings.Builder
	q := rune(0)
	for j < n {
		r := runes[j]
		if q == 0 && r == '\\' && j+1 < n {
			b.WriteRune(runes[j+1])
			j += 2
			continue
		}
		if q == 0 && (r == '\'' || r == '"') {
			q = r
			j++
			continue
		}
		if q != 0 && r == q {
			q = 0
			j++
			continue
		}
		// A backtick closes a legacy substitution; it is never part of the
		// path. Without it `echo `cat > f`` yielded the target "f`", and
		// the gate then judged a filename the shell never opens — which
		// took a write to .env past the secret rung as ".env`".
		//
		// Live inside double quotes as well as unquoted, which is the half
		// this originally missed: sh expands `…` within "…", and only a
		// single quote makes a backtick literal. While it was guarded on
		// the unquoted state alone, `>"` followed by a backtick pair read
		// as a filename spelled with backticks in it, so the gate judged
		// "`>0`" while the shell ran the substitution and opened `0` — a
		// write the enumeration never reported. Found by
		// FuzzRedirectTargetsSeesInsideEverySubstitution.
		if (q == 0 || q == '"') && r == '`' {
			break
		}
		if q == 0 && (r == ' ' || r == '\n' || r == '\t' || r == ';' || r == '|' || r == '&' ||
			r == '(' || r == ')') {
			break
		}
		b.WriteRune(r)
		j++
	}
	return b.String(), j, nil
}

// addArgumentWrites is the second stage: the files the line writes through its
// commands' arguments rather than through an operator. They are the same
// writes — `sed -i … f`, `tee f`, `cp a f` and `echo x > f` all leave f
// changed — and a rung that judged only the operator spelling refused the
// redirect and passed the other four. See argumentWriteTargets for which
// commands and why.
func (s *redirectScan) addArgumentWrites() {
	argTargets, argOpaque := argumentWriteTargets(s.masked.String())
	s.targets = append(s.targets, argTargets...)
	s.opaque = s.opaque || argOpaque
}

// reconcileWithSubstitutions is the last stage: the scan's targets are
// reconciled against liveSubstitutions before redirectTargets answers, because
// the two scan one string against one grammar and have never fully agreed
// about what that grammar is. Only this half knew `<<` introduces a
// heredoc; only that half knew a backtick stays live inside double quotes;
// neither had process substitution right in quotes; and they still tokenise
// a backslash differently inside a substitution span. Every one of those
// ended the same way — this half answered "these are the writes, and I am
// sure" about a line its sibling read differently — and that is the answer
// EvaluateRedirects turns straight into an allow.
//
// Four instances were found in four runs, so the instances are not the
// thing to fix. The disagreement itself is the condition: if the sibling
// scanner cannot read this line, or finds a span that resolves to a write
// this scan did not reach, the enumeration is not complete and says so.
//
// It reports opacity rather than adopting the other scanner's targets on
// purpose. When two lexers disagree about a string, which one is right is
// exactly what is not known here, and stating a target under this scan's
// authority that this scan did not derive would be a guess wearing a
// result's clothes. Opacity is already this function's word for "there may
// be a write in here that I did not resolve", and it is what the caller
// fails closed on.
//
// The direction is deliberate: this only ever *adds* opacity, and opacity
// is a denial. It also runs after the scan rather than before it, because
// containment is a claim about the targets this scan produced and there are
// none to compare against until it has finished.
func (s *redirectScan) reconcileWithSubstitutions(command string) {
	spans, subErr := liveSubstitutions(command)
	if subErr != nil {
		s.opaque = true
		return
	}
	have := make(map[string]struct{}, len(s.targets))
	for _, target := range s.targets {
		have[target] = struct{}{}
	}
	for _, span := range spans {
		// depth+1 so this shares the recursion bound with descend rather
		// than adding a second, independent one. Past the bound the call
		// returns opacity immediately, which is both the fail-closed answer
		// and what stops this walk.
		spanTargets, spanOpaque, spanErr := redirectTargets(span, s.depth+1)
		if spanErr != nil || spanOpaque {
			s.opaque = true
			return
		}
		for _, target := range spanTargets {
			if _, ok := have[target]; !ok {
				s.opaque = true
				return
			}
		}
	}
}

// substitutionPlaceholder stands in for a substitution span in the text the
// argument scan reads. It carries a `$`, so an operand built from a
// substitution — `cp a $(pick)`, `tee "$(date).log"` — reads as the
// unresolvable word it is, exactly as `> $(pick)` does.
const substitutionPlaceholder = "$()"

// processSubstitutionPlaceholder stands in for a <( … ) or >( … ) span in the
// text the argument scan reads. sh replaces the span with a path to a pipe
// under /dev/fd, which IsStreamDevice exempts, so a command writing to it —
// `tee >(gzip > out.gz)` — names no file beyond what the span itself writes.
const processSubstitutionPlaceholder = "/dev/fd/63"

// unresolvableWord reports whether a write target names a path only the shell
// can compute: it carries a parameter expansion ($HOME, ${X}, or a
// substitution's placeholder) or a tilde. One predicate for redirection targets
// and argument operands, so the two cannot disagree about what is nameable.
func unresolvableWord(word string) bool {
	return strings.ContainsAny(word, "$~")
}
