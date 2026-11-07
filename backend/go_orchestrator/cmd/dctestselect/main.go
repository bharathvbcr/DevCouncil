// Command dctestselect runs the tests DevMap says a change reaches before the
// full suite, and measures how much of what the full suite then fails the
// selection had already run.
//
//	dctestselect plan  [--repo DIR] [--devmap BIN] [--event E --ref-name R --before SHA --base-ref B | --base REV] --out plan.json
//	dctestselect run   --plan plan.json --phase selected|full [--log-dir DIR]
//	dctestselect audit --plan plan.json [--log-dir DIR] [--base-artifact F] --sha SHA --out selection-recall.json
//
// The selection orders the work and prices the graph; it never replaces the
// full suite. So every doubt widens it, and the audit reports a failure that
// the selection did not run and the base did not have as `selection missed`.
//
// Exit codes: plan 0 a plan was written (possibly "everything") · 2 it could
// not be written. run 0 every command passed · 1 a command failed · 2 a
// command could not run. audit 0 no miss · 1 a miss · 2 a suite was not
// examined, failures could not be classified, or the artifact could not be
// written. The artifact is written on every path that can write it.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"time"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	os.Exit(run(ctx, os.Args[1:], os.Stdout, os.Stderr))
}

func run(ctx context.Context, args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 {
		fmt.Fprintln(stderr, "usage: dctestselect plan|run|audit [flags]")
		return 2
	}
	var err error
	code := 0
	switch args[0] {
	case "plan":
		code, err = planCmd(ctx, args[1:], stdout)
	case "run":
		code, err = runCmdPhase(ctx, args[1:], stdout, stderr)
	case "audit":
		code, err = auditCmd(args[1:], stdout)
	default:
		err = fmt.Errorf("unknown command %q", args[0])
		code = 2
	}
	if err != nil {
		fmt.Fprintln(stderr, "dctestselect:", err)
	}
	return code
}

// escapeData escapes a workflow command's message the way the runner reads it:
// `%`, CR and LF, so a value cannot end the command and start another.
func escapeData(s string) string {
	return strings.NewReplacer("%", "%25", "\r", "%0D", "\n", "%0A").Replace(s)
}

// annotate renders a workflow command with an escaped message. Every value
// this program writes after `::` goes through here or group, because paths and
// test names come from the tree under test.
func annotate(level, msg string) string {
	return "::" + level + "::" + escapeData(msg)
}

func group(title string) string { return "::group::" + escapeData(title) }

type listFlag []string

func (l *listFlag) String() string     { return strings.Join(*l, ",") }
func (l *listFlag) Set(v string) error { *l = append(*l, v); return nil }

func planCmd(ctx context.Context, args []string, stdout io.Writer) (int, error) {
	fs := flag.NewFlagSet("plan", flag.ContinueOnError)
	var o Options
	var excludes listFlag
	out := fs.String("out", "plan.json", "where the plan is written")
	repo := fs.String("repo", ".", "repository root")
	fs.StringVar(&o.Devmap, "devmap", "devmap", "devmap binary")
	fs.StringVar(&o.Base, "base", "", "revision to measure from (overrides the event)")
	fs.StringVar(&o.Event, "event", "", "GITHUB_EVENT_NAME")
	fs.StringVar(&o.RefName, "ref-name", "", "GITHUB_REF_NAME")
	fs.StringVar(&o.Before, "before", "", "github.event.before")
	fs.StringVar(&o.BaseRef, "base-ref", "main", "the default branch")
	fs.StringVar(&o.Trigger, "trigger", "", "label for the artifact (default: the event)")
	fs.StringVar(&o.RustDir, "rust-dir", defaultRustDir, "cargo workspace, relative to the repository")
	fs.StringVar(&o.GoDir, "go-dir", defaultGoDir, "Go module, relative to the repository")
	fs.Var(&excludes, "exclude", "cargo package the full suite leaves out (repeatable; default: rust.yml's list)")
	fs.DurationVar(&o.Timeout, "timeout", 5*time.Minute, "bound for each question the plan asks, the devmap blast included")
	if err := fs.Parse(args); err != nil {
		return 2, nil
	}
	root, err := filepath.Abs(*repo)
	if err == nil {
		root, err = filepath.EvalSymlinks(root)
	}
	if err != nil {
		return 2, fmt.Errorf("repository %q: %w", *repo, err)
	}
	o.Repo = root
	if o.BaseRef == "" || !refToken.MatchString(o.BaseRef) || strings.HasPrefix(o.BaseRef, "-") {
		return 2, fmt.Errorf("--base-ref %q is not a branch name", o.BaseRef)
	}
	if o.Trigger == "" {
		o.Trigger = o.Event
	}
	o.Excludes = excludes
	if len(o.Excludes) == 0 {
		o.Excludes = DefaultRustExcludes
	}
	readFile := func(rel string) ([]byte, error) {
		if !filepath.IsLocal(filepath.FromSlash(rel)) {
			return nil, fmt.Errorf("%q is not inside the repository", rel)
		}
		return os.ReadFile(filepath.Join(root, filepath.FromSlash(rel)))
	}
	plan := BuildPlan(gather(ctx, execRunner{timeout: o.Timeout}, o, readFile))
	if err := writeJSON(*out, plan); err != nil {
		return 2, err
	}
	logPlan(stdout, plan)
	return 0, nil
}

const maxDroppedLogged = 40

func logPlan(w io.Writer, p Plan) {
	if p.Blast != nil {
		fmt.Fprintln(w, annotate("notice", fmt.Sprintf("blast %s: %d seeds, %d test files, complete=%t", p.Blast.Change, p.Blast.SeedsTotal, p.Blast.TestsTotal, p.Blast.Complete)))
	}
	for _, n := range p.Notices {
		fmt.Fprintln(w, annotate("notice", n))
	}
	for _, wd := range p.Widened {
		fmt.Fprintln(w, annotate("notice", "widened "+wd.Scope+": "+wd.Reason))
	}
	if p.Everything {
		fmt.Fprintln(w, annotate("notice", "selection is everything; the fast phase is skipped and the full suite runs"))
	}
	for _, s := range p.Selected {
		fmt.Fprintln(w, annotate("notice", fmt.Sprintf("selected %s (%s) files=%d seeds=%s", s.Display, s.Why, len(s.Files), strings.Join(s.Seeds, ", "))))
	}
	for i, d := range p.Dropped {
		if i == maxDroppedLogged {
			fmt.Fprintln(w, annotate("notice", fmt.Sprintf("... and %d more blast test paths dropped", len(p.Dropped)-i)))
			break
		}
		fmt.Fprintln(w, annotate("notice", "dropped "+d.Path+": "+d.Reason))
	}
	if !p.Everything && len(p.Selected) == 0 {
		fmt.Fprintln(w, annotate("notice", "the change reaches no test the full suite runs; the full suite runs next"))
	}
}

func writeJSON(path string, v any) error {
	body, err := jsonIndent(v)
	if err != nil {
		return err
	}
	if err := os.WriteFile(path, body, 0o644); err != nil {
		return fmt.Errorf("write %s: %w", path, err)
	}
	return nil
}

func readPlan(path string) (Plan, error) {
	p, _, err := loadPlan(path)
	return p, err
}

func auditCmd(args []string, stdout io.Writer) (int, error) {
	fs := flag.NewFlagSet("audit", flag.ContinueOnError)
	planPath := fs.String("plan", "plan.json", "the plan the run followed")
	logDir := fs.String("log-dir", "", "where run kept its logs and status files (default: beside --plan)")
	baseArtifact := fs.String("base-artifact", "", "the base branch's selection-recall.json (optional)")
	baseBranch := fs.String("base-branch", "main", "the branch whose record is the base")
	sha := fs.String("sha", "", "the commit under test")
	ref := fs.String("ref", "", "GITHUB_REF")
	event := fs.String("event", "", "GITHUB_EVENT_NAME")
	runID := fs.String("run-id", "", "GITHUB_RUN_ID")
	out := fs.String("out", "selection-recall.json", "where the artifact is written")
	if err := fs.Parse(args); err != nil {
		return 2, nil
	}
	if *logDir == "" {
		*logDir = filepath.Dir(*planPath)
	}
	in := AuditInput{SHA: *sha, Ref: *ref, Event: *event, ID: *runID}
	plan, planSHA, planErr := loadPlan(*planPath)
	if planErr != nil {
		// The artifact is written whatever happened: an absent record and a
		// record of "could not examine" are different facts.
		plan = Plan{Schema: planSchema, Widened: []Widening{}, Selected: []Selected{}, Dropped: []Dropped{}, Full: []Command{}, Notices: []string{}}
		in.PlanUnusable = true
	}
	in.Plan = plan
	status, statusErr := loadStatus(filepath.Join(*logDir, "full-status.json"), planSHA)
	if planErr != nil {
		statusErr = planErr
	}
	suite := func(logName, what string, parse func(io.Reader) SuiteRun) SuiteRun {
		run := readSuite(filepath.Join(*logDir, logName), what, parse)
		if statusErr != nil {
			if run.Examined {
				run.Examined, run.Note = false, "no usable exit status: "+statusErr.Error()
			}
			return run
		}
		return Qualify(run, status.byLog()[logName])
	}
	in.Rust = suite("full-rust.log", "the cargo test log", parseLibtest)
	in.Go = suite("full-go.jsonl", "the go test -json log", parseGoJSON)
	in.SelectedRust, in.SelectedGo, in.SelectedKnown = readSelected(*logDir, plan)
	if planErr == nil {
		in.Ran, in.SelectionNote = ranSelection(plan, filepath.Join(*logDir, "selected-status.json"), planSHA, in.SelectedRust, in.SelectedGo)
	}
	in.BaseWhy = "no base-branch artifact was given"
	if *baseArtifact != "" {
		if body, err := os.ReadFile(*baseArtifact); err != nil {
			in.BaseWhy = "the base artifact could not be read: " + err.Error()
		} else {
			var rec BaseRecord
			in.Base, rec, in.BaseWhy = BaseFailures(body, "refs/heads/"+*baseBranch)
			if in.Base != nil {
				in.BaseInfo = &rec
			}
		}
	}
	recall, notes := Audit(in)
	if planErr != nil {
		notes = append(notes, annotate("error", "the plan was unusable: "+planErr.Error()))
	}
	if err := writeAtomic(*out, recall); err != nil {
		return 2, err
	}
	for _, n := range notes {
		fmt.Fprintln(stdout, n)
	}
	return Exit(in, recall), nil
}

// ranSelection is the selection as it ran, read from the fast phase's status
// file: the plan less every selected command whose status does not say it
// completed or whose log was not examined (a cargo compile error exits 101 and
// leaves no `test result:`: it completed, and ran nothing). A plan that selected
// nothing, or everything, has no fast phase to read. Otherwise a status that is
// missing, stale (another plan's) or lacks a selected command means what ran is
// unknown, and the note says so.
func ranSelection(plan Plan, statusPath, planSHA string, rust, golang SuiteRun) (*Plan, string) {
	if plan.Everything || len(plan.Selected) == 0 {
		return &plan, ""
	}
	status, err := loadStatus(statusPath, planSHA)
	if err != nil {
		return &Plan{Selected: []Selected{}}, "no usable fast-phase status: " + err.Error()
	}
	by := status.byCmd()
	ran := plan
	ran.Selected = nil
	note := ""
	for _, s := range plan.Selected {
		st := by[s.Display]
		log := rust
		if s.GoJSON {
			log = golang
		}
		switch {
		case st == nil:
			note = "the status has no entry for " + s.Display
		case !st.Completed:
			note = s.Display + " did not run to completion (" + st.Detail + ")"
		case !log.Examined:
			note = s.Display + " left a log that was not examined (" + log.Note + ")"
		default:
			ran.Selected = append(ran.Selected, s)
		}
	}
	return &ran, note
}

// readSelected reads the fast phase's logs. They are known only when every
// kind of command the plan selected left an examined log; a plan that selected
// nothing has nothing to read.
func readSelected(dir string, plan Plan) (rust, golang SuiteRun, known bool) {
	var wantRust, wantGo bool
	for _, s := range plan.Selected {
		if s.GoJSON {
			wantGo = true
		} else {
			wantRust = true
		}
	}
	if plan.Everything || (!wantRust && !wantGo) {
		return SuiteRun{}, SuiteRun{}, false
	}
	rust, golang = SuiteRun{Examined: true}, SuiteRun{Examined: true}
	if wantRust {
		rust = readSuite(filepath.Join(dir, "selected-rust.log"), "the selected phase's cargo log", parseLibtest)
	}
	if wantGo {
		golang = readSuite(filepath.Join(dir, "selected-go.jsonl"), "the selected phase's go log", parseGoJSON)
	}
	return rust, golang, true
}

func readSuite(path, what string, parse func(io.Reader) SuiteRun) SuiteRun {
	if path == "" {
		return SuiteRun{Note: what + " was not given"}
	}
	f, err := os.Open(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return SuiteRun{Note: what + " does not exist, so the suite did not run"}
		}
		return SuiteRun{Note: what + " could not be opened: " + err.Error()}
	}
	defer f.Close()
	return parse(f)
}
