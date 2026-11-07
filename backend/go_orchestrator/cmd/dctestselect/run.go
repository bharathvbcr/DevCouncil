package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"time"
)

const (
	defaultRustDir = "rust"
	defaultGoDir   = "backend/go_orchestrator"
	statusSchema   = 2
	// builtinPlan is the plan hash a status file carries when the plan was
	// unusable and run used its built-in full suite.
	builtinPlan = "builtin"
)

func jsonIndent(v any) ([]byte, error) {
	body, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return nil, err
	}
	return append(body, '\n'), nil
}

func strictUnmarshal(body []byte, into any) error {
	dec := json.NewDecoder(bytes.NewReader(body))
	dec.DisallowUnknownFields()
	if err := dec.Decode(into); err != nil {
		return err
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return errors.New("trailing data after the document")
	}
	return nil
}

// goJSONTee keeps `go test -json` verbatim in a log and shows a person what
// `go test` would have printed, so a failing test's output is in the job log
// and not only in an artifact.
type goJSONTee struct {
	log, console io.Writer
	buf          []byte
}

func (g *goJSONTee) Write(p []byte) (int, error) {
	if _, err := g.log.Write(p); err != nil {
		return 0, err
	}
	g.buf = append(g.buf, p...)
	for {
		i := bytes.IndexByte(g.buf, '\n')
		if i < 0 {
			break
		}
		g.show(g.buf[:i])
		g.buf = g.buf[i+1:]
	}
	if len(g.buf) > maxLogLine {
		g.buf = nil
	}
	return len(p), nil
}

func (g *goJSONTee) show(line []byte) {
	var ev struct {
		Action, Output string
	}
	if json.Unmarshal(line, &ev) == nil && ev.Action == "output" {
		_, _ = io.WriteString(g.console, ev.Output)
	}
}

// teeWriter writes to two places. It is a pointer type on purpose: cargo
// prints the `Running` header and the failure footer on stderr and the test
// lines on stdout, and os/exec shares one pipe only when both are the same
// comparable writer, which is what keeps the log in the order a person read it.
type teeWriter struct{ a, b io.Writer }

func (t *teeWriter) Write(p []byte) (int, error) {
	if _, err := t.b.Write(p); err != nil {
		return 0, err
	}
	return t.a.Write(p)
}

// phaseStatus is the status file one phase leaves next to its logs. The audit
// reads it: a log says what a process printed, not whether it was allowed to
// finish. It carries the hash of the plan it ran, so a file left by another
// plan, or by an earlier run that was killed before it wrote its own, is not
// read as this run's.
type phaseStatus struct {
	Schema     int        `json:"schema"`
	PlanSHA256 string     `json:"plan_sha256"`
	Commands   []ExitInfo `json:"commands"`
}

// loadStatus reads a phase's status file and refuses one written for another
// plan.
func loadStatus(path, planSHA string) (*phaseStatus, error) {
	body, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var s phaseStatus
	if err := strictUnmarshal(body, &s); err != nil {
		return nil, fmt.Errorf("%s is not a status file: %w", path, err)
	}
	if s.Schema != statusSchema {
		return nil, fmt.Errorf("%s has schema %d, this build reads %d", path, s.Schema, statusSchema)
	}
	if s.PlanSHA256 != planSHA {
		return nil, fmt.Errorf("%s was written for plan %s, not the plan being audited (%s): it is a stale status", path, s.PlanSHA256, planSHA)
	}
	return &s, nil
}

// byLog indexes a status by the log each command wrote.
func (s *phaseStatus) byLog() map[string]*ExitInfo {
	out := map[string]*ExitInfo{}
	for i := range s.Commands {
		c := s.Commands[i]
		if c.Log == "" {
			continue
		}
		// A log shared by several commands is as healthy as its worst one.
		if prev, ok := out[c.Log]; ok && (!prev.Completed || prev.Code != 0) {
			continue
		}
		out[c.Log] = &c
	}
	return out
}

// byCmd indexes a status by command line. A command that appears twice is as
// healthy as its worst entry.
func (s *phaseStatus) byCmd() map[string]*ExitInfo {
	out := map[string]*ExitInfo{}
	for i := range s.Commands {
		c := s.Commands[i]
		if prev, ok := out[c.Cmd]; ok && !prev.Completed {
			continue
		}
		out[c.Cmd] = &c
	}
	return out
}

// writeAtomic writes a file by renaming a finished temp file over it, so a
// reader sees the whole document or none of it.
func writeAtomic(path string, v any) error {
	body, err := jsonIndent(v)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), filepath.Base(path)+".tmp*")
	if err != nil {
		return err
	}
	_, werr := tmp.Write(body)
	cerr := tmp.Close()
	if err := errors.Join(werr, cerr); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	if err := os.Rename(tmp.Name(), path); err != nil {
		os.Remove(tmp.Name())
		return err
	}
	return nil
}

func runCmdPhase(ctx context.Context, args []string, stdout, stderr io.Writer) (int, error) {
	fs := flag.NewFlagSet("run", flag.ContinueOnError)
	planPath := fs.String("plan", "plan.json", "the plan to run")
	phase := fs.String("phase", "", "selected or full")
	logDir := fs.String("log-dir", ".", "where the logs and the status file are kept")
	repo := fs.String("repo", ".", "repository root")
	timeout := fs.Duration("timeout", 90*time.Minute, "bound for each command")
	if err := fs.Parse(args); err != nil {
		return 2, nil
	}
	if *phase != "selected" && *phase != "full" {
		return 2, fmt.Errorf("--phase must be selected or full, got %q", *phase)
	}
	root, err := filepath.Abs(*repo)
	if err != nil {
		return 2, err
	}
	plan, planSHA, err := loadPlan(*planPath)
	if err != nil {
		if *phase == "selected" {
			return 2, err
		}
		// The full suite is the gate. A plan that is missing, unreadable or
		// empty is a reason to say so, not a reason not to run it.
		fmt.Fprintln(stdout, annotate("error", "the plan is unusable ("+err.Error()+"); running the built-in full suite"))
		plan = Plan{Full: fullCommands(Inputs{Excludes: DefaultRustExcludes, RustDir: defaultRustDir, GoDir: defaultGoDir})}
		planSHA = builtinPlan
	}
	if err := os.MkdirAll(*logDir, 0o755); err != nil {
		return 2, err
	}
	// A status from an earlier run must not outlive this run's start: if this
	// process is killed before it writes its own, there is none, and the audit
	// says so.
	statusPath := filepath.Join(*logDir, *phase+"-status.json")
	if err := os.Remove(statusPath); err != nil && !errors.Is(err, os.ErrNotExist) {
		return 2, err
	}
	var cmds []Command
	if *phase == "selected" {
		if plan.Everything {
			fmt.Fprintln(stdout, annotate("notice", "the selection is everything; the fast phase is skipped and the full suite runs next"))
			return 0, nil
		}
		for _, s := range plan.Selected {
			cmds = append(cmds, s.Command)
		}
	} else {
		cmds = plan.Full
	}
	code, statuses := runPhase(ctx, root, cmds, *timeout, *logDir, stdout, stderr)
	if err := writeAtomic(statusPath, phaseStatus{Schema: statusSchema, PlanSHA256: planSHA, Commands: statuses}); err != nil {
		return 2, err
	}
	return code, nil
}

// runAll is runPhase without the statuses.
func runAll(ctx context.Context, root string, cmds []Command, timeout time.Duration, logDir string, stdout, stderr io.Writer) int {
	code, _ := runPhase(ctx, root, cmds, timeout, logDir, stdout, stderr)
	return code
}

// runPhase runs every command, failed ones included, and reports the worst
// outcome: 2 when a command could not run or was stopped, 1 when a command
// failed, 0 otherwise, with how each one ended. Stopping at the first failure
// would hide the later ones, and the audit needs all of them.
//
// Commands that name the same log share it: the first truncates, the rest
// append, so a phase's many cargo invocations are one libtest log.
func runPhase(ctx context.Context, root string, cmds []Command, timeout time.Duration, logDir string, stdout, stderr io.Writer) (int, []ExitInfo) {
	worst := 0
	statuses := []ExitInfo{}
	logs := map[string]*os.File{}
	defer func() {
		for _, f := range logs {
			f.Close()
		}
	}()
	for _, c := range cmds {
		fmt.Fprintln(stdout, group(c.Display))
		info, code := runOne(ctx, root, c, timeout, logDir, logs, stdout, stderr)
		fmt.Fprintln(stdout, "::endgroup::")
		if code != 0 {
			fmt.Fprintln(stdout, annotate("error", fmt.Sprintf("%s exited %d", c.Display, code)))
		}
		statuses = append(statuses, info)
		if code > worst {
			worst = code
		}
	}
	return worst, statuses
}

// logFile opens a phase's log: the first command to name it truncates it.
func logFile(logDir, name string, logs map[string]*os.File) (*os.File, error) {
	if f, ok := logs[name]; ok {
		return f, nil
	}
	if filepath.Base(name) != name {
		return nil, fmt.Errorf("refusing a log name that is a path: %s", name)
	}
	f, err := os.Create(filepath.Join(logDir, name))
	if err != nil {
		return nil, err
	}
	logs[name] = f
	return f, nil
}

func runOne(ctx context.Context, root string, c Command, timeout time.Duration, logDir string, logs map[string]*os.File, stdout, stderr io.Writer) (ExitInfo, int) {
	info := ExitInfo{Cmd: c.Display, Log: c.Log, Code: -1}
	var out, errw io.Writer = stdout, stderr
	if logDir != "" && c.Log != "" {
		f, err := logFile(logDir, c.Log, logs)
		if err != nil {
			fmt.Fprintln(stderr, "dctestselect:", err)
			info.Detail = err.Error()
			return info, 2
		}
		if c.GoJSON {
			out = &goJSONTee{log: f, console: stdout}
		} else {
			t := &teeWriter{a: stdout, b: f}
			out, errw = t, t
		}
	} else if c.GoJSON {
		out = &goJSONTee{log: io.Discard, console: stdout}
	}
	dir := filepath.Join(root, filepath.FromSlash(c.Dir))
	err := runProcess(ctx, timeout, dir, c.Env, c.Argv, out, errw)
	if g, ok := out.(*goJSONTee); ok {
		g.show(g.buf)
	}
	var exit *exec.ExitError
	switch {
	case err == nil:
		info.Code, info.Completed, info.Detail = 0, true, "exit status 0"
		return info, 0
	case errors.As(err, &exit) && ctx.Err() == nil && exit.ExitCode() > 0:
		info.Code, info.Completed, info.Detail = exit.ExitCode(), true, exit.Error()
		return info, 1
	default:
		fmt.Fprintln(stderr, "dctestselect:", err)
		info.Detail = err.Error()
		return info, 2
	}
}

// loadPlan reads and validates a plan, and returns the hash of its bytes. A plan
// with an unknown schema, or with no full-suite command, is unusable: a plan
// that lists nothing to run would make the gate pass having run nothing.
func loadPlan(path string) (Plan, string, error) {
	body, err := os.ReadFile(path)
	if err != nil {
		return Plan{}, "", err
	}
	var p Plan
	if err := strictUnmarshal(body, &p); err != nil {
		return Plan{}, "", fmt.Errorf("%s is not a plan: %w", path, err)
	}
	if p.Schema != planSchema {
		return Plan{}, "", fmt.Errorf("%s has schema %d, this build reads %d", path, p.Schema, planSchema)
	}
	if len(p.Full) == 0 {
		return Plan{}, "", fmt.Errorf("%s lists no full-suite command", path)
	}
	for _, c := range p.Full {
		if len(c.Argv) == 0 {
			return Plan{}, "", fmt.Errorf("%s has a full-suite command with no argv", path)
		}
	}
	sum := sha256.Sum256(body)
	return p, hex.EncodeToString(sum[:]), nil
}
