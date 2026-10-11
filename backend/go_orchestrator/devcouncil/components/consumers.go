package components

// After an install, ask whether every app that reads DevCouncil's schemas
// reads the numbers this checkout declares. DevCouncil owns devmap-store's
// CURRENT_SCHEMA_VERSION, dc-store's SCHEMA_VERSION and WORKBENCH_SCHEMA;
// GitPulse vendors them, Manvi links them and Jarvis pins Manvi and
// DevCouncil. An owner that installs a new number while an installed reader
// still carries the old one has not finished the change.
//
// The check itself is GitPulse's scripts/check-schema-parity.mjs: it already
// knows where each reader lives. DevCouncil does not depend on GitPulse, so the
// check is found, never required. When it cannot be found or cannot run, the
// report says the consumers were not checked and why — never anything that
// reads as a pass. A check that did not run and a check that passed are
// different outcomes all the way to the exit code under --strict-consumers.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

const (
	// GitPulseRootEnv names a GitPulse checkout, as GitPulse's
	// GITPULSE_MANVI_ROOT names a Manvi one. --gitpulse DIR wins over it.
	GitPulseRootEnv = "DEVCOUNCIL_GITPULSE_ROOT"
	// parityScript is the check, relative to the GitPulse checkout.
	parityScript = "scripts/check-schema-parity.mjs"
	// consumerCheckTimeout bounds the whole check, including every reader it
	// asks for a version.
	consumerCheckTimeout = 2 * time.Minute
	// consumerCheckOutputLimit caps what the check may print. A capped prefix
	// is not an answer, so an overrun is "not checked".
	consumerCheckOutputLimit = 1 << 20
)

// ConsumerRow is one reader the check asked.
type ConsumerRow struct {
	Reader  string `json:"reader"`
	Verdict string `json:"verdict"`
	Detail  string `json:"detail,omitempty"`
}

// ConsumerReport is what the install says about the schema consumers.
// Checked false means the check did not run to an answer; Reason says why,
// and nothing else in the report may be read as a verdict.
type ConsumerReport struct {
	Checked bool   `json:"checked"`
	Reason  string `json:"reason,omitempty"`
	// GitPulse is the checkout whose check ran (or was looked for).
	GitPulse string `json:"gitpulse,omitempty"`
	// DevCouncil is the checkout the check read the owner's numbers from.
	DevCouncil string `json:"devcouncil,omitempty"`
	// OK is the check's own verdict: every present reader matches the owner.
	OK bool `json:"ok"`
	// Owner holds the numbers the check read from this checkout.
	Owner   json.RawMessage `json:"owner,omitempty"`
	Rows    []ConsumerRow   `json:"rows,omitempty"`
	Summary string          `json:"summary,omitempty"`
}

func notChecked(gitpulse, format string, args ...any) ConsumerReport {
	return ConsumerReport{GitPulse: gitpulse, Reason: fmt.Sprintf(format, args...)}
}

// Lagging lists the rows that are not a match and not merely absent: the
// readers that must be brought up before the change is done.
func (r ConsumerReport) Lagging() []ConsumerRow {
	var out []ConsumerRow
	for _, row := range r.Rows {
		if row.Verdict != "match" && row.Verdict != "absent" {
			out = append(out, row)
		}
	}
	return out
}

// Passed is true only for a check that ran and found every present reader
// matching. A check that did not run never passes.
func (r ConsumerReport) Passed() bool {
	return r.Checked && r.OK && len(r.Lagging()) == 0
}

// findGitPulse returns the GitPulse checkout to run the check from. An
// explicit choice (flag, then environment) that does not hold the check is an
// answer in itself; it never falls through to the sibling guess.
func findGitPulse(flag, src string) (root string, why string) {
	explicit, from := strings.TrimSpace(flag), "--gitpulse"
	if explicit == "" {
		explicit, from = strings.TrimSpace(os.Getenv(GitPulseRootEnv)), GitPulseRootEnv
	}
	if explicit != "" {
		if hasParityScript(explicit) {
			return explicit, ""
		}
		return "", fmt.Sprintf("%s=%s has no %s", from, explicit, parityScript)
	}
	if src == "" {
		return "", "no GitPulse checkout given (--gitpulse DIR or " + GitPulseRootEnv + ")"
	}
	sibling := filepath.Join(filepath.Dir(src), "GitPulse")
	if hasParityScript(sibling) {
		return sibling, ""
	}
	if _, err := os.Stat(sibling); err == nil {
		return "", fmt.Sprintf("%s has no %s (a GitPulse older than the parity check)", sibling, parityScript)
	}
	return "", fmt.Sprintf("no GitPulse checkout at %s; pass --gitpulse DIR or set %s", sibling, GitPulseRootEnv)
}

func hasParityScript(root string) bool {
	info, err := os.Stat(filepath.Join(root, filepath.FromSlash(parityScript)))
	return err == nil && info.Mode().IsRegular()
}

// consumerCheck carries the seams the tests replace.
type consumerCheck struct {
	src      string
	gitpulse string // --gitpulse
	lookPath func(string) (string, error)
	timeout  time.Duration
	limit    int
}

// CheckConsumers runs GitPulse's schema-parity check against the DevCouncil
// checkout src. Every way it can fail to produce an answer is a report with
// Checked false.
func CheckConsumers(src, gitpulse string, o Options) ConsumerReport {
	return consumerCheck{src: src, gitpulse: gitpulse, lookPath: o.lookPath()}.run()
}

func (c consumerCheck) run() ConsumerReport {
	if c.src == "" {
		return notChecked("", "no DevCouncil checkout to read the owner's schema numbers from (this install came from cargo install --git)")
	}
	root, why := findGitPulse(c.gitpulse, c.src)
	if root == "" {
		return notChecked("", "%s", why)
	}
	lookPath := c.lookPath
	if lookPath == nil {
		lookPath = exec.LookPath
	}
	node, err := lookPath("node")
	if err != nil {
		return notChecked(root, "node is not on PATH, and the check is a node script")
	}
	timeout, limit := c.timeout, c.limit
	if timeout <= 0 {
		timeout = consumerCheckTimeout
	}
	if limit <= 0 {
		limit = consumerCheckOutputLimit
	}

	ctx, cancel := context.WithTimeout(console.Context(), timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, node, filepath.FromSlash(parityScript), "--json")
	proc.ConfigureGroup(cmd)
	cmd.Dir = root
	cmd.Env = append(os.Environ(), "GITPULSE_DEVCOUNCIL_ROOT="+c.src)
	var stdout, stderr proc.CappedBuffer
	stdout.Limit, stderr.Limit = limit, 64<<10
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	cmd.WaitDelay = 2 * time.Second
	runErr, timedOut := proc.RunBoundedWithCleanup(ctx, cmd.Run)
	if timedOut {
		// The run was abandoned; its buffers may still be written. Quote nothing.
		if errors.Is(ctx.Err(), context.Canceled) {
			return notChecked(root, "the check was cancelled")
		}
		return notChecked(root, "the check did not finish within %s", timeout)
	}
	if stdout.Overflowed() {
		return notChecked(root, "the check printed more than %d bytes", limit)
	}
	code := 0
	if runErr != nil {
		var exitErr *exec.ExitError
		if !errors.As(runErr, &exitErr) {
			return notChecked(root, "the check could not start: %v", runErr)
		}
		code = exitErr.ExitCode()
	}
	return parseParity(root, code, stdout.Bytes(), stderr.String())
}

// parityOutput is what `check-schema-parity.mjs --json` prints: one line,
// last on stdout. When DevCouncil's constants cannot be read it is
// {ok:false, error, rows:[]} with no owner.
type parityOutput struct {
	OK         *bool           `json:"ok"`
	Error      string          `json:"error"`
	Owner      json.RawMessage `json:"owner"`
	DevCouncil string          `json:"devcouncil"`
	Rows       []ConsumerRow   `json:"rows"`
	Summary    string          `json:"summary"`
}

var parityVerdicts = map[string]bool{"match": true, "stale": true, "unverifiable": true, "absent": true}

// parseParity turns the check's exit status and output into a report. Exit 0
// and 1 carry an answer; anything else, or an answer that does not hold
// together, is "not checked" — never read as a verdict.
func parseParity(root string, code int, stdout []byte, stderr string) ConsumerReport {
	if code != 0 && code != 1 {
		why := fmt.Sprintf("the check exited %d", code)
		if code == 2 {
			why += " (usage error)"
		}
		if line := firstLine(stderr); line != "" {
			why += ": " + line
		}
		return notChecked(root, "%s", why)
	}
	lines := strings.Split(strings.TrimSpace(string(stdout)), "\n")
	last := strings.TrimSpace(lines[len(lines)-1])
	var out parityOutput
	if last == "" || json.Unmarshal([]byte(last), &out) != nil || out.OK == nil {
		why := fmt.Sprintf("the check exited %d without a JSON answer", code)
		if line := firstLine(stderr); line != "" {
			why += ": " + line
		}
		return notChecked(root, "%s", why)
	}
	// The exit status and the answer must agree; a disagreement is a check
	// that cannot be trusted either way.
	if *out.OK != (code == 0) {
		return notChecked(root, "the check exited %d but its answer says ok=%t", code, *out.OK)
	}
	for _, row := range out.Rows {
		if !parityVerdicts[row.Verdict] {
			return notChecked(root, "the check returned an unknown verdict %q for %s", row.Verdict, row.Reader)
		}
	}
	if len(out.Owner) == 0 || string(out.Owner) == "null" {
		// The owner's numbers are this checkout's own. Not reading them is
		// DevCouncil's failure, and the check compared nothing.
		reason := "DevCouncil's schema constants could not be read"
		if out.Error != "" {
			reason = out.Error
		}
		return ConsumerReport{GitPulse: root, Reason: "owner unreadable: " + reason}
	}
	return ConsumerReport{
		Checked:    true,
		GitPulse:   root,
		DevCouncil: out.DevCouncil,
		OK:         *out.OK,
		Owner:      out.Owner,
		Rows:       out.Rows,
		Summary:    out.Summary,
	}
}

// remedy is what brings a lagging reader up, keyed on the reader names the
// check prints. Bumping a pin is a commit in that repository that changes
// what it claims to build against, so it is reported and asked about, never
// done from here.
func remedy(reader, gitpulse string) string {
	switch {
	case strings.HasPrefix(reader, "GitPulse vendored crates"):
		return fmt.Sprintf("in %s: node scripts/vendor-crates.mjs, then bun run check:vendor-schema, then commit (ask first)", gitpulse)
	case strings.HasPrefix(reader, "gitpulse-mcp"):
		return fmt.Sprintf("in %s: bun run mcp:install, then bun run mcp:doctor", gitpulse)
	case strings.HasPrefix(reader, "GitPulse.app"):
		return fmt.Sprintf("in %s: re-vendor if needed, bun run tauri build, and reinstall GitPulse.app (docs/INSTALLATION.md)", gitpulse)
	case strings.HasPrefix(reader, "devmap"), strings.HasPrefix(reader, "dcstore"):
		name := strings.Fields(reader)[0]
		return fmt.Sprintf("devcouncil install %s; if it still lags, an earlier PATH entry shadows the install (see the PATH notes above)", name)
	case strings.HasPrefix(reader, "Manvi pin"):
		return "bump Manvi's DevCouncil pin in scripts/module-pins.txt — a commit in Manvi: report the delta and ask before bumping"
	case strings.HasPrefix(reader, "Jarvis pin"):
		return "bump Jarvis's DevCouncil pin in upstream.lock.json and go.mod — a commit in Jarvis: report the delta and ask before bumping"
	}
	return "see the detail; the check names no remedy this installer knows"
}

// PrintConsumerReport writes the human report to stderr, beside the other
// post-install notes. It never prints a pass for a check that did not run.
func PrintConsumerReport(r ConsumerReport) {
	console.Errorln()
	if !r.Checked {
		console.Errorf("schema consumers: NOT CHECKED — %s\n", r.Reason)
		console.Errorln("  GitPulse, Manvi and Jarvis may still read older DevCouncil schemas; nothing here says otherwise.")
		return
	}
	console.Errorf("schema consumers (GitPulse check-schema-parity, %s): %s\n", r.GitPulse, r.Summary)
	if len(r.Owner) > 0 {
		console.Errorf("  this checkout writes %s\n", string(r.Owner))
	}
	for _, row := range r.Rows {
		console.Errorf("  %-13s %s — %s\n", row.Verdict, row.Reader, row.Detail)
	}
	lagging := r.Lagging()
	if len(lagging) > 0 {
		console.Errorln()
		console.Errorf("FOLLOW-UP: %d schema consumer(s) do not read what this checkout writes. The change is not done until each reports a match:\n", len(lagging))
		for _, row := range lagging {
			console.Errorf("  - %s (%s): %s\n", row.Reader, row.Verdict, remedy(row.Reader, r.GitPulse))
		}
		return
	}
	if !r.OK {
		// No reader lags, yet the check did not pass: nothing present matched.
		console.Errorln("FOLLOW-UP: no installed reader reported a match; the consumers were not shown to carry this checkout's schemas.")
	}
}

// firstLine is a one-line excerpt of a child's diagnostic output.
func firstLine(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i]
	}
	if len(s) > 200 {
		s = s[:200] + "…"
	}
	return s
}
