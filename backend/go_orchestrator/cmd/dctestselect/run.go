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
	"time"
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

func runCmdPhase(ctx context.Context, args []string, stdout, stderr io.Writer) (int, error) {
	fs := flag.NewFlagSet("run", flag.ContinueOnError)
	planPath := fs.String("plan", "plan.json", "the plan to run")
	phase := fs.String("phase", "", "selected or full")
	logDir := fs.String("log-dir", ".", "where the full suite's logs are kept")
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
		return 2, err
	}
	if *phase == "selected" {
		if plan.Everything {
			fmt.Fprintln(stdout, annotate("notice", "the selection is everything; the fast phase is skipped and the full suite runs next"))
			return 0, nil
		}
		var cmds []Command
		for _, s := range plan.Selected {
			cmds = append(cmds, s.Command)
		}
		return runAll(ctx, root, cmds, *timeout, "", stdout, stderr), nil
	}
	if err := os.MkdirAll(*logDir, 0o755); err != nil {
		return 2, err
	}
	return runAll(ctx, root, plan.Full, *timeout, *logDir, stdout, stderr), nil
}

// runAll runs every command, failed ones included, and reports the worst
// outcome: 2 when a command could not run or was stopped, 1 when a command
// failed, 0 otherwise. Stopping at the first failure would hide the later
// ones, and the audit needs all of them.
func runAll(ctx context.Context, root string, cmds []Command, timeout time.Duration, logDir string, stdout, stderr io.Writer) int {
	worst := 0
	for _, c := range cmds {
		fmt.Fprintln(stdout, "::group::"+c.Display)
		code := runOne(ctx, root, c, timeout, logDir, stdout, stderr)
		fmt.Fprintln(stdout, "::endgroup::")
		if code != 0 {
			fmt.Fprintln(stdout, annotate("error", fmt.Sprintf("%s exited %d", c.Display, code)))
		}
		if code > worst {
			worst = code
		}
	}
	return worst
}

func runOne(ctx context.Context, root string, c Command, timeout time.Duration, logDir string, stdout, stderr io.Writer) int {
	var out, errw io.Writer = stdout, stderr
	if logDir != "" && c.Log != "" {
		if filepath.Base(c.Log) != c.Log {
			fmt.Fprintln(stderr, "dctestselect: refusing a log name that is a path:", c.Log)
			return 2
		}
		f, err := os.Create(filepath.Join(logDir, c.Log))
		if err != nil {
			fmt.Fprintln(stderr, "dctestselect:", err)
			return 2
		}
		defer f.Close()
		if c.GoJSON {
			out = &goJSONTee{log: f, console: stdout}
		} else {
			t := &teeWriter{a: stdout, b: f}
			out, errw = t, t
		}
	}
	dir := filepath.Join(root, filepath.FromSlash(c.Dir))
	err := runProcess(ctx, timeout, dir, c.Argv, out, errw)
	if g, ok := out.(*goJSONTee); ok {
		g.show(g.buf)
	}
	var exit *exec.ExitError
	switch {
	case err == nil:
		return 0
	case errors.As(err, &exit) && ctx.Err() == nil && exit.ExitCode() > 0:
		return 1
	default:
		fmt.Fprintln(stderr, "dctestselect:", err)
		return 2
	}
}
