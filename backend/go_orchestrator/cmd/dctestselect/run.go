package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

const (
	defaultRustDir = "rust"
	defaultGoDir   = "backend/go_orchestrator"
	statusSchema   = 1
	// maxCapture bounds the output kept to see whether a filtered run ran
	// anything. The count is in libtest's first lines, so little is needed.
	maxCapture = 8 << 20
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
// finish.
type phaseStatus struct {
	Schema   int        `json:"schema"`
	Commands []ExitInfo `json:"commands"`
}

func readStatus(path string) (map[string]*ExitInfo, error) {
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
	byLog := map[string]*ExitInfo{}
	for i := range s.Commands {
		c := s.Commands[i]
		if c.Log == "" {
			continue
		}
		// A log shared by several commands is as healthy as its worst one.
		if prev, ok := byLog[c.Log]; ok && (!prev.Completed || prev.Code != 0) {
			continue
		}
		byLog[c.Log] = &c
	}
	return byLog, nil
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
	plan, err := readPlan(*planPath)
	if err != nil {
		if *phase == "selected" {
			return 2, err
		}
		// The full suite is the gate. A plan that is missing or unreadable is
		// a reason to say so, not a reason not to run it.
		fmt.Fprintln(stdout, annotate("error", "the plan is unusable ("+err.Error()+"); running the built-in full suite"))
		plan = Plan{Full: fullCommands(Inputs{Excludes: DefaultRustExcludes, RustDir: defaultRustDir, GoDir: defaultGoDir})}
	}
	if err := os.MkdirAll(*logDir, 0o755); err != nil {
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
	if err := writeJSON(filepath.Join(*logDir, *phase+"-status.json"), phaseStatus{Schema: statusSchema, Commands: statuses}); err != nil {
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
	one := func(c Command) {
		fmt.Fprintln(stdout, group(c.Display))
		var capture *proc.CappedBuffer
		if c.Fallback != nil {
			capture = &proc.CappedBuffer{Limit: maxCapture}
		}
		info, code := runOne(ctx, root, c, timeout, logDir, logs, capture, stdout, stderr)
		fmt.Fprintln(stdout, "::endgroup::")
		if code != 0 {
			fmt.Fprintln(stdout, annotate("error", fmt.Sprintf("%s exited %d", c.Display, code)))
		}
		statuses = append(statuses, info)
		if code > worst {
			worst = code
		}
		if capture != nil && code == 0 && ranNothing(capture.String()) {
			// A filter that matches no test is not a pass. It names a module
			// by a path libtest does not use (a #[path] module, an include!),
			// and the tests in it never ran.
			fmt.Fprintln(stdout, annotate("notice", "filter matched nothing: "+c.Display+"; running the target unfiltered"))
			whole := Command{Display: strings.Join(c.Fallback, " "), Dir: c.Dir, Argv: c.Fallback, Log: c.Log, Env: c.Env}
			fmt.Fprintln(stdout, group(whole.Display))
			info, code := runOne(ctx, root, whole, timeout, logDir, logs, nil, stdout, stderr)
			fmt.Fprintln(stdout, "::endgroup::")
			if code != 0 {
				fmt.Fprintln(stdout, annotate("error", fmt.Sprintf("%s exited %d", whole.Display, code)))
			}
			statuses = append(statuses, info)
			if code > worst {
				worst = code
			}
		}
	}
	for _, c := range cmds {
		one(c)
	}
	return worst, statuses
}

var libtestRunning = regexp.MustCompile(`(?m)^running (\d+) tests?$`)

// ranNothing reports whether libtest's output shows no test ran: no `running
// N tests` line, or only zeros.
func ranNothing(output string) bool {
	counts := libtestRunning.FindAllStringSubmatch(stripANSI(output), -1)
	for _, m := range counts {
		if m[1] != "0" {
			return false
		}
	}
	return true
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

func runOne(ctx context.Context, root string, c Command, timeout time.Duration, logDir string, logs map[string]*os.File, capture *proc.CappedBuffer, stdout, stderr io.Writer) (ExitInfo, int) {
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
	if capture != nil {
		t := &teeWriter{a: out, b: capture}
		// Only a command whose two streams share one writer is captured whole;
		// a second writer on the capture would race the first. Filtered
		// commands are cargo's, which are.
		if errw == out {
			errw = t
		}
		out = t
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
