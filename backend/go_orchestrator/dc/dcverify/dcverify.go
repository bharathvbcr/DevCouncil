// Package dcverify is the harness's rigor boundary.
//
// It does not implement the gates. `dcverify` already parses the unified diff,
// classifies it against planned scope, scans added lines for secrets, detects
// stubs, and intersects added lines with a coverage profile; a second
// implementation here would be a second set of verdicts to keep in step with
// the first. This package execs that binary and reads its JSON, exactly as the
// store and search boundaries do, and for the same reason: a process keeps
// CGO_ENABLED=0 and the single static binary intact, and a diff that makes the
// parser misbehave cannot take the agent loop with it.
//
// What this package owns is the part a verifier must not get wrong, which is
// refusing to report a gate it did not run:
//
//   - A missing binary is an error naming what to build, never a clean
//     `findings: []`. An empty finding list means "these gates ran and found
//     nothing", and a verifier that records that for gates which never ran has
//     turned "unexamined" into "approved" — which is the whole failure the
//     rigor layer exists to catch, committed by the layer itself.
//
//   - Coverage is measured only when a profile was supplied. Without one the
//     binary still answers, with every changed file in `coverage_unmeasured`
//     and `coverage_gaps` empty — which is byte-for-byte what a fully exercised
//     diff looks like to a caller that reads only the gaps. So Measured is set
//     from the request rather than off the wire, and a reply carrying gaps
//     nobody could have measured is refused instead of believed.
//
//   - A finding naming a gate this client cannot map is refused rather than
//     dropped. A caller turns gates into gap types; an unmapped one is a
//     blocking finding that disappears between the two processes.
//
//   - A binary speaking a different schema is refused rather than decoded
//     through the wrong shape.
package dcverify

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// BinaryEnv overrides binary discovery, so an operator who names a path means
// it. It is declared here rather than at the call site so the message that
// names the remedy and the code that reads it cannot drift apart.
const BinaryEnv = "MANVI_VERIFY_BINARY"

// SchemaVersion must match dcverify's `health.schema_version`. A mismatch is
// refused rather than decoded through the wrong shape.
const SchemaVersion = 1

// identity is how the binary names itself to a health probe. A binary that
// answers with anything else is some other program that happens to sit at the
// configured path — dcstore's health names itself under "store" and this one
// under "verifier", so a probe of the wrong path would otherwise read a
// healthy reply from the wrong process.
const identity = "dc-verify"

// The gates a reply may attribute a finding to. Both are spelled by
// dc-verify/src/rigor.rs and matched exactly: a gate this client does not know
// is refused in validate rather than silently carried to a caller that has no
// gap type for it.
const (
	GateStubDetection = "stub_detection"
	GateSecretScan    = "secret_scan"
)

// GateDiffCoverage names the coverage half of the rigor layer. It is not a
// findings gate — coverage arrives as its own arrays rather than as findings —
// so it is spelled here for the caller that reports which gates ran, and never
// compared against a finding's gate field.
const GateDiffCoverage = "diff_coverage"

// GateSubstance names the informativeness measurement. Like GateDiffCoverage it
// is not a findings gate — it arrives as its own object — so it is spelled here
// for the caller that reports which gates ran, and never compared against a
// finding's gate field.
const GateSubstance = "substance"

// The severities a finding may carry, spelled by dc-verify's Severity enum.
// Anything else is refused: Go would decode an unknown string without
// complaint, and the caller's "is this blocking" test would then answer false
// for a finding the verifier meant to block on.
const (
	SeverityBlocking = "blocking"
	SeverityAdvisory = "advisory"
)

// The strengths a finding may carry, spelled by dc-verify's Strength enum.
//
// Severity says how much a finding matters; strength says how much it can be
// trusted. Both are refused when unrecognised, and for the same reason: Go
// would decode an unknown string without complaint, and a consumer weighing a
// blocking finding would then read an unrecognised strength as the empty
// string rather than learning that the verifier speaks a vocabulary this
// client does not.
//
// StrengthDerived is the one every findings gate currently reports —
// `secret_scan` matches a credential's shape and `stub_detection` matches text,
// and neither is a measurement. That is a true and slightly uncomfortable
// description of the rigor layer, which is the point of putting it on the wire.
const (
	// StrengthProven follows from the parsed structure of the diff.
	StrengthProven = "proven"
	// StrengthObserved is read from an execution artifact the caller supplied.
	StrengthObserved = "observed"
	// StrengthDerived is a textual pattern that correlates with the thing
	// being looked for, and can match something else.
	StrengthDerived = "derived"
)

const (
	// defaultTimeout bounds one check. The work is linear in the diff plus the
	// coverage profile — a scan of added lines and a binary search per line —
	// so a large change is seconds; anything past this is a wedged child rather
	// than a slow one.
	defaultTimeout = 60 * time.Second

	// maxOutput bounds the reply. A legitimate result is the findings for one
	// diff, which is kilobytes; this is the bound on a child gone wrong,
	// applied during the copy rather than checked after it so a runaway cannot
	// allocate the whole thing before anyone looks.
	maxOutput = 16 << 20

	// maxStderr bounds the diagnostic half. It exists to make a failure
	// reportable, not to capture a log.
	maxStderr = 64 << 10

	// waitDelay matches the other process boundaries. Killing a child does not
	// unblock Wait while something still holds its stdout pipe.
	waitDelay = 2 * time.Second
)

// Client runs the dcverify binary against one diff.
type Client struct {
	// Binary is the path to `dcverify`.
	Binary string
	// Root is the repository the diff came from. Coverage profiles carrying
	// absolute paths are reduced against it; without it the binary uses its own
	// working directory, which a host driving it from elsewhere cannot rely on.
	Root string
	// Timeout bounds one invocation. Zero means defaultTimeout.
	Timeout time.Duration

	// maxOutput and maxStderr bound what one invocation may hand back, in
	// bytes. They are fields rather than the constants they shadow so a test
	// can drive the bound without generating megabytes to reach it — the same
	// reason the search and devmap boundaries carry them as fields. Nothing
	// outside this package sets them.
	maxOutput int
	maxStderr int
}

// New builds a client with defaults.
func New(binary, root string) *Client {
	return &Client{
		Binary: binary, Root: root, Timeout: defaultTimeout,
		maxOutput: maxOutput, maxStderr: maxStderr,
	}
}

// Request is one rigor check.
type Request struct {
	// Diff is the unified diff, handed to the binary on stdin.
	Diff string
	// Planned is the task's planned file paths. Scope classification splits the
	// diff against them: a changed file matching none of them is an orphan, and
	// a planned path no file matched is untouched.
	Planned []string
	// CoveragePath is an LCOV profile for the same change. Empty means the
	// coverage gate does not run — see Coverage.Measured for why that is not
	// the same as a diff with no coverage gaps.
	CoveragePath string
}

// Finding is one rigor finding: a gate, where it fired, and what it saw.
type Finding struct {
	Gate     string `json:"gate"`
	Severity string `json:"severity"`
	// Strength is how the gate knows: see the Strength* constants. A verifier
	// predating the field leaves it empty, which validate refuses — an
	// unlabelled finding would otherwise be indistinguishable from a proven
	// one to any caller that reads the field at all.
	Strength string `json:"strength"`
	Path     string `json:"path"`
	Line     int    `json:"line"`
	// Evidence is what triggered the finding, already truncated by the verifier
	// and — for a secret finding — already redacted there. It is carried
	// verbatim rather than re-derived here: re-reading the line to quote it
	// would put the unredacted secret back into the report.
	Evidence string `json:"evidence"`
	Message  string `json:"message"`
}

// Blocking reports whether this finding is one the verifier meant to block on.
//
// It reads the severity the gate assigned rather than inferring one from the
// gate name, because the assignment is the verifier's to make: dc-verify scales
// strictness by what the gate found, and a client that decided for itself would
// be a second opinion nobody asked for.
func (f Finding) Blocking() bool { return f.Severity == SeverityBlocking }

// CoverageGap is one changed file whose added lines were not all executed.
type CoverageGap struct {
	Path string `json:"path"`
	// AddedLines is how many lines the diff added to this file, so a caller can
	// report the gap as a proportion rather than as a bare count.
	AddedLines     int   `json:"added_lines"`
	UncoveredLines []int `json:"uncovered_lines"`
}

// Coverage is the diff-versus-coverage answer, with the one fact that decides
// whether any of it means anything attached to it.
type Coverage struct {
	// Measured reports whether a coverage profile was supplied at all. It is
	// set by the client from the request and never decoded from the wire: with
	// no profile the binary answers with empty Gaps, which is indistinguishable
	// from a fully exercised diff for anyone reading Gaps alone.
	Measured bool
	// Gaps are the files with unexecuted added lines. Meaningful only when
	// Measured; empty and meaningless otherwise.
	Gaps []CoverageGap
	// Unmeasured are executable changed files the profile said nothing about.
	// When Measured is false this is every executable changed file, which is
	// the honest reading of "the gate did not run".
	Unmeasured []string
	// SkippedByType are changed files coverage does not apply to at all — a
	// Markdown file has no executable lines. Reported rather than dropped,
	// because a file the gate never asked about otherwise reads exactly like a
	// file that was measured and clean.
	SkippedByType []string
}

// FileSubstance is one file's share of the substance measurement.
type FileSubstance struct {
	Path             string `json:"path"`
	AddedLines       int    `json:"added_lines"`
	SubstantiveLines int    `json:"substantive_lines"`
}

// Substance is how much of the diff is new work.
//
// Every other answer here is about something being *wrong* with a change. This
// one is about there being anything in it: a diff of relocated functions, brace
// lines and a regenerated lockfile trips no gate, produces no coverage gap
// because moved code carries its moved tests, and arrives as `findings: []` —
// which a caller reasonably reads as work having been done.
//
// It is a measurement and never a finding, and it has no Measured flag beside
// it because there is no state in which it could not run: it needs only the
// diff every other gate already read. Judged is the analogous distinction and a
// weaker one — not "the gate did not run" but "the diff is too small for the
// ratio to say anything", which is a fact about the input rather than about the
// verifier.
//
// Low is decided by dc-verify, not here. The threshold is calibrated against
// real history in the crate that owns the classification, and a second copy of
// it on this side would be a second policy that disagrees the first time either
// moves.
type Substance struct {
	AddedLines       int `json:"added_lines"`
	SubstantiveLines int `json:"substantive_lines"`
	Trivial          int `json:"trivial"`
	Moved            int `json:"moved"`
	Repeated         int `json:"repeated"`
	Generated        int `json:"generated"`
	// Judged reports whether the diff had enough added lines for the ratio to
	// carry information. False means this measurement has nothing to say, which
	// is not the same as saying the change is fine.
	Judged bool `json:"judged"`
	// Low reports whether a judged diff fell under the verifier's threshold.
	// Always false when Judged is false.
	Low   bool            `json:"low"`
	Files []FileSubstance `json:"files"`
}

// Result is what the verifier returned.
type Result struct {
	OK    bool   `json:"ok"`
	Error string `json:"error"`

	// Files is how many files the diff touched.
	Files int `json:"files"`
	// InScope and Orphans partition those files against Request.Planned, and
	// UntouchedPlanned is the planned paths no file matched.
	InScope          []string `json:"in_scope"`
	Orphans          []string `json:"orphans"`
	UntouchedPlanned []string `json:"untouched_planned"`

	Findings []Finding `json:"findings"`

	// Substance is the informativeness measurement. Unlike the coverage
	// arrays it is safe to read straight off the wire: it carries its own
	// Judged flag, so there is no "supplied by the caller" fact this client
	// has to attach before it can be acted on.
	Substance Substance `json:"substance"`

	// The coverage arrays as they arrive. Callers read Coverage instead, which
	// carries the same data with the measured/unmeasured distinction that makes
	// it safe to act on; these stay unexported-by-intent behind that accessor.
	coverageGaps          []CoverageGap
	coverageUnmeasured    []string
	coverageSkippedByType []string
	coverageMeasured      bool
}

// wireResult is the reply exactly as dcverify prints it.
//
// It is separate from Result so the coverage fields cannot be read off the wire
// by a caller: they reach Result only through Check, which is the one place
// that knows whether a profile was supplied.
type wireResult struct {
	OK                    bool          `json:"ok"`
	Error                 string        `json:"error"`
	Files                 int           `json:"files"`
	InScope               []string      `json:"in_scope"`
	Orphans               []string      `json:"orphans"`
	UntouchedPlanned      []string      `json:"untouched_planned"`
	Findings              []Finding     `json:"findings"`
	CoverageUnmeasured    []string      `json:"coverage_unmeasured"`
	CoverageGaps          []CoverageGap `json:"coverage_gaps"`
	CoverageSkippedByType []string      `json:"coverage_skipped_by_type"`
	// A pointer so its ABSENCE is detectable. A verifier built before the
	// substance gate existed emits no `substance` key, which decodes into a
	// zero value — nought added lines, judged false — and that is
	// indistinguishable from a real measurement of a diff too small to judge.
	// A measurement that never ran must not read as one that ran and declined
	// to conclude, so nil is refused in validate rather than defaulted.
	//
	// This is why the schema version does not move. A version bump would
	// refuse an old binary too, but it would equally refuse a *new* binary
	// paired with an older host for a field that host never reads, which is a
	// cost the additive rule exists to avoid. Detecting the absent key is
	// exact, and it fails in only the direction that is actually unsafe.
	Substance *Substance `json:"substance"`
}

// Coverage returns the coverage answer together with whether it was measured.
func (r *Result) Coverage() Coverage {
	return Coverage{
		Measured:      r.coverageMeasured,
		Gaps:          r.coverageGaps,
		Unmeasured:    r.coverageUnmeasured,
		SkippedByType: r.coverageSkippedByType,
	}
}

// GatesRun names the gates this invocation actually performed.
//
// The two findings gates are unconditional in `dcverify check`, which calls
// scan_secrets and then detect_stubs on every parsed diff — see
// rust/dc-verify/src/bin/dcverify.rs. That is a claim about another program in
// another language, so it is not left to this comment: the interop test plants
// a stub and a secret in one diff and fails if either gate stops reporting.
//
// Coverage is the conditional one, and it is named here only when a profile
// was supplied. A caller records this list as the rigor it applied, so a gate
// named here that did not run is a gate reported as passed without examining
// anything.
func (r *Result) GatesRun() []string {
	// Substance is unconditional: it reads the diff the other gates already
	// read and needs nothing from the caller, so unlike coverage there is no
	// state in which it was configured away. It is named here so a report that
	// lists the gates it applied does not leave out the one measurement that
	// ran on every pass.
	gates := []string{GateSecretScan, GateStubDetection, GateSubstance}
	if r.coverageMeasured {
		gates = append(gates, GateDiffCoverage)
	}
	return gates
}

// ErrNoBinary is returned when no verifier is configured or none can be found.
// Callers turn it into a message naming the remedy rather than into an empty
// finding list.
var ErrNoBinary = errors.New("no dcverify binary configured")

// Check runs the rigor gates over one diff.
//
// Every error here names a fault, and none of them is reachable by a diff that
// simply earned no findings: that is a Result with an empty Findings slice.
func (c *Client) Check(ctx context.Context, req Request) (*Result, error) {
	if c == nil || c.Binary == "" {
		return nil, ErrNoBinary
	}

	args := []string{"check"}
	if len(req.Planned) > 0 {
		encoded, err := encodePlanned(req.Planned)
		if err != nil {
			return nil, err
		}
		args = append(args, "--planned", encoded)
	}
	if req.CoveragePath != "" {
		args = append(args, "--coverage", req.CoveragePath)
	}
	if c.Root != "" {
		args = append(args, "--root", c.Root)
	}

	var wire wireResult
	if err := c.run(ctx, args, req.Diff, &wire); err != nil {
		return nil, err
	}
	out := &Result{
		OK:                    wire.OK,
		Error:                 wire.Error,
		Files:                 wire.Files,
		InScope:               wire.InScope,
		Orphans:               wire.Orphans,
		UntouchedPlanned:      wire.UntouchedPlanned,
		Findings:              wire.Findings,
		coverageGaps:          wire.CoverageGaps,
		coverageUnmeasured:    wire.CoverageUnmeasured,
		coverageSkippedByType: wire.CoverageSkippedByType,
		coverageMeasured:      req.CoveragePath != "",
	}
	if wire.Substance == nil {
		return nil, fmt.Errorf(
			"the verifier at %s returned no substance measurement; it predates the "+
				"substance gate. Rebuild it from this tree (`cargo build -p dc-verify "+
				"--bin dcverify`) or point %s at one that has it — an absent "+
				"measurement must not be read as a diff too small to measure",
			c.Binary, BinaryEnv)
	}
	out.Substance = *wire.Substance
	if err := validate(out); err != nil {
		return nil, err
	}
	return out, nil
}

// encodePlanned renders the planned paths for the `--planned` flag, which
// separates them by newline.
//
// A path containing a newline cannot be represented: the far side would split
// it into two patterns, and a file the task planned would be classified as an
// orphan of a path that names nothing. Unix filenames may contain newlines, so
// this is refused at the boundary rather than assumed away — the whole point of
// the newline-separated encoding is that a path with a comma or a space
// survives, and silently mangling the one case it cannot carry would undo that.
func encodePlanned(planned []string) (string, error) {
	for _, path := range planned {
		if strings.ContainsAny(path, "\n\r") {
			return "", fmt.Errorf(
				"planned path %q contains a line break, which the verifier's path list cannot carry",
				path)
		}
	}
	return strings.Join(planned, "\n"), nil
}

// run is the one place this package execs the verifier.
//
// Both operations go through it so the bound, the caps, the group isolation
// and — most of all — the refusal to read a reply as an answer unless it says
// ok cannot be present at one call site and missing at the other.
func (c *Client) run(ctx context.Context, args []string, stdin string, out any) error {
	timeout := c.Timeout
	if timeout <= 0 {
		timeout = defaultTimeout
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	// #nosec G204 -- c.Binary is the verifier this harness configured, and the
	// arguments are literals from this file plus paths the caller supplied as
	// flag *values*. None of them is a command, and none reaches here from a
	// model.
	cmd := exec.CommandContext(ctx, c.Binary, args...)
	// See proc.ConfigureGroup. The verifier spawns nothing today, which is
	// exactly the argument that was made at the boundaries where a grandchild
	// later appeared and held the pipe open past the deadline.
	proc.ConfigureGroup(cmd)
	cmd.Stdin = strings.NewReader(stdin)
	stdout := &cappedBuffer{limit: c.outputBound()}
	stderr := &cappedBuffer{limit: c.stderrBound()}
	cmd.Stdout = stdout
	cmd.Stderr = stderr
	cmd.WaitDelay = waitDelay

	runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		// RunBounded abandons the goroutine on a deadline, so the buffers may
		// still be written. Nothing below reads them on this path.
		return fmt.Errorf("verifier timed out after %s", timeout)
	}
	if stdout.overflow {
		// Refused rather than decoded from the prefix. A truncated reply can
		// still be valid JSON — the findings array simply ends early — and
		// accepting it would report a capped sample of the gates' findings as
		// everything they found.
		return fmt.Errorf("verifier produced more than %d bytes", c.outputBound())
	}

	if err := json.Unmarshal(bytes.TrimSpace(stdout.buf.Bytes()), out); err != nil {
		if runErr != nil {
			return fmt.Errorf("verifier failed: %w (stderr: %s)", runErr, bytes.TrimSpace(stderr.buf.Bytes()))
		}
		return fmt.Errorf("verifier returned unparseable output: %w (%q)", err, stdout.buf.String())
	}
	// The OK flag is the only thing separating a real clean answer from a reply
	// that decoded into a zero value — `null` and `{}` both unmarshal without
	// error, leaving every field at zero and the finding list empty.
	if ok, reason := okOf(out); !ok {
		if reason == "" {
			reason = "no reason given"
		}
		return errors.New(reason)
	}
	return nil
}

// validate refuses a reply that said ok but cannot be believed.
//
// Everything below is a property the caller is about to rely on, checked here
// rather than assumed because the thing that produced it is a separate program
// in a different language that this build does not compile. Each is a way for a
// misbehaving verifier to produce a report that reads as rigor applied:
//
//   - A gate name this client cannot map. The caller turns gates into gap
//     types; an unmapped gate is a blocking finding that reaches no gap.
//   - A severity outside the two the enum has. Go decodes any string into the
//     field, and `Blocking()` then answers false for a finding meant to block.
//   - A file count that does not match the scope split, since classify_scope
//     puts every file in exactly one of in_scope and orphans. A mismatch means
//     the reply is about a different set of files than the one it counted.
//   - Coverage gaps with no profile supplied, which is a measurement nobody
//     could have made.
//   - A coverage gap reporting more uncovered lines than the file had added, or
//     none at all, which makes the ratio a caller enforces on meaningless.
//   - An absolute or climbing path presented as repository contents. The
//     verifier does not emit these; that is exactly why the client must not
//     depend on it not emitting them. Containment asserted at one end only is
//     containment that a change at the other end silently removes.
func validate(out *Result) error {
	for _, finding := range out.Findings {
		switch finding.Gate {
		case GateStubDetection, GateSecretScan:
		default:
			return fmt.Errorf(
				"verifier reported a finding from gate %q, which this client cannot map to a gap; "+
					"a gate added there must be added here in the same change or its findings vanish",
				finding.Gate)
		}
		switch finding.Severity {
		case SeverityBlocking, SeverityAdvisory:
		default:
			return fmt.Errorf("finding from %s carries severity %q, which is neither %q nor %q",
				finding.Gate, finding.Severity, SeverityBlocking, SeverityAdvisory)
		}
		switch finding.Strength {
		case StrengthProven, StrengthObserved, StrengthDerived:
		default:
			return fmt.Errorf(
				"finding from %s carries strength %q, which is none of %q, %q, %q; "+
					"an unlabelled finding reads as a measurement to any caller that "+
					"weighs the field",
				finding.Gate, finding.Strength,
				StrengthProven, StrengthObserved, StrengthDerived)
		}
		if err := validPath(finding.Path); err != nil {
			return fmt.Errorf("finding path: %w", err)
		}
		if finding.Line < 1 {
			return fmt.Errorf("finding in %s carries line number %d, which names no line",
				finding.Path, finding.Line)
		}
	}

	if out.Files < 0 {
		return fmt.Errorf("verifier reported %d files, which is not a count", out.Files)
	}

	// The substance classes must partition the added lines. Checked here as
	// well as in the crate that computes them, because this is the boundary
	// between two processes: a verifier from a different build, or one whose
	// classifier gained a class this client does not know about, would
	// otherwise hand over a ratio whose denominator does not match its parts
	// and every number derived from it would be quietly wrong.
	s := out.Substance
	if s.AddedLines < 0 || s.SubstantiveLines < 0 {
		return fmt.Errorf("substance reports %d added and %d substantive lines, which are not counts",
			s.AddedLines, s.SubstantiveLines)
	}
	if sum := s.SubstantiveLines + s.Trivial + s.Moved + s.Repeated + s.Generated; sum != s.AddedLines {
		return fmt.Errorf(
			"substance classes sum to %d but the diff added %d lines "+
				"(substantive %d, trivial %d, moved %d, repeated %d, generated %d); "+
				"every added line must fall in exactly one class or the ratio means nothing",
			sum, s.AddedLines, s.SubstantiveLines, s.Trivial, s.Moved, s.Repeated, s.Generated)
	}
	if s.Low && !s.Judged {
		return fmt.Errorf(
			"substance reports low on an unjudged diff; a diff too small to judge " +
				"has no ratio to be under a threshold")
	}
	for _, f := range s.Files {
		if err := validPath(f.Path); err != nil {
			return fmt.Errorf("substance path: %w", err)
		}
		if f.SubstantiveLines > f.AddedLines {
			return fmt.Errorf("substance reports %d substantive of %d added lines in %s",
				f.SubstantiveLines, f.AddedLines, f.Path)
		}
	}
	if total := len(out.InScope) + len(out.Orphans); total != out.Files {
		return fmt.Errorf(
			"verifier counted %d files but classified %d (%d in scope, %d orphaned); "+
				"every changed file belongs to exactly one of the two",
			out.Files, total, len(out.InScope), len(out.Orphans))
	}
	for label, paths := range map[string][]string{
		"in-scope":          out.InScope,
		"orphan":            out.Orphans,
		"unmeasured":        out.coverageUnmeasured,
		"type-skipped":      out.coverageSkippedByType,
		"untouched planned": out.UntouchedPlanned,
	} {
		for _, path := range paths {
			if err := validPath(path); err != nil {
				return fmt.Errorf("%s path: %w", label, err)
			}
		}
	}

	if !out.coverageMeasured && len(out.coverageGaps) > 0 {
		return fmt.Errorf(
			"verifier reported %d coverage gaps without a coverage profile; nothing was "+
				"measured, so nothing could be found uncovered", len(out.coverageGaps))
	}
	for _, gap := range out.coverageGaps {
		if err := validPath(gap.Path); err != nil {
			return fmt.Errorf("coverage gap path: %w", err)
		}
		if gap.AddedLines < 1 {
			return fmt.Errorf("coverage gap in %s reports %d added lines, so there was nothing to cover",
				gap.Path, gap.AddedLines)
		}
		if len(gap.UncoveredLines) == 0 {
			return fmt.Errorf("coverage gap in %s lists no uncovered line, so it is not a gap", gap.Path)
		}
		if len(gap.UncoveredLines) > gap.AddedLines {
			return fmt.Errorf(
				"coverage gap in %s reports %d uncovered lines out of %d added; a covered "+
					"proportion above one is not a measurement",
				gap.Path, len(gap.UncoveredLines), gap.AddedLines)
		}
		for _, line := range gap.UncoveredLines {
			if line < 1 {
				return fmt.Errorf("coverage gap in %s names line %d, which names no line", gap.Path, line)
			}
		}
	}
	return nil
}

// validPath refuses anything that is not a repository-relative file path.
//
// The rule is the same one the search boundary applies, asserted again here
// because this is where bytes from another process become a path a caller will
// open, quote in a gap, or hand to a repair loop.
//
// A thin adapter over safefile.ValidRepoPath, which owns the rule — see the
// twin of this comment in dc/dcgrep. "The same rule as the search boundary" is
// now true by construction rather than by two copies happening to agree.
func validPath(path string) error {
	return safefile.ValidRepoPath(path)
}

// okOf reads the ok flag and error text off whichever reply shape was decoded.
//
// A type switch rather than reflection or an interface on the wire structs:
// there are two shapes, both defined in this file, and a switch that stops
// compiling when a third is added is the behaviour wanted. Silently returning
// true for an unknown shape is the one thing this must not do.
func okOf(out any) (bool, string) {
	switch reply := out.(type) {
	case *wireResult:
		return reply.OK, reply.Error
	case *healthReply:
		return reply.OK, reply.Error
	default:
		return false, "the client decoded a reply shape it does not know how to check"
	}
}

// healthReply is the handshake a caller probes the binary with.
type healthReply struct {
	OK    bool   `json:"ok"`
	Error string `json:"error"`
	// Verifier names the binary that answered.
	Verifier string `json:"verifier"`
	// SchemaVersion is a pointer so an absent key is distinguishable from a
	// zero. A version that silently decodes to 0 would read as "older than
	// anything" and send this client down a compatibility path nobody chose.
	SchemaVersion *int `json:"schema_version"`
}

// Available reports whether the verifier can be reached, by asking it to
// identify itself rather than by the absence of an error.
//
// It does not check `evidence_schema_versions`. That field advertises the
// evidence-bundle protocol, which this client does not speak — it drives
// `check` only — and asserting a version nothing here reads would be a
// compatibility claim with no code behind it. The binary's own interop test
// holds that field to protocols/evidence/v1.md.
func (c *Client) Available(ctx context.Context) error {
	if c == nil || c.Binary == "" {
		return ErrNoBinary
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()

	var health healthReply
	if err := c.run(ctx, []string{"health"}, "", &health); err != nil {
		return err
	}
	if health.Verifier != identity {
		return fmt.Errorf("%s identified itself as %q, not %q", c.Binary, health.Verifier, identity)
	}
	if health.SchemaVersion == nil {
		return fmt.Errorf("%s reported no schema_version, so this harness cannot tell whether it "+
			"speaks schema %d", c.Binary, SchemaVersion)
	}
	if *health.SchemaVersion != SchemaVersion {
		return fmt.Errorf("%s speaks schema %d, this harness speaks %d",
			c.Binary, *health.SchemaVersion, SchemaVersion)
	}
	return nil
}

// outputBound and stderrBound resolve the per-invocation caps, so a zero-valued
// Client built by a caller that did not go through New is bounded rather than
// unbounded. A cap that defaults to "none" is the failure the cap exists for.
func (c *Client) outputBound() int {
	if c.maxOutput <= 0 {
		return maxOutput
	}
	return c.maxOutput
}

func (c *Client) stderrBound() int {
	if c.maxStderr <= 0 {
		return maxStderr
	}
	return c.maxStderr
}

// cappedBuffer forwards at most limit bytes and records that it stopped.
//
// Capped during the copy rather than checked after it: a child gone rogue on a
// large diff would otherwise allocate the whole reply before the bound was ever
// consulted. The write is reported as complete so io.Copy does not turn a cap
// into io.ErrShortWrite, close the pipe, and hand the child a SIGPIPE — which
// is how a check that ran fine comes back as a failure.
type cappedBuffer struct {
	buf      bytes.Buffer
	limit    int
	overflow bool
}

func (c *cappedBuffer) Write(p []byte) (int, error) {
	remaining := c.limit - c.buf.Len()
	if remaining <= 0 {
		c.overflow = true
		return len(p), nil
	}
	if len(p) > remaining {
		c.buf.Write(p[:remaining])
		c.overflow = true
		return len(p), nil
	}
	return c.buf.Write(p)
}
