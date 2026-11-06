// Command dctestselect runs the tests DevMap says a change reaches before the
// full suite, and measures how much of what the full suite then fails the
// selection had already run.
//
//	dctestselect plan  [--repo DIR] [--devmap BIN] [--event E --ref-name R --before SHA --base-ref B | --base REV] --out plan.json
//	dctestselect run   --plan plan.json --phase selected|full [--log-dir DIR]
//	dctestselect audit --plan plan.json --rust-log F --go-log F [--base-artifact F] --sha SHA --out selection-recall.json
//
// The selection orders the work and prices the graph; it never replaces the
// full suite. So every doubt widens it, and the audit reports a failure that
// the selection did not run and the base did not have as `selection missed`.
//
// Exit codes: plan 0 a plan was written (possibly "everything") · 2 it could
// not be written. run 0 every command passed · 1 a command failed · 2 a
// command could not run. audit 0 no miss · 1 a miss · 2 a suite was not
// examined or the artifact could not be written.
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

// annotate renders a GitHub workflow command, escaping what the runner would
// otherwise read as the end of the message.
func annotate(level, msg string) string {
	r := strings.NewReplacer("%", "%25", "\r", "%0D", "\n", "%0A")
	return "::" + level + "::" + r.Replace(msg)
}

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
	fs.StringVar(&o.RustDir, "rust-dir", "rust", "cargo workspace, relative to the repository")
	fs.StringVar(&o.GoDir, "go-dir", "backend/go_orchestrator", "Go module, relative to the repository")
	fs.Var(&excludes, "exclude", "cargo package the full suite leaves out (repeatable; default: rust.yml's list)")
	timeout := fs.Duration("timeout", 5*time.Minute, "bound for each question the plan asks")
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
	plan := BuildPlan(gather(ctx, execRunner{timeout: *timeout}, o, readFile))
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
	body, err := os.ReadFile(path)
	if err != nil {
		return Plan{}, err
	}
	var p Plan
	if err := strictUnmarshal(body, &p); err != nil {
		return Plan{}, fmt.Errorf("%s is not a plan: %w", path, err)
	}
	if p.Schema != planSchema {
		return Plan{}, fmt.Errorf("%s has schema %d, this build reads %d", path, p.Schema, planSchema)
	}
	return p, nil
}

func auditCmd(args []string, stdout io.Writer) (int, error) {
	fs := flag.NewFlagSet("audit", flag.ContinueOnError)
	planPath := fs.String("plan", "plan.json", "the plan the run followed")
	rustLog := fs.String("rust-log", "", "the full suite's cargo test output")
	goLog := fs.String("go-log", "", "the full suite's go test -json output")
	baseArtifact := fs.String("base-artifact", "", "the base branch's selection-recall.json (optional)")
	sha := fs.String("sha", "", "the commit under test")
	out := fs.String("out", "selection-recall.json", "where the artifact is written")
	if err := fs.Parse(args); err != nil {
		return 2, nil
	}
	plan, err := readPlan(*planPath)
	if err != nil {
		return 2, err
	}
	in := AuditInput{Plan: plan, SHA: *sha}
	in.Rust = readSuite(*rustLog, "the cargo test log", parseLibtest)
	in.Go = readSuite(*goLog, "the go test -json log", parseGoJSON)
	in.BaseWhy = "no base-branch artifact was given"
	if *baseArtifact != "" {
		if body, err := os.ReadFile(*baseArtifact); err != nil {
			in.BaseWhy = "the base artifact could not be read: " + err.Error()
		} else {
			in.Base, in.BaseWhy = BaseFailures(body)
		}
	}
	recall, notes := Audit(in)
	if err := writeJSON(*out, recall); err != nil {
		return 2, err
	}
	for _, n := range notes {
		fmt.Fprintln(stdout, n)
	}
	switch {
	case len(recall.Missed) > 0:
		return 1, nil
	case !recall.Full.Rust.Examined || !recall.Full.Go.Examined:
		return 2, nil
	}
	return 0, nil
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
