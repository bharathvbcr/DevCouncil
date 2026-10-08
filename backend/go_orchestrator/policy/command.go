package policy

import (
	"fmt"
	"path"
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// Git-safety patterns, ported from DevCouncil. Compiled once: they are checked
// on every shell command the harness runs.
var (
	hardResetProtectedRe   = regexp.MustCompile(`\bgit\s+reset\s+--hard\s+(origin/)?(main|master)\b`)
	forcePushFlagRe        = regexp.MustCompile(`\bgit\s+push\b.*(\s--force(-with-lease)?\b|\s-f\b)`)
	forcePushPlusRefspecRe = regexp.MustCompile(`\bgit\s+push\s+\S+\s+\+\S`)
	protectedBranchPushRe  = regexp.MustCompile(`\bgit\s+push\s+\S+\s+((head:)?(main|master)|(main|master):\S+)\b`)

	uvRunDirFlagRe = regexp.MustCompile(`^(uv\s+run)((?:\s+(?:--project|--directory|-p)\s+\S+)+)(\s+.+)$`)
	// A directory change at the head of a clause, including inside a subshell
	// or group — `(cd ..; echo x > f)` moves where f lands just as surely, and
	// was missed while the rung only matched the clause's first word — and when
	// spelled through `builtin` or `command`.
	cdSegmentRe = regexp.MustCompile(`^(?:[({]\s*)*(?:(?:builtin|command)\s+)?(?:cd|pushd|popd)(?:\s|$|[;)}])`)
	// Trailing shell redirections break glob matching against patterns like
	// "dev map *". Stripped for matching only, in a loop, so the pattern itself
	// stays single-clause and cannot backtrack pathologically.
	redirectTailRe = regexp.MustCompile(`(?: \d*(?:>>|>\||>|<|&>|&>>) ?\S+| \d*>&\d+)$`)
)

// devBinaries are the executable names normalised back to a bare "dev", so a
// hook-installed ".venv/bin/dev map" still matches the "dev map" allowlist
// entry instead of failing closed.
var devBinaries = map[string]bool{
	"dev":            true,
	"dev.exe":        true,
	"devcouncil":     true,
	"devcouncil.exe": true,
}

// NoTaskAllowedCommands run without any lease: read-only orientation.
//
// Every `dev` entry names a command the host binary dispatches, held there by
// TestPolicyNamesOnlyLiveCommands in cmd/devcouncil. This list used to allow
// `dev status`, `dev tasks`, `dev approve`, `dev checkout`, `dev next-task` and
// `dev doctor`, each also spelled `uv run dev …`: the Python CLI's commands,
// every one of which exits 2 on the Go host, and a `uv` entry point that no
// longer exists. A lease is acquired through the devcouncil_checkout_task MCP
// tool, which is not a shell command and needs no entry here.
var NoTaskAllowedCommands = []string{
	"git status", "git diff", "git diff *",
	"echo", "echo *", "true", ":",
	// Orientation, and here rather than in LeaseLifecycleAllowedCommands
	// because the no-lease denial below names it as the remedy. A message
	// that points at `dev map` while the list holding it sits behind the lease
	// check is a message that cannot be followed.
	"dev map", "dev map *",
}

// noLeaseRemedy is the reason a shell command is refused for want of a lease.
// Every command it names must be one the allowlist above admits and the host
// dispatches; the host's tests read it back from a real decision.
const noLeaseRemedy = "Shell commands require an active task lease. Acquire one with the " +
	"`devcouncil_checkout_task` MCP tool, or use the allowlisted read-only orientation " +
	"commands (`git status`, `git diff`, `dev map …`)."

// LeaseLifecycleAllowedCommands are available to a lease holder, independent
// of the task's own allowed_commands.
//
// "To a lease holder" is enforced by where this list is matched, not only by
// this sentence: the rung sits *below* the no-lease refusal in
// evaluateSingleCommand. It used to sit above it, so a session with no task at
// all reached these entries — and the entries include test runners. `pytest -q`
// was allowed with task == nil. That is not an orientation command: running a
// project's tests executes the project's code, with whatever authority this
// process holds, which is the authority a lease exists to allocate.
//
// Entries that genuinely need no lease belong in NoTaskAllowedCommands, where
// the no-lease refusal can honestly point at them.
//
// `dev release`, `dev lease`, `dev scope` and `dev run-cmd` were dropped with
// the Python CLI: leases are released and renewed through the devcouncil_*
// MCP tools, and none of those names dispatches on the Go host.
var LeaseLifecycleAllowedCommands = []string{
	"dev graph", "dev graph *",
	"python -m pytest *", "uv run python -m pytest *",
	"uv run pytest *", "pytest *",
}

// CommandGate evaluates shell commands against a task's allowlist and against
// the git-safety rules.
type CommandGate struct {
	// GlobalAllowedCommands supplement every task's own allowlist.
	GlobalAllowedCommands []string
	// HardRules mirrors flags.PolicyHardRules.
	HardRules bool
	// Root is the absolute path of the working tree this gate judges for.
	//
	// It exists so that an absolute `…/.venv/bin/dev` can be told apart from
	// the repository's own — see NormalizeAllowlistCommand. Empty is a
	// supported value and it fails closed: with no root to compare against,
	// no absolute path is attributed to this tree, so none is normalised into
	// a bare `dev` and none inherits `dev`'s allowlist entries. A gate that
	// leaves this unset is therefore stricter, never looser.
	Root string
	// Matcher answers the allowlist questions. Nil is GoMatcher. A matcher
	// that fails turns the decision into a denial under
	// RuleCommandEngineUnavailable: an allowlist that cannot be consulted
	// allows nothing.
	Matcher Matcher
}

// maxSubstitutionDepth bounds the recursion into command-substitution
// contents. Each level judges strictly less text than its parent, so the cap
// is unreachable except by adversarial input — and adversarial input is
// exactly what gets refused rather than recursed into forever.
const maxSubstitutionDepth = 8

// EvaluateCommand walks DevCouncil's command ladder.
//
// Git safety runs first and unconditionally. In the Python engine the two
// evaluations are separate entry points — `evaluate_command` for the allowlist
// and `evaluate_hook_command` for git safety — which means a caller that only
// invokes one gets only half the protection. Folding safety into the front of
// the single entry point closes that, and it cannot loosen anything: every
// git-safety outcome is a denial or a warning, never an allow that skips the
// allowlist below.
func (g CommandGate) EvaluateCommand(command string, task *dc.Task) (d Decision) {
	taskID := ""
	if task != nil {
		taskID = task.ID
	}
	defer failClosed(&d, RuleCommandEngineUnavailable, command, taskID, g.HardRules)
	// Length is checked once, here, before any rung reads the line. Every rung
	// below is a scan and several recurse, so an unbounded line is an unbounded
	// amount of work handed to the gate by whoever composed the string — and
	// the string is composed by a model. Measured on this machine, a 1.8 MB
	// line of chained substitutions took seven seconds to refuse.
	//
	// It is deliberately not a rung inside the ladder: a rung would be reached
	// per clause, after the splitter had already walked the whole input, which
	// is most of the cost it exists to avoid.
	if len(command) > maxCommandBytes {
		taskID := ""
		if task != nil {
			taskID = task.ID
		}
		return g.noteHardRules(deny(RuleCommandTooLong,
			fmt.Sprintf("Command is %d bytes; the limit is %d. A line this long is not a command "+
				"someone wrote — put the work in a script file and run that.",
				len(command), maxCommandBytes),
			// The line itself is not echoed back into the decision: it is
			// megabytes of model-authored text and Target travels into logs,
			// transcripts and the TUI.
			fmt.Sprintf("<%d-byte command>", len(command)), taskID))
	}
	return g.evaluate(command, task, 0)
}

// maxCommandBytes bounds the command line the ladder will read.
//
// Generous by two orders of magnitude against anything a person or an agent
// legitimately writes — real command lines are tens of bytes — and small enough
// that the worst measured input costs well under a second to refuse. It is a
// backstop on cost, not a style rule.
const maxCommandBytes = 128 << 10

func (g CommandGate) evaluate(command string, task *dc.Task, depth int) Decision {
	parts := SplitCommandChain(command)
	if len(parts) > 1 {
		// Every clause is judged, and the refusal reported is the most
		// important one, not the first. Returning the first let an earlier
		// clause hide a later one: `date; git push --force` came back as
		// `date`'s soft command.not_allowed, which a posture demotes, so the
		// push ran; `cd src && git push --force` came back as a directory
		// change, which a host may reasonably treat as "not judged" rather
		// than "forbidden". The order is: a hard refusal for what a clause
		// does, then one for what the gate could not read (IsUnreadableRule),
		// then a soft one, then a warning.
		var warnDecision, unreadable, soft *Decision
		for _, part := range parts {
			d := g.evaluateSingleCommand(part, task, depth)
			switch {
			case d.Action == Deny && IsUnreadableRule(d.Rule):
				if unreadable == nil {
					unreadable = &d
				}
			case d.Action == Deny && d.Severity == Hard:
				return d
			case d.Action == Deny:
				if soft == nil {
					soft = &d
				}
			case d.Action == Warn && warnDecision == nil:
				warnDecision = &d
			}
		}
		if unreadable != nil {
			return *unreadable
		}
		if soft != nil {
			return *soft
		}
		if warnDecision != nil {
			return *warnDecision
		}
		taskID := ""
		if task != nil {
			taskID = task.ID
		}
		return g.noteHardRules(allow("Compound command allowed.", command, taskID))
	}
	return g.evaluateSingleCommand(command, task, depth)
}

func (g CommandGate) evaluateSingleCommand(command string, task *dc.Task, depth int) Decision {
	raw := collapseSpaces(command)
	normalized := NormalizeAllowlistCommandInRoot(command, g.Root)
	taskID := ""
	if task != nil {
		taskID = task.ID
	}

	if normalized == "" {
		return g.noteHardRules(deny(RuleCommandEmpty, "Empty command is not allowed.", command, taskID))
	}

	// Git safety first, ahead of the rungs that refuse what they cannot read.
	// It reads the visible text, which is all it ever reads, and it used to run
	// only after them — so `git push --force origin main <<'EOF'` was refused
	// as a heredoc and the force push was never named. A refusal for what the
	// line does must never be hidden behind one for what the gate could not
	// read (see evaluate).
	if g.HardRules {
		if d, fired := gitSafety(normalized, taskID); fired && d.Action == Deny {
			return d
		}
	}

	var f clauseFindings
	g.noteOpaqueConstructs(&f, raw, normalized, taskID)
	if d, hard := g.judgeSubstitutions(&f, raw, normalized, task, taskID, depth); hard {
		return d
	}
	g.noteDirectoryChanges(&f, normalized, taskID)
	if d, found := f.verdict(); found {
		return d
	}

	return g.matchAllowlists(raw, normalized, task, taskID)
}

// clauseFindings collects the refusals and warnings one clause's rungs find
// before the allowlists are consulted.
//
// The unreadable rungs (IsUnreadableRule) are collected rather than returned
// on the spot, so a hard refusal found later in the clause — in a
// substitution span, say — still outranks them. The first one found is the
// one reported, in the order evaluateSingleCommand runs the rungs.
type clauseFindings struct {
	unreadable, softInner, warnDecision *Decision
}

func (f *clauseFindings) noteUnreadable(d Decision) {
	if f.unreadable == nil {
		f.unreadable = &d
	}
}

// verdict reports the most important finding, if there is one: an unreadable
// construct, then a soft refusal from inside a substitution, then a warning.
func (f *clauseFindings) verdict() (Decision, bool) {
	if f.unreadable != nil {
		return *f.unreadable, true
	}
	if f.softInner != nil {
		return *f.softInner, true
	}
	if f.warnDecision != nil {
		return *f.warnDecision, true
	}
	return Decision{}, false
}

// noteOpaqueConstructs notes the constructs whose meaning is not in the text
// being judged: a heredoc and a re-parsing command word.
//
// A live substitution or a heredoc carries code this ladder cannot read:
// an allowlist entry matched against the surrounding line never judged
// what `sh -c` would actually execute inside it. Substitution contents are
// extracted and run through this same gate — so `echo $(date)` is judged
// as both echo and date — and anything the scanner cannot bound is refused
// outright rather than guessed at. A heredoc body is expanded data with no
// reliable static end, so it has no extraction path and is refused. (Its
// lines are still judged: the chain splitter breaks on unquoted newlines,
// so each body line reaches this ladder as a clause of its own.)
func (g CommandGate) noteOpaqueConstructs(f *clauseFindings, raw, normalized, taskID string) {
	if hasHeredoc(raw) {
		f.noteUnreadable(g.noteHardRules(deny(RuleCommandHeredoc,
			"Heredocs carry expanded data with no statically checkable end and are not allowed; "+
				"write the content to a file instead.", normalized, taskID)))
	}
	// Checked on the raw line, beside the heredoc rung, because it is the same
	// refusal: a construct whose meaning is not in the text being judged. It
	// runs before the substitution rung so that `eval $(...)` is named as the
	// re-parse it is rather than as the substitution it also contains.
	if word, isReparse := reparsingCommandWord(raw); isReparse {
		f.noteUnreadable(g.noteHardRules(deny(RuleCommandReparse,
			"`"+word+"` re-parses its argument as shell code after expansion, so nothing in this "+
				"line — the allowlist match, the git-safety rules, or the redirection targets — "+
				"describes what would actually run; write the commands out directly instead.",
			normalized, taskID)))
	}
}

// judgeSubstitutions runs every live substitution span in raw through this
// same gate, one level deeper. A hard refusal from inside a span is returned
// with hard=true, to be reported at once; everything else is noted in f.
func (g CommandGate) judgeSubstitutions(f *clauseFindings, raw, normalized string, task *dc.Task, taskID string, depth int) (Decision, bool) {
	spans, subErr := liveSubstitutions(raw)
	switch {
	case subErr != nil:
		f.noteUnreadable(g.noteHardRules(deny(RuleCommandSubstitution,
			"Command substitution could not be analysed to its end and is not allowed; "+
				"rewrite without $(), backticks, <() or >().", normalized, taskID)))
	case len(spans) > 0 && depth >= maxSubstitutionDepth:
		f.noteUnreadable(g.noteHardRules(deny(RuleCommandSubstitution,
			"Command substitution nested beyond the analysis limit is not allowed; "+
				"run the inner commands separately.", normalized, taskID)))
	case len(spans) > 0:
		for _, span := range spans {
			d := g.evaluate(span, task, depth+1)
			if d.Action == Deny {
				// The inner refusal is returned under its own rule and
				// severity. It used to be re-issued as a hard
				// command.substitution, which made a refusal negotiable or not
				// according to where the command was written: `date` alone was
				// a soft command.not_allowed a grant or posture could clear,
				// `echo "$(date)"` a hard denial nothing could — though the
				// span was read to its end and nothing about it was opaque.
				// command.substitution stays for what it names: a span the
				// scanner could not bound (the two cases above). A hard inner
				// refusal is still hard, so wrapping `git push --force` in
				// $(…) launders nothing.
				d.Reason = fmt.Sprintf("Substituted command `%s` was denied: %s", clipForReason(span), d.Reason)
				d.Target = normalized
				switch {
				case IsUnreadableRule(d.Rule):
					f.noteUnreadable(d)
				case d.Severity == Hard:
					return d, true
				case f.softInner == nil:
					f.softInner = &d
				}
				continue
			}
			if d.Action == Warn && f.warnDecision == nil {
				f.warnDecision = &d
			}
		}
	}
	return Decision{}, false
}

// noteDirectoryChanges notes a clause that moves where its relative paths
// resolve: a cd/pushd/popd, or git's own directory options.
func (g CommandGate) noteDirectoryChanges(f *clauseFindings, normalized, taskID string) {
	// A directory change is refused, not waved through. It used to be allowed
	// on the ground that it "cannot write" — true of the cd itself, and beside
	// the point, because it decides where every relative path *after* it
	// writes. See RuleCommandDirectoryChange.
	if cdSegmentRe.MatchString(normalized) {
		f.noteUnreadable(g.noteHardRules(deny(RuleCommandDirectoryChange,
			"Changing the working directory is not allowed: every relative path in this command "+
				"would then resolve somewhere other than where the gate judged it. Use paths "+
				"relative to the repository root instead.", normalized, taskID)))
	}

	// git carries its own directory change, and it is the same refusal. `git -C
	// ../elsewhere status` reads a different working tree; `--git-dir` and
	// `--work-tree` split the two apart. A rung that refuses `cd` while these
	// pass is a rung that refuses one spelling of the problem.
	if opt, moved := gitDirectoryEscape(normalized); moved {
		f.noteUnreadable(g.noteHardRules(deny(RuleCommandDirectoryChange,
			"`git "+opt+"` runs against a working tree other than the one this gate judges for, "+
				"so nothing it does was examined. Run git from the repository root instead.",
			normalized, taskID)))
	}
}

// matchAllowlists is the last stage of evaluateSingleCommand: the lists a
// clause that nothing above refused is allowed by, in order, and the refusal
// when none admits it.
func (g CommandGate) matchAllowlists(raw, normalized string, task *dc.Task, taskID string) Decision {
	if matcherOf(g.Matcher).any(NoTaskAllowedCommands, normalized) {
		return g.finish(allow("Bootstrap or read-only command allowed.", normalized, taskID), normalized, taskID)
	}

	// Ordering is the rule, not a detail. Every rung below this point describes
	// what a *lease holder* may do, so the refusal has to come first: with the
	// lifecycle rung above it, a session holding no task reached an allowlist
	// documented as "available to a lease holder" and ran the project's test
	// suite from it. A list that is only lease-gated by its own name is not
	// lease-gated.
	if task == nil {
		return g.noteHardRules(deny(RuleCommandNoLease,
			noLeaseRemedy,
			normalized, ""))
	}

	if matcherOf(g.Matcher).any(LeaseLifecycleAllowedCommands, normalized) {
		return g.finish(allow("Lease lifecycle or repo maintenance command allowed.", normalized, taskID), normalized, taskID)
	}

	// Allowlist entries are matched against both the raw and normalized forms,
	// so a task listing ".venv/bin/dev *" still works while a path-prefixed
	// "dev map" continues to hit the lifecycle patterns after normalization.
	if matchesEither(matcherOf(g.Matcher), task.AllowedCommands, normalized, raw) {
		return g.finish(allow("Command matches task allowed_commands.", normalized, task.ID), normalized, task.ID)
	}
	if matchesEither(matcherOf(g.Matcher), g.GlobalAllowedCommands, normalized, raw) {
		return g.finish(allow("Command matches global allowed commands.", normalized, task.ID), normalized, task.ID)
	}

	return g.noteHardRules(deny(RuleCommandNotAllowed,
		"Command is not in task or global allowlists.", normalized, task.ID))
}

// ansiCQuote is the scanner state inside a $'…' span.
//
// sh's ANSI-C quoting is not an ordinary single quote: a backslash escapes
// inside it, so a dollar-quote span holding a backslash-escaped quote is one
// word containing a quote character, and everything after that span is
// UNQUOTED. Reading it as a plain '…' swallows the rest of the line — which is
// how `echo $[backslash-quote span] ; mkdir OWNED` reached the gate as a
// single command. It is therefore its own state, not a flag on the
// single-quote one.
const ansiCQuote = rune(-1)

// shellQuoteStep advances past exactly one quoting construct at runes[i],
// given the quote state on entry. It returns the index one past what it
// consumed, the resulting state, and whether it consumed anything at all.
//
// Every scanner in this file routes its quote handling through here. They each
// used to carry their own copy of the rules and had drifted: the chain splitter
// alone had no unquoted-backslash case, so `echo \'` flipped it INTO a quoted
// span while sh — which reads \' as a literal quote character and stays
// unquoted — went on to run whatever followed the next `;`. One owner for the
// rules is what makes that drift impossible rather than merely fixed once.
//
// handled=false means the rune is not part of the quoting grammar and the
// caller must decide what it is. That happens in the unquoted state, and in the
// "…" state, where command substitutions still execute.
func shellQuoteStep(runes []rune, i int, quote rune) (next int, newQuote rune, handled bool) {
	n := len(runes)
	r := runes[i]
	switch quote {
	case ansiCQuote:
		if r == '\\' && i+1 < n {
			return i + 2, quote, true
		}
		if r == '\'' {
			return i + 1, 0, true
		}
		return i + 1, quote, true
	case '\'':
		// Inside '…' nothing is special but the closing quote — a backslash
		// there is an ordinary character.
		if r == '\'' {
			return i + 1, 0, true
		}
		return i + 1, quote, true
	case '"':
		if r == '\\' && i+1 < n {
			return i + 2, quote, true
		}
		if r == '"' {
			return i + 1, 0, true
		}
		return i, quote, false
	default: // unquoted
		switch {
		case r == '\\':
			// A backslash quotes the single next character, whatever it is —
			// a quote, an operator, or a newline. A trailing one quotes
			// nothing and is consumed on its own.
			if i+1 < n {
				return i + 2, 0, true
			}
			return i + 1, 0, true
		case r == '$' && i+1 < n && runes[i+1] == '\'':
			return i + 2, ansiCQuote, true
		case r == '\'':
			return i + 1, '\'', true
		case r == '"':
			return i + 1, '"', true
		}
		return i, 0, false
	}
}

// SplitCommandChain splits a shell command line on &&, ||, ;, |, a lone &,
// and unquoted newlines — every operator sh treats as a command boundary.
// Quoting is read through shellQuoteStep, so single quotes, double quotes,
// $'…' spans and backslash escapes are honoured exactly as sh honours them. A
// boundary the splitter misses is a second command hidden inside one string
// the gate judges once, so anything the shell could read as "start another
// command" splits here; splitting too much costs nothing, because each part is
// judged on its own merits anyway.
func SplitCommandChain(command string) []string {
	var parts []string
	var cur strings.Builder
	quote := rune(0)
	runes := []rune(command)
	n := len(runes)
	flush := func() {
		if s := strings.TrimSpace(cur.String()); s != "" {
			parts = append(parts, s)
		}
		cur.Reset()
	}
	for i := 0; i < n; {
		if next, nq, handled := shellQuoteStep(runes, i, quote); handled {
			cur.WriteString(string(runes[i:next]))
			quote = nq
			i = next
			continue
		}
		r := runes[i]
		if quote != 0 {
			// Only the "…" state reaches here. Its contents are one word as
			// far as command boundaries go; the substitution rung judges the
			// code that still executes inside it.
			cur.WriteRune(r)
			i++
			continue
		}
		switch {
		case r == ';' || r == '|':
			if r == '|' && i+1 < n && runes[i+1] == '|' {
				i++
			}
			flush()
			i++
		case r == '&' && i+1 < n && runes[i+1] == '&':
			i += 2
			flush()
		case r == '&':
			// sh reads '&' as a control operator except where it completes a
			// redirection: `2>&1` and `>&2` duplicate descriptors, and `&>` /
			// `&>>` are themselves redirections. Those stay text. Anything
			// else — backgrounding, with or without a trailing space — is a
			// command boundary, because the backgrounded job runs just the
			// same.
			if i > 0 && runes[i-1] == '>' {
				cur.WriteRune(r)
				i++
				break
			}
			if i+1 < n && runes[i+1] == '>' {
				cur.WriteRune('&')
				cur.WriteRune('>')
				i += 2
				if i < n && runes[i] == '>' {
					cur.WriteRune('>')
					i++
				}
				break
			}
			flush()
			i++
		case r == '\n' || r == '\r':
			// An unquoted newline is sh's statement separator.
			flush()
			i++
		default:
			cur.WriteRune(r)
			i++
		}
	}
	flush()
	return parts
}

// finish applies the git-safety warnings to a command the allowlist accepted,
// so a protected-branch push is still flagged even when a task allows "git *".
func (g CommandGate) finish(d Decision, normalized, taskID string) Decision {
	if g.HardRules {
		if safety, fired := gitSafety(normalized, taskID); fired {
			return g.noteHardRules(safety)
		}
	}
	return g.noteHardRules(d)
}

// GitSafety evaluates only the git-safety rules, mirroring DevCouncil's
// `evaluate_hook_command` exactly so the two can be compared command for
// command. Returns an allow when no rule fires.
//
// EvaluateCommand calls the same rules; this entry point exists because the
// incumbent exposes them separately and parity has to be provable at that
// granularity.
func GitSafety(command string) Decision {
	normalized := collapseSpaces(command)
	if normalized == "" {
		return allow("No command detected.", normalized, "")
	}
	if d, fired := gitSafety(normalized, ""); fired {
		return d
	}
	return allow("Command is allowed.", normalized, "")
}

// gitSafety returns the git-safety verdict and whether any rule fired.
//
// The rules are matched against the line as normalised *and* against a
// dequoted reading of it. Quoting is how sh hides characters from word
// splitting while still concatenating them into one argument, so
// `--no-'v'erify` reaches git as `--no-verify` and `git "reset" --hard` as an
// ordinary reset — a check that only reads the raw text is bypassed by
// exactly the commands worth catching. Checking both readings costs at most a
// false positive on a command that merely prints forbidden text, which is the
// safe direction for this rung.
func gitSafety(normalized, taskID string) (Decision, bool) {
	variants := []string{normalized}
	add := func(v string) {
		for _, have := range variants {
			if have == v {
				return
			}
		}
		variants = append(variants, v)
	}
	if dq := shellDequote(normalized); dq != normalized {
		add(dq)
	}
	// Each variant is also read with git's global options removed. The rules
	// below are written against `git <subcommand> …` and matched by adjacency,
	// so anything git accepts between the two hides the subcommand from them:
	// `git -C . push --force`, `git -c core.pager=cat push --force` and
	// `git --no-pager push --force` all force-push and none of them matched.
	// Added as extra readings rather than as a replacement, because every
	// outcome this rung produces is a denial or a warning — one more reading
	// can only catch more, never excuse something the plain text already
	// convicted.
	for _, v := range append([]string(nil), variants...) {
		if stripped := stripGitGlobalOptions(v); stripped != v {
			add(stripped)
		}
	}
	for _, variant := range variants {
		if d, fired := gitSafetyVariant(strings.ToLower(variant), taskID); fired {
			return d, true
		}
	}
	return Decision{}, false
}

// gitDirectoryEscape reports whether a git invocation redirects itself at a
// working tree other than the one the gate judges for, and names the option.
//
// These are exactly the global options stripGitGlobalOptions already knows how
// to skip past; this rung is what keeps skipping them from being the same as
// ignoring them.
func gitDirectoryEscape(command string) (string, bool) {
	fields := strings.Fields(command)
	for i, f := range fields {
		if !isGitWord(f) {
			continue
		}
		for j := i + 1; j < len(fields) && strings.HasPrefix(fields[j], "-"); j++ {
			opt := fields[j]
			name := opt
			if eq := strings.IndexByte(opt, '='); eq >= 0 {
				name = opt[:eq]
			} else if gitGlobalOptionsWithValue[opt] {
				j++ // step over the value so it is not read as another option
			}
			switch name {
			case "-C", "--git-dir", "--work-tree":
				return name, true
			}
		}
	}
	return "", false
}

// gitGlobalOptionsWithValue are git's global options that consume the word
// after them when they are written without `=`. `git -C ../elsewhere push` is
// three words before the subcommand, not one.
var gitGlobalOptionsWithValue = map[string]bool{
	"-C": true, "-c": true,
	"--exec-path": true, "--git-dir": true, "--work-tree": true,
	"--namespace": true, "--super-prefix": true, "--config-env": true,
	"--attr-source": true,
}

// isGitWord reports whether a word invokes git, by the name or by a path
// ending in it.
func isGitWord(word string) bool {
	if word == "git" || word == "git.exe" {
		return true
	}
	i := strings.LastIndexAny(word, `/\`)
	if i < 0 {
		return false
	}
	base := word[i+1:]
	return base == "git" || base == "git.exe"
}

// stripGitGlobalOptions rewrites every git invocation in the line so that the
// word after `git` is its subcommand.
//
// It is deliberately textual and deliberately greedy: it drops any option-like
// word between `git` and the first word that is not one, plus the value of the
// options that take a separate one. Over-stripping costs a false positive on a
// line that merely mentions git — the same trade the dequoted reading already
// makes, and the same safe direction — while under-stripping costs a force
// push that no rule saw.
func stripGitGlobalOptions(command string) string {
	fields := strings.Fields(command)
	out := make([]string, 0, len(fields))
	for i := 0; i < len(fields); i++ {
		out = append(out, fields[i])
		if !isGitWord(fields[i]) {
			continue
		}
		for i+1 < len(fields) && strings.HasPrefix(fields[i+1], "-") {
			opt := fields[i+1]
			i++
			// `--git-dir=x` carries its value; `--git-dir x` and `-C x` do not.
			if !strings.Contains(opt, "=") && gitGlobalOptionsWithValue[opt] && i+1 < len(fields) {
				i++
			}
		}
	}
	return strings.Join(out, " ")
}

func gitSafetyVariant(lowered, taskID string) (Decision, bool) {
	if strings.Contains(lowered, "--no-verify") || strings.Contains(lowered, "--no-gpg-sign") {
		return deny(RuleCommandBypassFlag, "Verification bypass flags are not allowed.", lowered, taskID), true
	}
	if hardResetProtectedRe.MatchString(lowered) {
		return deny(RuleCommandProtectedReset, "Protected branch hard resets are not allowed.", lowered, taskID), true
	}
	// The refspec form (`git push origin +HEAD:master`) forces a
	// non-fast-forward update without carrying --force.
	if forcePushFlagRe.MatchString(lowered) || forcePushPlusRefspecRe.MatchString(lowered) {
		return deny(RuleCommandForcePush, "Force pushes are not allowed.", lowered, taskID), true
	}
	if protectedBranchPushRe.MatchString(lowered) {
		return warn(RuleCommandProtectedPush,
			"Direct pushes to protected branches should go through verification gates.", lowered, taskID), true
	}
	return Decision{}, false
}

func (g CommandGate) noteHardRules(d Decision) Decision {
	if !g.HardRules {
		d.Degraded = append(d.Degraded, "policy.hard_rules.disabled")
	}
	return d
}

// liveSubstitutions returns the inner text of every command substitution sh
// would execute in this line: $( … ), a legacy backtick span, and the process
// substitutions <( … ) and >( … ).
//
// Substitutions inside single quotes are data; inside double quotes they are
// live, which is why the quote state is tracked here rather than delegated to
// a pre-pass. Arithmetic expansions `$(( … ))` are descended into rather than
// skipped: a substitution nested inside one executes, so its contents are
// scanned by this same function. An unterminated span is an error, not an
// empty list — a scanner that lost track of where code ends must refuse
// rather than guess.
func liveSubstitutions(command string) ([]string, error) {
	var spans []string
	runes := []rune(command)
	n := len(runes)
	quote := rune(0)
	i := 0
	for i < n {
		if next, nq, handled := shellQuoteStep(runes, i, quote); handled {
			quote = nq
			i = next
			continue
		}
		// Only the unquoted state and the "…" state reach here, and both are
		// live: sh executes $( … ) and ` … ` inside double quotes.
		r := runes[i]
		switch {
		case r == '`':
			text, next, err := scanBacktickSpan(runes, i)
			if err != nil {
				return nil, err
			}
			spans = append(spans, text)
			i = next
		case r == '$' && i+2 < n && runes[i+1] == '(' && runes[i+2] == '(':
			inner, next, ok := scanArithmetic(runes, i)
			if !ok {
				return nil, fmt.Errorf("unterminated arithmetic expansion")
			}
			// Descended into, not skipped: a substitution nested in here runs,
			// and one this scanner does not report is one the ladder never
			// judges. See scanArithmetic.
			nested, err := liveSubstitutions(inner)
			if err != nil {
				return nil, err
			}
			spans = append(spans, nested...)
			i = next
		case r == '$' && i+1 < n && runes[i+1] == '(':
			text, next, err := scanParenSpan(runes, i+1)
			if err != nil {
				return nil, err
			}
			spans = append(spans, text)
			i = next
		// Process substitution, unlike $( … ) and ` … `, is not performed
		// inside double quotes: `echo "<(echo hi)"` prints the text, and
		// `>"<(>0)"` creates a file actually named `<(>0)`. Both measured
		// against bash rather than recalled. Guarding on the unquoted state is
		// what keeps this scanner and redirectTargets describing the same
		// string — without it this half reported a live substitution where the
		// other half correctly saw a filename, and two answers about one line
		// is the defect this pairing exists to catch.
		case quote == 0 && (r == '<' || r == '>') && i+1 < n && runes[i+1] == '(':
			text, next, err := scanParenSpan(runes, i+1)
			if err != nil {
				return nil, err
			}
			spans = append(spans, text)
			i = next
		default:
			i++
		}
	}
	return spans, nil
}

// scanParenSpan reads from the opening parenthesis at position open to its
// matching close, honouring nested parentheses and quoted spans within the
// substitution. It returns the inner text and the index one past the closing
// parenthesis.
func scanParenSpan(runes []rune, open int) (string, int, error) {
	depth := 0
	quote := rune(0)
	for j := open; j < len(runes); j++ {
		r := runes[j]
		switch {
		case quote == '\'' && r == '\'':
			quote = 0
		case quote == '"' && r == '"':
			quote = 0
		case quote != 0:
		case r == '\'' || r == '"':
			quote = r
		case r == '(':
			depth++
		case r == ')':
			depth--
			if depth == 0 {
				return string(runes[open+1 : j]), j + 1, nil
			}
		}
	}
	return "", 0, fmt.Errorf("unterminated $( substitution")
}

// scanBacktickSpan reads from the opening backtick at position open to its
// matching close, honouring the backslash escapes sh allows inside a legacy
// substitution. It returns the inner text and the index one past the closing
// backtick. An unterminated span is an error: a scanner that lost track of
// where code ends must refuse rather than guess.
func scanBacktickSpan(runes []rune, open int) (string, int, error) {
	for j := open + 1; j < len(runes); j++ {
		switch runes[j] {
		case '\\':
			j++
		case '`':
			return string(runes[open+1 : j]), j + 1, nil
		}
	}
	return "", 0, fmt.Errorf("unterminated backtick substitution")
}

// scanArithmetic consumes a $(( … )) span. It returns the text between the
// opening `$((` and the closing `))`, the index one past the span, and whether
// the span closed at all; anything that does not close as arithmetic is
// reported so the caller can refuse rather than misparse.
//
// The inner text is returned rather than discarded, and that is the whole
// point of this function. Arithmetic is not the inert construct it looks like:
// `echo $(( $(touch f)0 ))` runs touch. A command substitution nested inside an
// arithmetic expansion executes exactly as it would outside one, and so does
// one in an array subscript — `$(( a[$(cmd)] ))`. Both measured against sh and
// bash rather than recalled. This scanner used to skip the whole span on the
// stated ground that arithmetic "cannot execute commands", which meant the
// ladder judged `echo` and never saw the touch: a command denied on its own
// was allowed by wrapping it in two parentheses.
//
// What arithmetic cannot do is reach code through a variable. `x='$(cmd)';
// echo $(( x ))` is a syntax error in sh and bash alike — operand expected —
// so every execution path in here is lexically present in the text, and
// descending into that text is sufficient. Refusing the construct outright,
// which is the other available answer, would break `$(( i + 1 ))` for no gain.
//
// Quotes are tracked while counting parentheses, because `$(( $(echo "(") ))`
// otherwise closes the span in the wrong place — and a span that ends in the
// wrong place hands its caller the wrong contents, which is worse than not
// looking at all.
func scanArithmetic(runes []rune, start int) (string, int, bool) {
	depth := 0
	quote := rune(0)
	for j := start; j < len(runes); j++ {
		r := runes[j]
		switch {
		case quote == '\'' && r == '\'':
			quote = 0
		case quote == '"' && r == '"':
			quote = 0
		case quote != 0:
		case r == '\'' || r == '"':
			quote = r
		case r == '(':
			depth++
		case r == ')':
			depth--
			if depth == 0 {
				// start+3 steps over `$((`; j-1 is the first of the two
				// closing parentheses. `$(())` has no inner text at all.
				if j-1 <= start+3 {
					return "", j + 1, true
				}
				return string(runes[start+3 : j-1]), j + 1, true
			}
		}
	}
	return "", 0, false
}

// hasHeredoc reports whether the command carries a heredoc introducer outside
// quotes. A heredoc body undergoes expansion yet has no statically checkable
// end — the terminator is whatever word the author chose on some following
// line — so no extraction is attempted; the construct is refused instead.
// Arithmetic contexts are skipped so `$((a << 2))` is not misread as one.
func hasHeredoc(command string) bool {
	runes := []rune(command)
	n := len(runes)
	quote := rune(0)
	i := 0
	for i < n {
		if next, nq, handled := shellQuoteStep(runes, i, quote); handled {
			quote = nq
			i = next
			continue
		}
		r := runes[i]
		if quote != 0 {
			// Only the "…" state reaches here; a << inside it is literal text.
			i++
			continue
		}
		switch {
		case r == '$' && i+2 < n && runes[i+1] == '(' && runes[i+2] == '(':
			// Skipped rather than descended into here, and only here: this
			// scanner asks whether a `<<` is a heredoc introducer or a shift
			// operator. A heredoc inside a nested substitution is reached when
			// the substitution rung extracts that text and evaluates it, which
			// runs this function again against it.
			_, next, ok := scanArithmetic(runes, i)
			if !ok {
				return false // malformed arithmetic; the substitution rung refuses it
			}
			i = next
		case r == '<' && i+1 < n && runes[i+1] == '<':
			return true
		default:
			i++
		}
	}
	return false
}

// reparsingCommandWord reports whether this single command's command word is a
// builtin that re-parses its arguments as shell code, and names it.
//
// Only the command word counts. `grep eval file` and `echo eval` mention the
// word without invoking it, and refusing those would make the rung fire on
// text rather than on behaviour — the exact defect the git-safety rules avoid
// by reading a dequoted variant rather than by matching substrings.
//
// The word is dequoted before comparison, because sh removes quotes before it
// decides what to run: `\eval`, `"eval"` and `'ev'al` all invoke the builtin.
// Leading VAR=value assignments are stepped over for the same reason — they
// precede the command word without being it.
func reparsingCommandWord(command string) (string, bool) {
	words := shellWords(command)
	for i := 0; i < len(words); i++ {
		word := words[i]
		if word.quotedHead {
			// The word begins inside quotes, so its first character is data
			// rather than syntax: `">"` is a filename and `"FOO=1"` is a
			// command named FOO=1, neither an operator nor an assignment. It
			// can still *be* the command word — sh runs `"eval"` and `\eval`
			// alike — so it is matched here before the scan stops.
			if reparsingBuiltins[word.text] {
				return word.text, true
			}
			return "", false
		}
		if isAssignmentWord(word.text) {
			continue
		}
		if operand, isRedirect := redirectionPrefix(word.text); isRedirect {
			// `> out eval …` puts the target in the next word; `2>out eval …`
			// carries it in this one. Stepping over the wrong number of words
			// is how `> out eval` read `>` as the command and stopped looking.
			if operand {
				i++
			}
			continue
		}
		// `command eval …` and `builtin eval …` reach the builtin through a
		// wrapper, so the search continues past them rather than stopping on a
		// word that is not itself the thing being run.
		if word.text == "command" || word.text == "builtin" {
			continue
		}
		if reparsingBuiltins[word.text] {
			return word.text, true
		}
		return "", false
	}
	return "", false
}

// redirectionPrefix reports whether a word is a redirection operator standing
// before the command word, and whether its target is a separate word.
func redirectionPrefix(word string) (targetIsNextWord, isRedirect bool) {
	i := 0
	for i < len(word) && word[i] >= '0' && word[i] <= '9' {
		i++
	}
	if i >= len(word) || (word[i] != '<' && word[i] != '>') {
		return false, false
	}
	for i < len(word) && (word[i] == '<' || word[i] == '>' || word[i] == '&' || word[i] == '|') {
		i++
	}
	return i == len(word), true
}

// shellWord is one word of a command line, with its quoting recorded.
type shellWord struct {
	text string
	// quotedHead reports whether the word's *first* character was produced by
	// quoting or by a backslash escape, which is the distinction sh itself
	// draws when deciding whether a word is syntax or data.
	//
	// It is the head specifically, not "any part quoted". `FOO="a b"` is an
	// assignment — the quoting is in the value — while `"FOO=1"` is a command
	// named FOO=1; `>` is an operator while `">"` is a filename. A flag set by
	// quoting anywhere in the word conflates those, and did: it read
	// `FOO="a b" eval …` as a quoted literal and stopped looking for the
	// command word.
	quotedHead bool
}

// shellWords splits a command line into the words sh would produce, honouring
// quotes and backslash escapes and performing no expansion.
//
// strings.Fields is not a substitute, and using it here was a defect this
// function exists to fix: it split `FOO="a b" eval "…"` into four pieces, the
// second of which (`b"`) is neither an assignment nor a command, so the scan
// concluded the command word was not eval and stopped. Quoting is precisely how
// a shell word holds a space, so a word splitter that does not read quotes is
// answering a different question from the one sh asks.
func shellWords(command string) []shellWord {
	var words []shellWord
	var cur strings.Builder
	started, quotedHead := false, false
	quote := rune(0)
	runes := []rune(command)
	for i := 0; i < len(runes); i++ {
		r := runes[i]
		switch {
		case quote == 0 && r == '\\' && i+1 < len(runes):
			quotedHead = quotedHead || !started
			cur.WriteRune(runes[i+1])
			i++
			started = true
		case quote == 0 && (r == '\'' || r == '"'):
			quotedHead = quotedHead || !started
			quote = r
			started = true
		case quote != 0 && r == quote:
			quote = 0
		case quote == '"' && r == '\\' && i+1 < len(runes) &&
			(runes[i+1] == '"' || runes[i+1] == '\\' || runes[i+1] == '$' || runes[i+1] == '`'):
			cur.WriteRune(runes[i+1])
			i++
		case quote != 0:
			cur.WriteRune(r)
		case r == ' ' || r == '\t' || r == '\n' || r == '\r':
			if started {
				words = append(words, shellWord{text: cur.String(), quotedHead: quotedHead})
				cur.Reset()
				started, quotedHead = false, false
			}
		default:
			cur.WriteRune(r)
			started = true
		}
	}
	if started {
		words = append(words, shellWord{text: cur.String(), quotedHead: quotedHead})
	}
	return words
}

// reparsingBuiltins are the command words whose arguments become shell code
// only at run time.
//
// `eval` is the whole set, and the boundary is deliberate. This rung refuses
// code that is *inline in the line being judged* — text the gate holds and
// cannot interpret. Running a script that lives on disk (`sh x.sh`, `make`,
// `pytest`) is a different problem and is not addressed here: the code is not
// in this string at all, so no reading of this string could catch it, and
// refusing the command words that do it would deny every test runner while
// leaving the capability one rename away. That boundary is documented in
// docs/POLICY_AND_SAFETY.md rather than left implicit here.
var reparsingBuiltins = map[string]bool{"eval": true}

// isAssignmentWord reports whether a word is a NAME=value prefix rather than
// the command word.
func isAssignmentWord(word string) bool {
	eq := strings.Index(word, "=")
	if eq <= 0 {
		return false
	}
	for i, r := range word[:eq] {
		if r == '_' || unicode.IsLetter(r) {
			continue
		}
		if i > 0 && unicode.IsDigit(r) {
			continue
		}
		return false
	}
	return true
}

// shellDequote removes quote characters the way sh concatenates their content:
// '…' contributes literally, "…" contributes everything except the quotes and
// escapes, and an escaped character contributes itself. An unterminated quote
// contributes the rest of the line as data. The result is what the shell would
// hand a single command after quote removal — the reading safety rules must
// also see, because quoting exists precisely to hide characters from naive
// substring checks.
func shellDequote(s string) string {
	var b strings.Builder
	runes := []rune(s)
	for i := 0; i < len(runes); i++ {
		r := runes[i]
		switch r {
		case '\\':
			if i+1 < len(runes) {
				b.WriteRune(runes[i+1])
				i++
			}
		case '\'':
			// Scan for the closing quote in rune space. IndexRune on a
			// re-encoded substring returns a BYTE offset, and mixing the two
			// desyncs on the first multibyte character — found by the chain
			// fuzzer as a slice-bounds panic on input like 'ααααααα'.
			close := -1
			for j := i + 1; j < len(runes); j++ {
				if runes[j] == '\'' {
					close = j
					break
				}
			}
			if close < 0 {
				b.WriteString(string(runes[i+1:]))
				return b.String()
			}
			b.WriteString(string(runes[i+1 : close]))
			i = close
		case '"':
			j := i + 1
			for ; j < len(runes); j++ {
				if runes[j] == '\\' && j+1 < len(runes) && (runes[j+1] == '"' || runes[j+1] == '\\') {
					b.WriteRune(runes[j+1])
					j++
					continue
				}
				if runes[j] == '"' {
					break
				}
				b.WriteRune(runes[j])
			}
			if j >= len(runes) {
				return b.String()
			}
			i = j
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

// clipForReason bounds model-authored text quoted into a decision's Reason,
// which travels into logs, transcripts and the TUI. Cut on a rune boundary.
func clipForReason(s string) string {
	const limit = 120
	if len(s) <= limit {
		return s
	}
	cut := limit
	for cut > 0 && !utf8.RuneStart(s[cut]) {
		cut--
	}
	return s[:cut] + "…"
}

// devToolDirs are the directory names a repo-local dev CLI is installed under.
// Matched exactly: a case-insensitive comparison let /tmp/x/BIN/dev through on
// a case-sensitive filesystem where that is a different directory entirely.
var devToolDirs = map[string]bool{"bin": true, "scripts": true, "Scripts": true}

// devVenvDirs are the virtualenv roots whose bin/ (Scripts/ on Windows) holds
// the installed `dev` console script.
var devVenvDirs = map[string]bool{".venv": true, "venv": true}

// NormalizeAllowlistCommand collapses the forms an allowlist would otherwise
// miss: path-prefixed dev binaries, `uv run --project X` wrappers, and trailing
// shell redirections.
//
// Normalisation is a claim that two spellings name the same program, and the
// executed string is the un-normalised one — so every spelling accepted here
// is a spelling that inherits `dev`'s allowlist entries. It used to accept any
// token whose basename folded to "dev" under a parent folding to "bin" or
// "scripts", which made `/tmp/attacker/bin/dev status`,
// `../../../../tmp/attacker/bin/dev status`, `attacker/scripts/dev run-cmd …`
// and `/tmp/x/bin/DEV status` all read as the repo's own CLI. What is accepted
// now is only what the harness can attribute to this working tree:
//
//   - a bare `dev`/`devcouncil` token resolved off PATH;
//   - a relative `bin/dev` or `scripts/dev` directly beneath the working
//     directory — the repo's own tool directories;
//   - a `…/.venv/bin/dev` (or venv/, or Scripts/ on Windows) layout, the shape
//     a project virtualenv install actually produces.
//
// Anything with a `..` component is refused outright: a path that can climb out
// of the tree cannot be attributed to it. A backslash is likewise not treated
// as a separator — the gate hands these lines to `sh`, where `\` escapes the
// next character rather than descending a directory, so reading `x\bin\dev` as
// a path laundered a token sh never resolves as `dev` at all.
//
// An ABSOLUTE `…/.venv/bin/dev` is the one spelling that cannot be judged from
// the command alone: `/repo/.venv/bin/dev` and `/tmp/attacker/.venv/bin/dev`
// have the same shape, and only one of them is this tree's. That is what
// NormalizeAllowlistCommandInRoot's root argument decides. This entry point
// keeps the old signature for callers with no root to give, and answers the
// question the only way an unknown root permits: no absolute path is this
// repository's, so none is laundered into a bare `dev`. Fail closed — a
// spelling that is refused here is a spelling that must be spelled out in the
// allowlist, which is a nuisance; a spelling wrongly accepted here is a
// foreign binary inheriting `dev`'s entries, which is not.
func NormalizeAllowlistCommand(command string) string {
	return NormalizeAllowlistCommandInRoot(command, "")
}

// NormalizeAllowlistCommandInRoot is NormalizeAllowlistCommand with the working
// tree named, so an absolute path to this repository's own virtualenv `dev` is
// distinguishable from a foreign one at the same layout.
//
// root must be absolute; anything else is treated as no root at all, because a
// relative root cannot decide containment for an absolute token and guessing
// from the process's cwd would make the answer depend on where the harness
// happened to be started.
func NormalizeAllowlistCommandInRoot(command, root string) string {
	normalized := collapseSpaces(command)
	if normalized == "" {
		return normalized
	}
	for {
		stripped := redirectTailRe.ReplaceAllString(normalized, "")
		if stripped == normalized {
			break
		}
		normalized = stripped
	}
	if m := uvRunDirFlagRe.FindStringSubmatch(normalized); m != nil {
		normalized = m[1] + m[3]
	}

	tokens := strings.Fields(normalized)
	for i, token := range tokens {
		if isRepoDevBinary(token, root) {
			tokens[i] = "dev"
		}
	}
	return strings.Join(tokens, " ")
}

// insideRoot reports whether an absolute token names a path strictly beneath an
// absolute root.
//
// Slash semantics rather than filepath's, deliberately: these strings are
// handed to `sh`, which resolves "/" and nothing else, so judging them by the
// host platform's separator would judge a path the shell will never walk. A
// token equal to the root is not inside it — the root is a directory, not a
// program.
func insideRoot(root, token string) bool {
	if !strings.HasPrefix(root, "/") || !strings.HasPrefix(token, "/") {
		return false
	}
	// path.Clean cannot climb out here: isRepoDevBinary has already refused
	// any token carrying a ".." component.
	cleanRoot := path.Clean(root)
	cleanToken := path.Clean(token)
	if cleanRoot == "/" {
		return cleanToken != "/"
	}
	return strings.HasPrefix(cleanToken, cleanRoot+"/")
}

// isRepoDevBinary reports whether a token names this repo's own dev CLI in a
// spelling the gate can attribute to the working tree. See
// NormalizeAllowlistCommand for why the answer has to be this narrow.
func isRepoDevBinary(token, root string) bool {
	if strings.ContainsRune(token, '\\') {
		return false
	}
	parts := strings.Split(token, "/")
	absolute := parts[0] == "" && len(parts) > 1
	var comps []string
	for _, p := range parts {
		switch p {
		case "..":
			// The path can leave the working tree, so it is not this repo's.
			return false
		case "", ".":
			// Empty from a leading, trailing or doubled separator; "." is the
			// working directory itself.
		default:
			comps = append(comps, p)
		}
	}
	if len(comps) == 0 || !devBinaries[comps[len(comps)-1]] {
		return false
	}
	switch {
	case len(comps) == 1:
		// A bare name resolved off PATH — but "/dev" is the device directory,
		// not a program.
		return !absolute
	case len(comps) == 2:
		return !absolute && devToolDirs[comps[0]]
	default:
		if !devVenvDirs[comps[len(comps)-3]] || !devToolDirs[comps[len(comps)-2]] {
			return false
		}
		if !absolute {
			// Relative, no "..": it resolves beneath the working directory,
			// which is this tree.
			return true
		}
		// Absolute, so the layout alone proves nothing — /tmp/attacker/.venv/
		// bin/dev has exactly the shape a project virtualenv produces. Only
		// containment in the tree this gate was built for makes it this
		// repository's, and an unknown root cannot establish that.
		return insideRoot(root, token)
	}
}

func matchesEither(mt matching, patterns []string, normalized, raw string) bool {
	// Two questions, one per form, instead of two per pattern: "some pattern
	// matches either form" is the same OR.
	return mt.any(patterns, normalized) || mt.any(patterns, raw)
}

func collapseSpaces(s string) string { return strings.Join(strings.Fields(s), " ") }
