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

const recallSchema = 1

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
}

var (
	libtestSection = regexp.MustCompile(`^\s+(?:Running|Doc-tests)\b`)
	libtestFailed  = regexp.MustCompile(`^test (.+) \.\.\. FAILED$`)
	libtestFooter  = regexp.MustCompile("^error: (?:test|doctest) failed, to rerun pass `([^`]+)`$")
	libtestResult  = regexp.MustCompile(`^test result: `)
	libtestCompile = regexp.MustCompile(`^error: could not compile `)
)

const maxLogLine = 4 << 20

// parseLibtest reads `cargo test --no-fail-fast` output.
//
// libtest's JSON format is nightly-only, so this reads the text. Cargo prints
// a failing target's tests first and then a footer naming the package and the
// target (`-p cb --test it`); the footer is what attributes them, because the
// `Running tests/it.rs` header carries a path relative to the package, and two
// packages both have a src/lib.rs.
func parseLibtest(r io.Reader) SuiteRun {
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 64<<10), maxLogLine)
	var run SuiteRun
	var pending []string
	results, compileErrors := 0, 0
	flush := func(pkg, target string) {
		for _, name := range pending {
			run.Failures = append(run.Failures, Failure{Kind: "rust", Pkg: pkg, Target: target, Name: name})
		}
		pending = nil
	}
	for sc.Scan() {
		line := sc.Text()
		switch {
		case libtestSection.MatchString(line):
			flush("?", "")
		case libtestFailed.MatchString(line):
			pending = append(pending, libtestFailed.FindStringSubmatch(line)[1])
		case libtestFooter.MatchString(line):
			pkg, target := splitRerun(libtestFooter.FindStringSubmatch(line)[1])
			flush(pkg, target)
		case libtestResult.MatchString(line):
			results++
		case libtestCompile.MatchString(line):
			compileErrors++
		}
	}
	flush("?", "")
	switch {
	case sc.Err() != nil:
		run.Note = "the libtest log could not be read to the end: " + sc.Err().Error()
	case compileErrors > 0:
		run.Note = fmt.Sprintf("%d target(s) failed to compile, so their tests never ran", compileErrors)
	case results == 0:
		run.Note = "the libtest log holds no `test result:` line, so no test ran"
	default:
		run.Examined = true
	}
	sortFailures(run.Failures)
	return run
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

// parseGoJSON reads a `go test -json` stream.
func parseGoJSON(r io.Reader) SuiteRun {
	var run SuiteRun
	dec := json.NewDecoder(r)
	testFailed := map[string]bool{}
	pkgFailed := map[string]bool{}
	seen := map[Failure]bool{}
	finished := 0
	for {
		var ev goEvent
		if err := dec.Decode(&ev); errors.Is(err, io.EOF) {
			break
		} else if err != nil {
			run.Note = "the go test -json log could not be parsed to the end: " + err.Error()
			run.Failures = nil
			return run
		}
		if ev.Action != "fail" && ev.Action != "pass" && ev.Action != "skip" {
			continue
		}
		if ev.Test == "" {
			finished++
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
	if finished == 0 {
		run.Note = "the go test -json log holds no package result, so no test ran"
		return run
	}
	run.Examined = true
	return run
}

func sortFailures(fs []Failure) {
	sort.Slice(fs, func(i, j int) bool { return fs[i].Key() < fs[j].Key() })
}

// Covers reports whether the plan's selection ran the failed test.
func (p Plan) Covers(f Failure) bool {
	if p.Everything {
		return true
	}
	for _, sel := range p.Selected {
		for _, u := range sel.Units {
			if u.Kind != f.Kind || u.Pkg != f.Pkg {
				continue
			}
			if f.Kind == "go" {
				return true
			}
			if u.Target != "" && u.Target != f.Target {
				continue
			}
			if len(u.Filters) == 0 {
				return true
			}
			for _, filter := range u.Filters {
				if strings.Contains(f.Name, filter) {
					return true
				}
			}
		}
	}
	return false
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

// Recall is selection-recall.json.
type Recall struct {
	Schema     int              `json:"schema"`
	SHA        string           `json:"sha"`
	Base       string           `json:"base"`
	Trigger    string           `json:"trigger"`
	Blast      *BlastSummary    `json:"blast"`
	Everything bool             `json:"everything"`
	Widened    []Widening       `json:"widened"`
	Selected   []RecallSelected `json:"selected"`
	Full       FullReport       `json:"full"`
	// BaseFailed is null when the base run's failures are not known, which is
	// not the same as an empty list.
	BaseFailed []string `json:"base_failed"`
	// Missed are failures the selection did not run and the base did not have.
	Missed []string `json:"missed"`
	// Unclassified are failures outside the selection while the base is
	// unknown: they may be the base's own.
	Unclassified []string `json:"unclassified"`
	NFailed      int      `json:"n_failed"`
	// Recall is null when it cannot be asserted: nothing failed, the base is
	// unknown, or a suite was not examined. It is never 1.0 by default.
	Recall     *float64 `json:"recall"`
	RecallNote string   `json:"recall_note"`
}

// BaseFailures reads the failures a base-branch run recorded. It returns nil,
// with the reason, when they cannot be trusted: a different schema, a run that
// did not examine both suites, or a document that is not one.
func BaseFailures(raw []byte) ([]string, string) {
	var b struct {
		Schema int `json:"schema"`
		Full   *struct {
			Rust   SuiteReport `json:"rust"`
			Go     SuiteReport `json:"go"`
			Failed *[]string   `json:"failed"`
		} `json:"full"`
	}
	if err := json.Unmarshal(raw, &b); err != nil {
		return nil, "the base artifact is not JSON: " + err.Error()
	}
	switch {
	case b.Schema != recallSchema:
		return nil, fmt.Sprintf("the base artifact has schema %d, this build reads %d", b.Schema, recallSchema)
	case b.Full == nil || b.Full.Failed == nil:
		return nil, "the base artifact carries no full.failed list"
	case !b.Full.Rust.Examined || !b.Full.Go.Examined:
		return nil, "the base run did not examine both suites, so its failures are not its whole set"
	}
	return *b.Full.Failed, ""
}

// AuditInput is everything Audit measures.
type AuditInput struct {
	Plan    Plan
	SHA     string
	Rust    SuiteRun
	Go      SuiteRun
	Base    []string // nil: unknown
	BaseWhy string
}

// Audit computes selection-recall.json and the annotations that go with it.
//
// A miss is a failure of the full suite that the selection did not run and
// that the base did not already have. Whether the base failed it is the
// difference between "DevMap missed a test this change broke" and "this test
// was broken before"; with the base unknown the first cannot be asserted, so
// those failures are listed as unclassified and never as misses.
func Audit(in AuditInput) (Recall, []string) {
	var notes []string
	r := Recall{
		Schema: recallSchema, SHA: in.SHA, Base: in.Plan.Base, Trigger: in.Plan.Trigger, Blast: in.Plan.Blast,
		Everything: in.Plan.Everything, Widened: in.Plan.Widened, Selected: []RecallSelected{},
		Missed: []string{}, Unclassified: []string{},
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
	byKey := map[string]Failure{}
	for _, f := range failures {
		r.Full.Failed = append(r.Full.Failed, f.Key())
		byKey[f.Key()] = f
	}

	baseKnown := in.Base != nil
	baseSet := map[string]bool{}
	if baseKnown {
		r.BaseFailed = append([]string{}, in.Base...)
		for _, k := range in.Base {
			baseSet[k] = true
		}
	} else {
		notes = append(notes, "::warning::base_failed is unknown ("+in.BaseWhy+"); failures outside the selection are unclassified, not misses")
	}
	var attributable []Failure
	for _, f := range failures {
		if baseSet[f.Key()] {
			continue
		}
		attributable = append(attributable, f)
	}
	covered := 0
	for _, f := range attributable {
		switch {
		case in.Plan.Covers(f):
			covered++
		case baseKnown:
			r.Missed = append(r.Missed, f.Key())
			notes = append(notes, "::error::selection missed "+f.Key())
		default:
			r.Unclassified = append(r.Unclassified, f.Key())
		}
	}
	r.NFailed = len(attributable)

	switch {
	case !in.Rust.Examined || !in.Go.Examined:
		r.RecallNote = "a suite was not examined, so a failure may be absent from the record"
		for _, s := range []struct {
			name string
			run  SuiteRun
		}{{"Rust", in.Rust}, {"Go", in.Go}} {
			if !s.run.Examined {
				notes = append(notes, "::warning::the full "+s.name+" suite was not examined: "+s.run.Note)
			}
		}
	case !baseKnown:
		r.RecallNote = "the base's failures are unknown, so which failures belong to this change is unknown"
	case r.NFailed == 0:
		r.RecallNote = "nothing failed that the base did not, so there was no test for the selection to find"
	default:
		v := float64(covered) / float64(r.NFailed)
		r.Recall = &v
		r.RecallNote = fmt.Sprintf("%d of %d new failure(s) were inside the selection", covered, r.NFailed)
	}
	return r, notes
}
