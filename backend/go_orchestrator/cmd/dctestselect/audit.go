package main

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"regexp"
	"sort"
	"strings"
)

const recallSchema = 4

// Failure is one failed test, in the terms a unit is described in.
type Failure struct {
	Kind string // "rust" or "go"
	// Pkg is the cargo package, or the Go import path. "?" when cargo's
	// per-target footer that names the package never arrived.
	Pkg string
	// Target is the rest of cargo's `to rerun pass` argument list: `--lib`,
	// `--test it`, `--doc`. Empty for Go.
	Target string
	// Name is the libtest name, the Go test name, or "[package]" for a Go
	// package that failed with no failing test (a build failure or a panic).
	Name string
}

// Key is the stable identity compared across runs.
func (f Failure) Key() string {
	if f.Kind == "go" {
		return "go:" + f.Pkg + ":" + f.Name
	}
	return "rust:" + f.Pkg + ":" + f.Target + ":" + f.Name
}

// SuiteRun is what one suite's output says happened.
type SuiteRun struct {
	// Examined is true only when the output shows tests ran to completion. A
	// log that is missing, empty, or ends in a compile error is not a suite
	// that passed.
	Examined bool
	Note     string
	Failures []Failure
	// FailedResults counts the `test result: FAILED` lines of a cargo log: the
	// proof that a non-zero exit came from tests that ran.
	FailedResults int
	// Done is the Go packages with a final pass, fail or skip action.
	Done map[string]bool
}

var (
	// Cargo's own header shapes, exactly: five spaces and `Running`, an
	// optional `unittests`, a path and the binary in parentheses; three spaces
	// and `Doc-tests <crate>`. Anything looser matches a test's own output.
	libtestSection      = regexp.MustCompile(`^ {5}Running (?:unittests )?\S.* \([^()\s]+\)$|^ {3}Doc-tests \S+$`)
	libtestBlock        = regexp.MustCompile(`^---- (.+) stdout ----$`)
	libtestFailed       = regexp.MustCompile(`(?:^|\W)test (.+?) \.\.\. FAILED$`)
	libtestFooter       = regexp.MustCompile("^error: (?:test|doctest) failed, to rerun pass `([^`]+)`$")
	libtestResult       = regexp.MustCompile(`^test result: `)
	libtestResultFailed = regexp.MustCompile(`^test result: FAILED`)
	libtestCompile      = regexp.MustCompile(`^error: could not compile `)
	ansiEscape          = regexp.MustCompile(`\x1b\[[0-9;?]*[ -/]*[@-~]`)
)

const maxLogLine = 4 << 20

func stripANSI(s string) string { return ansiEscape.ReplaceAllString(s, "") }

// parseLibtest reads `cargo test --no-fail-fast` output.
//
// libtest's JSON format is nightly-only, so this reads the text, and a test can
// print anything into it. Only the lines libtest itself writes outside a
// failing test's captured `---- name stdout ----` block are structure: the
// `Running` header, the `failures:` summary's name list, and `test result:`.
// So a section's failures are the summary's names (the pre-summary FAILED
// lines are the fallback when there is none), and its package is named by the
// footer cargo prints after `test result:`. A footer or FAILED line inside a
// captured block comes before the section's own `test result:` and the summary,
// and changes nothing. A section with no `test result:` line was cut off or
// aborted, and the suite is then not examined.
func parseLibtest(r io.Reader) SuiteRun {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 64<<10), maxLogLine)
	var run SuiteRun
	var section []string
	var st blockState
	compileErrors, results, incomplete := 0, 0, 0
	seen := map[Failure]bool{}
	finish := func() {
		if section == nil {
			return
		}
		failed, pkg, target, done, resultFailed := readSection(section)
		if done {
			results++
			if resultFailed {
				run.FailedResults++
			}
		} else {
			incomplete++
		}
		for _, name := range failed {
			f := Failure{Kind: "rust", Pkg: pkg, Target: target, Name: name}
			if !seen[f] {
				seen[f] = true
				run.Failures = append(run.Failures, f)
			}
		}
		section = nil
	}
	for sc.Scan() {
		line := stripANSI(sc.Text())
		if libtestCompile.MatchString(line) {
			compileErrors++
		}
		// A header-shaped line is a new test binary only outside a failing
		// test's captured output. Inside it (after a `---- x stdout ----`
		// marker, until the `failures:` summary naming exactly those tests) it
		// is the test's own text: a migration log, a spawned cargo.
		if libtestSection.MatchString(line) && (section == nil || st.closed()) {
			finish()
			section = []string{}
			st = blockState{}
			continue
		}
		if section != nil {
			section = append(section, line)
			st.feed(line)
		}
	}
	finish()
	switch {
	case sc.Err() != nil:
		run.Note = "the libtest log could not be read to the end: " + sc.Err().Error()
	case compileErrors > 0:
		run.Note = fmt.Sprintf("%d target(s) failed to compile, so their tests never ran", compileErrors)
	case incomplete > 0:
		run.Note = fmt.Sprintf("%d test binary(ies) have a `Running` header and no `test result:` line: killed, aborted or cut off", incomplete)
	case results == 0:
		run.Note = "the libtest log holds no `test result:` line, so no test ran"
	default:
		run.Examined = true
	}
	sortFailures(run.Failures)
	return run
}

// blockState follows one test binary's output far enough to know whether a
// header-shaped line can begin a new binary: it cannot while a failing test's
// captured output is open, which is from its `---- name stdout ----` marker
// until the `failures:` summary lists exactly the tests that had markers.
type blockState struct {
	blocks     map[string]bool
	names      map[string]bool
	collecting bool
	summaryOK  bool
}

func (b *blockState) closed() bool { return len(b.blocks) == 0 || b.summaryOK }

func (b *blockState) feed(line string) {
	if m := libtestBlock.FindStringSubmatch(line); m != nil {
		if b.blocks == nil {
			b.blocks = map[string]bool{}
		}
		b.blocks[m[1]] = true
		b.collecting, b.summaryOK = false, false
		return
	}
	switch {
	case line == "failures:":
		b.collecting, b.names = true, map[string]bool{}
	case b.collecting && strings.HasPrefix(line, "    ") && strings.TrimSpace(line) != "":
		b.names[strings.TrimSpace(line)] = true
	case b.collecting:
		if len(b.names) > 0 && len(b.names) == len(b.blocks) {
			same := true
			for n := range b.names {
				same = same && b.blocks[n]
			}
			b.summaryOK = same
		}
		b.collecting = false
	}
}

// readSection reads one test binary's output. done is whether it reached its
// own `test result:` line.
func readSection(lines []string) (failed []string, pkg, target string, done, resultFailed bool) {
	result := -1
	for i, l := range lines {
		if libtestResult.MatchString(l) {
			result = i
		}
	}
	pre := lines
	if result >= 0 {
		pre = lines[:result]
	}
	for i, l := range pre {
		if l == "failures:" {
			pre = pre[:i]
			break
		}
	}
	var fromLines []string
	for _, l := range pre {
		if m := libtestFailed.FindStringSubmatch(l); m != nil {
			fromLines = append(fromLines, m[1])
		}
	}
	failed = fromLines
	pkg, target = "?", ""
	if result < 0 {
		return failed, pkg, target, false, false
	}
	// The summary: the names listed after the last `failures:` before the result.
	last := -1
	for i := 0; i < result; i++ {
		if lines[i] == "failures:" {
			last = i
		}
	}
	if last >= 0 {
		var names []string
		for _, l := range lines[last+1 : result] {
			if strings.HasPrefix(l, "    ") && strings.TrimSpace(l) != "" {
				names = append(names, strings.TrimSpace(l))
			} else if strings.TrimSpace(l) != "" || len(names) > 0 {
				break
			}
		}
		if len(names) > 0 {
			failed = names
		}
	}
	for _, l := range lines[result+1:] {
		if m := libtestFooter.FindStringSubmatch(l); m != nil {
			pkg, target = splitRerun(m[1])
			break
		}
	}
	return failed, pkg, target, true, libtestResultFailed.MatchString(lines[result])
}

// splitRerun splits `-p cb --test it` into the package and `--test it`.
func splitRerun(args string) (pkg, target string) {
	fields := strings.Fields(args)
	var rest []string
	for i := 0; i < len(fields); i++ {
		if fields[i] == "-p" && i+1 < len(fields) {
			pkg = fields[i+1]
			i++
			continue
		}
		rest = append(rest, fields[i])
	}
	if pkg == "" {
		pkg = "?"
	}
	return pkg, strings.Join(rest, " ")
}

type goEvent struct {
	Action  string `json:"Action"`
	Package string `json:"Package"`
	Test    string `json:"Test"`
}

// parseGoJSON reads a `go test -json` stream. A package that has events and
// no final pass, fail or skip of its own was killed or cut off, so the stream
// is not examined.
func parseGoJSON(r io.Reader) SuiteRun {
	var run SuiteRun
	dec := json.NewDecoder(r)
	testFailed := map[string]bool{}
	pkgFailed := map[string]bool{}
	seenPkg := map[string]bool{}
	donePkg := map[string]bool{}
	seen := map[Failure]bool{}
	for {
		var ev goEvent
		if err := dec.Decode(&ev); errors.Is(err, io.EOF) {
			break
		} else if err != nil {
			run.Note = "the go test -json log could not be parsed to the end: " + err.Error()
			run.Failures = nil
			return run
		}
		if ev.Package != "" {
			seenPkg[ev.Package] = true
		}
		if ev.Action != "fail" && ev.Action != "pass" && ev.Action != "skip" {
			continue
		}
		if ev.Test == "" {
			donePkg[ev.Package] = true
			if ev.Action == "fail" {
				pkgFailed[ev.Package] = true
			}
			continue
		}
		if ev.Action == "fail" {
			f := Failure{Kind: "go", Pkg: ev.Package, Name: ev.Test}
			if !seen[f] {
				seen[f] = true
				run.Failures = append(run.Failures, f)
			}
			testFailed[ev.Package] = true
		}
	}
	for pkg := range pkgFailed {
		if !testFailed[pkg] {
			run.Failures = append(run.Failures, Failure{Kind: "go", Pkg: pkg, Name: "[package]"})
		}
	}
	sortFailures(run.Failures)
	run.Done = donePkg
	var unfinished []string
	for pkg := range seenPkg {
		if !donePkg[pkg] {
			unfinished = append(unfinished, pkg)
		}
	}
	sort.Strings(unfinished)
	switch {
	case len(donePkg) == 0:
		run.Note = "the go test -json log holds no package result, so no test ran"
	case len(unfinished) > 0:
		run.Note = "the go test -json log has package(s) with no final result, killed or cut off: " + strings.Join(unfinished, ", ")
	default:
		run.Examined = true
	}
	return run
}

func sortFailures(fs []Failure) {
	sort.Slice(fs, func(i, j int) bool { return fs[i].Key() < fs[j].Key() })
}

// RecallSelected is the artifact's record of one selected command.
type RecallSelected struct {
	Cmd   string   `json:"cmd"`
	Files []string `json:"files"`
	Seeds []string `json:"seeds"`
	Why   string   `json:"why"`
}

// SuiteReport is one suite's verdict in the artifact.
type SuiteReport struct {
	Examined bool   `json:"examined"`
	Note     string `json:"note,omitempty"`
}

// FullReport is what the full suite did.
type FullReport struct {
	Rust   SuiteReport `json:"rust"`
	Go     SuiteReport `json:"go"`
	Failed []string    `json:"failed"`
}

// Recall is selection-recall.json: a record of what the job selected, what the
// full suite failed, and whether each suite could be examined. It carries no
// recall figure and no verdict about the selection (the file keeps its name so
// the artifact's consumers keep finding it).
type Recall struct {
	Schema     int              `json:"schema"`
	SHA        string           `json:"sha"`
	Ref        string           `json:"ref"`
	Event      string           `json:"event"`
	RunID      string           `json:"run_id"`
	Base       string           `json:"base"`
	Trigger    string           `json:"trigger"`
	Blast      *BlastSummary    `json:"blast"`
	Everything bool             `json:"everything"`
	Widened    []Widening       `json:"widened"`
	Selected   []RecallSelected `json:"selected"`
	Full       FullReport       `json:"full"`
	// SelectedPhaseFailedOnly are failures of the fast phase that the full
	// suite did not have: a red only the selected commands' feature set or
	// ordering produced. Null when either phase was not examined.
	SelectedPhaseFailedOnly []string `json:"selected_phase_failed_only"`
}

// ExitInfo is how one command of a phase ended, as run recorded it.
type ExitInfo struct {
	Cmd  string `json:"cmd"`
	Log  string `json:"log"`
	Code int    `json:"exit_code"`
	// Detail is the process state ("exit status 101", "signal: killed") or why
	// the command could not run.
	Detail string `json:"detail"`
	// Completed is true only when the process exited on its own.
	Completed bool `json:"completed"`
}

// Qualify demotes a suite that its own exit status contradicts. A run that was
// killed, did not start, exited non-zero with no failing test to show for it,
// or exited zero with failures in its output is not one whose log says what
// happened, whatever the log parses to.
func Qualify(run SuiteRun, st *ExitInfo) SuiteRun {
	if !run.Examined {
		return run
	}
	why := ""
	switch {
	case st == nil:
		why = "no exit status was recorded for the command"
	case !st.Completed:
		why = "the command did not run to completion (" + st.Detail + ")"
	case st.Code != 0 && len(run.Failures) == 0:
		why = fmt.Sprintf("the command exited %d with no failing test in its log", st.Code)
	case st.Code == 0 && len(run.Failures) > 0:
		why = "the command exited 0 but its log shows failures"
	}
	if why != "" {
		run.Examined = false
		run.Note = why
	}
	return run
}

// AuditInput is everything Audit records.
type AuditInput struct {
	Plan                Plan
	SHA, Ref, Event, ID string
	Rust                SuiteRun
	Go                  SuiteRun
	// PlanUnusable is set when the plan could not be read: nothing is known
	// about the selection.
	PlanUnusable bool
	// SelectionNote is non-empty when the fast phase could not be examined (a
	// selected command did not complete, or its status is missing or stale).
	SelectionNote string
	// SelectedRust and SelectedGo are the fast phase's logs; SelectedKnown is
	// false when they could not be read.
	SelectedRust, SelectedGo SuiteRun
	SelectedKnown            bool
}

// Audit computes selection-recall.json and the annotations that go with it. It
// records; it does not measure the selection against the full suite.
func Audit(in AuditInput) (Recall, []string) {
	var notes []string
	r := Recall{
		Schema: recallSchema, SHA: in.SHA, Ref: in.Ref, Event: in.Event, RunID: in.ID,
		Base: in.Plan.Base, Trigger: in.Plan.Trigger, Blast: in.Plan.Blast,
		Everything: in.Plan.Everything, Widened: in.Plan.Widened, Selected: []RecallSelected{},
	}
	for _, s := range in.Plan.Selected {
		r.Selected = append(r.Selected, RecallSelected{Cmd: s.Display, Files: s.Files, Seeds: s.Seeds, Why: s.Why})
	}
	r.Full = FullReport{
		Rust:   SuiteReport{Examined: in.Rust.Examined, Note: in.Rust.Note},
		Go:     SuiteReport{Examined: in.Go.Examined, Note: in.Go.Note},
		Failed: []string{},
	}
	var failures []Failure
	failures = append(failures, in.Rust.Failures...)
	failures = append(failures, in.Go.Failures...)
	sortFailures(failures)
	fullFailed := map[string]bool{}
	for _, f := range failures {
		r.Full.Failed = append(r.Full.Failed, f.Key())
		fullFailed[f.Key()] = true
	}
	if in.SelectedKnown && in.SelectedRust.Examined && in.SelectedGo.Examined && in.Rust.Examined && in.Go.Examined {
		r.SelectedPhaseFailedOnly = []string{}
		for _, f := range append(append([]Failure{}, in.SelectedRust.Failures...), in.SelectedGo.Failures...) {
			if !fullFailed[f.Key()] {
				r.SelectedPhaseFailedOnly = append(r.SelectedPhaseFailedOnly, f.Key())
			}
		}
		sort.Strings(r.SelectedPhaseFailedOnly)
		for _, k := range r.SelectedPhaseFailedOnly {
			notes = append(notes, annotate("warning", "failed in the selected phase only, not in the full suite: "+k))
		}
	}
	for _, s := range []struct {
		name string
		run  SuiteRun
	}{{"Rust", in.Rust}, {"Go", in.Go}} {
		if !s.run.Examined {
			notes = append(notes, annotate("warning", "the full "+s.name+" suite was not examined: "+s.run.Note))
		}
	}
	if in.SelectionNote != "" {
		notes = append(notes, annotate("warning", "the selected phase was not examined: "+in.SelectionNote))
	}
	return r, notes
}

// Exit is the audit's exit code: 2 when the plan, a suite or the fast phase
// could not be examined, otherwise 0. The job's gate is the full suite's own
// step; this is a loud report, never a verdict on the selection.
func Exit(in AuditInput, r Recall) int {
	if in.PlanUnusable || !r.Full.Rust.Examined || !r.Full.Go.Examined || in.SelectionNote != "" {
		return 2
	}
	return 0
}
