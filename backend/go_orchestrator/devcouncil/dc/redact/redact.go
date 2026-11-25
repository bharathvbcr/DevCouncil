// Package redact provides a bounded JSON-on-stdio Go client for the Rust
// `dcredact` binary (rust/dc-redact-cli) and a fail-closed pure-Go fallback
// scrubber for defense-in-depth egress scrubbing of PR Gate outputs.
//
// Security invariants:
//   - Binary discovery uses proc.LookPathOutside / proc.ResolveComponent and
//     refuses any binary inside the repository under analysis.
//   - Subprocess execution is bounded via proc.ConfigureGroup, proc.CappedBuffer,
//     and proc.RunBounded.
//   - Fail closed: on any subprocess failure, crash, timeout, oversize reply,
//     schema mismatch, `ok: false`, or `redaction_ran: false`, Redact returns an
//     error and NEVER returns unscrubbed input text.
//   - Error messages never echo stdin or full stdout so secrets cannot leak into
//     logs or error strings.
package redact

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

const (
	// BinaryEnv is the optional environment variable overriding dcredact discovery.
	BinaryEnv = "DCREDACT_BIN"

	// ComponentID matches rust/dc-redact-cli/src/bin/dcredact.rs COMPONENT_ID.
	ComponentID = "dcredact"

	// Identity matches rust/dc-redact-cli/src/lib.rs IDENTITY.
	Identity = "dc-redact"

	// SchemaVersion matches rust/dc-redact-cli/src/lib.rs SCHEMA_VERSION.
	SchemaVersion = 1

	// MaxTextBytes matches rust/dc-redact-cli/src/lib.rs MAX_TEXT_BYTES (1 MiB).
	MaxTextBytes = 1024 * 1024

	defaultTimeout  = 15 * time.Second
	maxOutput       = 8 << 20 // 8 MiB
	maxStderr       = 64 << 10
	waitDelay       = 2 * time.Second
	stderrEchoLimit = 256
)

// ErrNoBinary is returned when a dcredact binary is required but not configured.
var ErrNoBinary = errors.New("no dcredact binary configured")

// Client invokes the `dcredact` binary over JSON-on-stdio.
type Client struct {
	// Binary is the absolute path to the `dcredact` executable.
	Binary string
	// Timeout bounds a single invocation. Zero uses defaultTimeout.
	Timeout time.Duration

	maxOutput int
	maxStderr int
}

// New builds a dcredact client with bounded defaults.
func New(binary string) *Client {
	return &Client{
		Binary:    binary,
		maxOutput: maxOutput,
		maxStderr: maxStderr,
	}
}

// LookPath resolves `dcredact` on PATH strictly outside the repository containing root.
func LookPath(root string) (string, error) {
	return proc.LookPathOutside(ComponentID, root)
}

// ResolveOutside resolves `dcredact` strictly outside root.
//
// When explicitPath is non-empty, it is resolved and refused if missing, a
// directory, or located inside root (following symlinks). When explicitPath is
// empty, DCREDACT_BIN is checked first (still refused if inside root), then
// PATH via proc.LookPathOutside and ~/.local/bin via proc.ResolveComponent.
func ResolveOutside(root, explicitPath string) (*Client, error) {
	override := proc.EnvOverride(BinaryEnv)
	if explicitPath != "" {
		override = proc.Override{Path: explicitPath, Source: "--dcredact"}
	}
	bin, err := proc.ResolveComponent(ComponentID, root, override)
	if err != nil {
		return nil, err
	}
	if err := ensureOutsideRepo(root, bin); err != nil {
		return nil, err
	}
	return New(bin), nil
}

func ensureOutsideRepo(root, bin string) error {
	if root == "" {
		return nil
	}
	resolvedRoot, err := filepath.EvalSymlinks(root)
	if err != nil {
		resolvedRoot = filepath.Clean(root)
	}
	resolvedRoot, err = filepath.Abs(resolvedRoot)
	if err != nil {
		return err
	}
	resolvedBin, err := filepath.EvalSymlinks(bin)
	if err != nil {
		resolvedBin = filepath.Clean(bin)
	}
	resolvedBin, err = filepath.Abs(resolvedBin)
	if err != nil {
		return err
	}
	rel, err := filepath.Rel(resolvedRoot, resolvedBin)
	if err != nil {
		return nil
	}
	if rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(os.PathSeparator))) {
		return fmt.Errorf("refusing %s binary %q inside repository under analysis: %w", ComponentID, bin, proc.ErrRepositoryLocal)
	}
	return nil
}

// HealthReply is the wire payload of `dcredact health`.
type HealthReply struct {
	OK            bool   `json:"ok"`
	Error         string `json:"error,omitempty"`
	ID            string `json:"id"`
	Component     string `json:"component"`
	Version       string `json:"version"`
	SchemaVersion *int   `json:"schema_version"`
	Limits        struct {
		MaxRequestBytes uint64 `json:"max_request_bytes"`
		MaxTextBytes    int    `json:"max_text_bytes"`
	} `json:"limits"`
}

// Result is the validated wire payload of `dcredact redact`.
type Result struct {
	OK            bool     `json:"ok"`
	Error         string   `json:"error,omitempty"`
	ID            string   `json:"id"`
	Component     string   `json:"component"`
	SchemaVersion *int     `json:"schema_version"`
	RedactionRan  bool     `json:"redaction_ran"`
	Text          string   `json:"text"`
	RedactedLines int      `json:"redacted_lines"`
	RedactedSpans int      `json:"redacted_spans"`
	Kinds         []string `json:"kinds"`
	InputBytes    int      `json:"input_bytes"`
	OutputBytes   int      `json:"output_bytes"`
}

type redactRequest struct {
	Text string `json:"text"`
}

// Health probes the configured `dcredact` binary and validates its identity and schema version.
func (c *Client) Health(ctx context.Context) (*HealthReply, error) {
	if c == nil || c.Binary == "" {
		return nil, ErrNoBinary
	}
	var reply HealthReply
	if err := c.run(ctx, []string{"health"}, nil, &reply); err != nil {
		return nil, err
	}
	if reply.ID != ComponentID || reply.Component != Identity {
		return nil, fmt.Errorf("dcredact health identity mismatch: id=%q component=%q (want %q / %q)",
			reply.ID, reply.Component, ComponentID, Identity)
	}
	if reply.SchemaVersion == nil || *reply.SchemaVersion != SchemaVersion {
		return nil, fmt.Errorf("dcredact health schema_version mismatch (want %d)", SchemaVersion)
	}
	return &reply, nil
}

// Redact invokes `dcredact redact` over JSON-on-stdio and validates that
// redaction actually ran (`ok: true`, `redaction_ran: true`, matching identity
// and schema version). Never returns the input text on error.
func (c *Client) Redact(ctx context.Context, text string) (*Result, error) {
	if c == nil || c.Binary == "" {
		return nil, ErrNoBinary
	}
	if len(text) > MaxTextBytes {
		return nil, fmt.Errorf("dcredact: input text (%d bytes) exceeds %d-byte limit", len(text), MaxTextBytes)
	}
	payload, err := json.Marshal(redactRequest{Text: text})
	if err != nil {
		return nil, fmt.Errorf("dcredact: marshal request: %w", err)
	}

	var reply Result
	if err := c.run(ctx, []string{"redact"}, payload, &reply); err != nil {
		return nil, err
	}
	if !reply.RedactionRan {
		return nil, errors.New("dcredact: reply did not confirm redaction_ran=true")
	}
	if reply.ID != ComponentID || reply.Component != Identity {
		return nil, fmt.Errorf("dcredact: reply identity mismatch: id=%q component=%q", reply.ID, reply.Component)
	}
	if reply.SchemaVersion == nil || *reply.SchemaVersion != SchemaVersion {
		return nil, fmt.Errorf("dcredact: reply schema_version mismatch (want %d)", SchemaVersion)
	}
	if reply.InputBytes != len(text) {
		return nil, fmt.Errorf("dcredact: input_bytes=%d disagrees with sent %d bytes", reply.InputBytes, len(text))
	}
	if reply.OutputBytes != len(reply.Text) {
		return nil, fmt.Errorf("dcredact: output_bytes=%d disagrees with returned %d bytes", reply.OutputBytes, len(reply.Text))
	}
	return &reply, nil
}

func (c *Client) effectiveTimeout() time.Duration {
	if c != nil && c.Timeout > 0 {
		return c.Timeout
	}
	return defaultTimeout
}

func (c *Client) outputBound() int {
	if c == nil || c.maxOutput <= 0 {
		return maxOutput
	}
	return c.maxOutput
}

func (c *Client) stderrBound() int {
	if c == nil || c.maxStderr <= 0 {
		return maxStderr
	}
	return c.maxStderr
}

func (c *Client) run(ctx context.Context, args []string, stdin []byte, out any) error {
	timeout := c.effectiveTimeout()
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	// #nosec G204 -- c.Binary is a validated dcredact path outside the repo,
	// and args are fixed subcommand literals ("health" or "redact").
	cmd := exec.CommandContext(ctx, c.Binary, args...)
	proc.ConfigureGroup(cmd)
	if stdin != nil {
		cmd.Stdin = bytes.NewReader(stdin)
	} else if devNull, err := os.Open(os.DevNull); err == nil {
		defer devNull.Close()
		cmd.Stdin = devNull
	}
	stdout := &proc.CappedBuffer{Limit: c.outputBound()}
	stderr := &proc.CappedBuffer{Limit: c.stderrBound()}
	cmd.Stdout = stdout
	cmd.Stderr = stderr
	cmd.WaitDelay = waitDelay

	runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		return fmt.Errorf("dcredact timed out after %s", timeout)
	}
	if stdout.Overflowed() {
		return fmt.Errorf("dcredact produced more than %d bytes", c.outputBound())
	}

	raw := bytes.TrimSpace(stdout.Bytes())
	if len(raw) == 0 {
		if runErr != nil {
			return fmt.Errorf("dcredact failed: %w (stderr, %d bytes: %s)", runErr, stderr.Len(), boundedHead(stderr.Bytes(), stderrEchoLimit))
		}
		return errors.New("dcredact returned empty output")
	}
	if err := json.Unmarshal(raw, out); err != nil {
		if runErr != nil {
			return fmt.Errorf("dcredact failed: %w (stderr, %d bytes: %s)", runErr, stderr.Len(), boundedHead(stderr.Bytes(), stderrEchoLimit))
		}
		return fmt.Errorf("dcredact returned unparseable output: %w (%d bytes on stdout)", err, stdout.Len())
	}

	ok, reason := okOf(out)
	if !ok {
		if reason == "" {
			reason = "dcredact returned ok=false"
		}
		return errors.New(reason)
	}
	if runErr != nil {
		return fmt.Errorf("dcredact exited non-zero despite ok=true: %w", runErr)
	}
	return nil
}

func okOf(out any) (bool, string) {
	switch r := out.(type) {
	case *HealthReply:
		return r.OK, r.Error
	case *Result:
		return r.OK, r.Error
	default:
		return false, "unknown dcredact reply shape"
	}
}

func boundedHead(b []byte, limit int) string {
	b = bytes.TrimSpace(b)
	if len(b) <= limit {
		return string(b)
	}
	cut := limit
	for cut > 0 && !utf8.RuneStart(b[cut]) {
		cut--
	}
	return string(b[:cut]) + "…"
}

// Scrubber performs defense-in-depth egress scrubbing using `dcredact` when
// configured, plus the pure-Go fail-closed fallback scrubber.
type Scrubber struct {
	// Client is the optional `dcredact` subprocess client.
	Client *Client
	// RequireBinary forces Scrub to fail closed if Client is nil.
	RequireBinary bool
}

// Scrub redacts secret tokens from text fail-closed.
//
//   - If Client is configured, `dcredact` MUST succeed (`ok: true`,
//     `redaction_ran: true`); any crash, timeout, or error fails closed instead
//     of silently downgrading.
//   - Even after `dcredact` succeeds (or when no binary is configured and
//     RequireBinary is false), the pure-Go fallback scrubber runs and verifies
//     convergence so high-confidence secret patterns can never slip through.
func (s *Scrubber) Scrub(ctx context.Context, text string) (string, error) {
	current := text
	if s != nil && s.Client != nil {
		res, err := s.Client.Redact(ctx, current)
		if err != nil {
			return "", fmt.Errorf("egress redaction via dcredact failed: %w", err)
		}
		current = res.Text
	} else if s != nil && s.RequireBinary {
		return "", fmt.Errorf("egress redaction failed: %w", ErrNoBinary)
	}

	scrubbed, err := ScrubFallback(current)
	if err != nil {
		return "", fmt.Errorf("egress fallback redaction failed: %w", err)
	}
	return scrubbed, nil
}
