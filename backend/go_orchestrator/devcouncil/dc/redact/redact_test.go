package redact

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

func makeFakeDCRedact(t *testing.T, script string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("shell script fakes require unix")
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "dcredact")
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return bin
}

func TestScrubFallbackPatternsAndNoFalsePositives(t *testing.T) {
	// Construct test credentials dynamically so this test file itself does not
	// carry literal secret tokens in source.
	awsKey := "AKIA" + "IOSFODNN7EXAMPLE"
	ghpTok := "ghp_" + strings.Repeat("A1b2C3d4", 4) + "abcd"
	ghPat := "github_pat_" + strings.Repeat("11AbCdEf", 6)
	skKey := "sk-" + strings.Repeat("AbCd1234", 6)
	skAnt := "sk-ant-" + "api03-" + strings.Repeat("xY9z", 6)
	pemHdr := "-----BEGIN " + "RSA PRIVATE KEY-----"
	pemEnd := "-----END " + "RSA PRIVATE KEY-----"
	pemBlock := pemHdr + "\nMIIEpAIBAAKCAQEAtestbody\n" + pemEnd

	cases := []struct {
		name   string
		input  string
		secret string
		want   string
	}{
		{"aws_key", "key=" + awsKey + " ok", awsKey, "[REDACTED:aws-key]"},
		{"ghp_token", "token: " + ghpTok, ghpTok, "[REDACTED:github-token]"},
		{"github_pat", "pat=" + ghPat, ghPat, "[REDACTED:github-pat]"},
		{"openai_sk", "openai=" + skKey, skKey, "[REDACTED:sk-key]"},
		{"anthropic_sk", "ant=" + skAnt, skAnt, "[REDACTED:anthropic-key]"},
		{"pem_block", "cert:\n" + pemBlock + "\ndone", "MIIEpAIBAAKCAQEAtestbody", "[REDACTED:private-key]"},
		{"pem_header_only", "prefix " + pemHdr + " suffix", pemHdr, "[REDACTED:private-key]"},
		{"underscore_joined_aws_and_ghp", "pkg/" + awsKey + "_" + ghpTok + ".go", awsKey, "[REDACTED:aws-key]"},
		{"underscore_joined_ghp_part", "pkg/" + awsKey + "_" + ghpTok + ".go", ghpTok, "[REDACTED:github-token]"},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if !ContainsFallbackSecret(tc.input) {
				t.Fatalf("ContainsFallbackSecret(%q) = false, want true", tc.input)
			}
			got, err := ScrubFallback(tc.input)
			if err != nil {
				t.Fatalf("ScrubFallback error: %v", err)
			}
			if strings.Contains(got, tc.secret) {
				t.Fatalf("ScrubFallback leaked secret %q in %q", tc.secret, got)
			}
			if !strings.Contains(got, tc.want) {
				t.Fatalf("ScrubFallback = %q, want marker %q", got, tc.want)
			}
			if ContainsFallbackSecret(got) {
				t.Fatalf("scrubbed output still triggers ContainsFallbackSecret: %q", got)
			}
		})
	}

	// Normal PR Gate output (40-hex SHAs, paths, markdown, JSON) must not be altered.
	benign := "<!-- devcouncil:pr-gate -->\n- **Commit:** `0123456789abcdef0123456789abcdef01234567`\n- `pkg/disk-manager/task-runner.go:42`\n"
	got, err := ScrubFallback(benign)
	if err != nil {
		t.Fatalf("benign ScrubFallback error: %v", err)
	}
	if got != benign {
		t.Fatalf("benign text altered:\ngot:  %q\nwant: %q", got, benign)
	}
}

func TestClientHealthAndRedactSuccess(t *testing.T) {
	awsKey := "AKIA" + "IOSFODNN7EXAMPLE"
	bin := makeFakeDCRedact(t, `#!/bin/sh
if [ "$1" = "health" ]; then
  printf '%s\n' '{"ok":true,"id":"dcredact","component":"dc-redact","version":"0.2.5","schema_version":1,"limits":{"max_request_bytes":8388608,"max_text_bytes":1048576}}'
  exit 0
fi
if [ "$1" = "redact" ]; then
  cat >/dev/null
  printf '%s\n' '{"ok":true,"id":"dcredact","component":"dc-redact","schema_version":1,"redaction_ran":true,"text":"clean AKIA… (20 chars)","redacted_lines":1,"redacted_spans":1,"kinds":["aws access key id"],"input_bytes":26,"output_bytes":24}'
  exit 0
fi
exit 2
`)
	c := New(bin)
	ctx := context.Background()
	h, err := c.Health(ctx)
	if err != nil {
		t.Fatalf("Health failed: %v", err)
	}
	if h.ID != ComponentID || h.Component != Identity {
		t.Fatalf("unexpected health: %+v", h)
	}

	input := "clean " + awsKey // 26 bytes
	res, err := c.Redact(ctx, input)
	if err != nil {
		t.Fatalf("Redact failed: %v", err)
	}
	if !res.RedactionRan || strings.Contains(res.Text, awsKey) {
		t.Fatalf("unexpected redact result: %+v", res)
	}

	// Defense-in-depth Scrubber combines dcredact + fallback seamlessly.
	scrubber := &Scrubber{Client: c, RequireBinary: true}
	out, err := scrubber.Scrub(ctx, input)
	if err != nil {
		t.Fatalf("Scrubber.Scrub failed: %v", err)
	}
	if strings.Contains(out, awsKey) || !strings.Contains(out, "AKIA… (20 chars)") {
		t.Fatalf("unexpected scrubbed output: %q", out)
	}
}

func TestClientFailClosedOnErrors(t *testing.T) {
	ctx := context.Background()

	t.Run("redaction_ran_false", func(t *testing.T) {
		bin := makeFakeDCRedact(t, `#!/bin/sh
cat >/dev/null
printf '%s\n' '{"ok":true,"id":"dcredact","component":"dc-redact","schema_version":1,"redaction_ran":false,"text":"raw","redacted_lines":0,"redacted_spans":0,"kinds":[],"input_bytes":3,"output_bytes":3}'
`)
		scrubber := &Scrubber{Client: New(bin), RequireBinary: true}
		out, err := scrubber.Scrub(ctx, "raw")
		if err == nil || out != "" {
			t.Fatalf("expected fail-closed error, got out=%q err=%v", out, err)
		}
	})

	t.Run("crash_exit_1", func(t *testing.T) {
		bin := makeFakeDCRedact(t, `#!/bin/sh
echo "boom" >&2
exit 1
`)
		scrubber := &Scrubber{Client: New(bin), RequireBinary: true}
		out, err := scrubber.Scrub(ctx, "hello")
		if err == nil || out != "" {
			t.Fatalf("expected fail-closed error on crash, got out=%q err=%v", out, err)
		}
	})

	t.Run("ok_false", func(t *testing.T) {
		bin := makeFakeDCRedact(t, `#!/bin/sh
cat >/dev/null
printf '%s\n' '{"ok":false,"redaction_ran":false,"error":"redaction did not converge"}'
exit 2
`)
		out, err := (&Scrubber{Client: New(bin)}).Scrub(ctx, "hello")
		if err == nil || out != "" {
			t.Fatalf("expected fail-closed error on ok=false, got out=%q err=%v", out, err)
		}
	})

	t.Run("timeout", func(t *testing.T) {
		bin := makeFakeDCRedact(t, `#!/bin/sh
sleep 5
`)
		c := New(bin)
		c.Timeout = 100 * time.Millisecond
		out, err := (&Scrubber{Client: c, RequireBinary: true}).Scrub(ctx, "hello")
		if err == nil || out != "" {
			t.Fatalf("expected fail-closed timeout error, got out=%q err=%v", out, err)
		}
	})

	t.Run("require_binary_nil_client", func(t *testing.T) {
		out, err := (&Scrubber{RequireBinary: true}).Scrub(ctx, "hello")
		if !errors.Is(err, ErrNoBinary) || out != "" {
			t.Fatalf("expected ErrNoBinary, got out=%q err=%v", out, err)
		}
	})

	t.Run("oversize_input", func(t *testing.T) {
		c := New("/nonexistent/dcredact")
		huge := strings.Repeat("a", MaxTextBytes+1)
		if _, err := c.Redact(ctx, huge); err == nil {
			t.Fatal("expected oversize input error")
		}
	})
}

func TestResolveOutsideRefusesRepoLocalBinary(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("shell script fakes require unix")
	}
	repo := t.TempDir()
	if err := os.MkdirAll(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	insideBin := filepath.Join(repo, "dcredact")
	if err := os.WriteFile(insideBin, []byte("#!/bin/sh\nexit 0\n"), 0o755); err != nil {
		t.Fatal(err)
	}

	// Explicit path inside repo must be refused.
	if _, err := ResolveOutside(repo, insideBin); !errors.Is(err, proc.ErrRepositoryLocal) {
		t.Fatalf("expected ErrRepositoryLocal for explicit repo-local binary, got %v", err)
	}

	// PATH pointing at repo must also be refused by LookPath / ResolveOutside.
	t.Setenv("PATH", repo+string(os.PathListSeparator)+os.Getenv("PATH"))
	t.Setenv(BinaryEnv, "")
	if _, err := LookPath(repo); !errors.Is(err, proc.ErrRepositoryLocal) {
		t.Fatalf("expected ErrRepositoryLocal from LookPath, got %v", err)
	}
	if _, err := ResolveOutside(repo, ""); !errors.Is(err, proc.ErrRepositoryLocal) {
		t.Fatalf("expected ErrRepositoryLocal from ResolveOutside, got %v", err)
	}
}

func TestRealDCRedactBinaryInterop(t *testing.T) {
	// Locate built rust/target/debug/dcredact relative to this package.
	candidate := filepath.Join("..", "..", "..", "..", "..", "rust", "target", "debug", "dcredact")
	if runtime.GOOS == "windows" {
		candidate += ".exe"
	}
	abs, err := filepath.Abs(candidate)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(abs); err != nil {
		t.Skipf("real dcredact binary not built at %s: %v", abs, err)
	}

	c := New(abs)
	ctx := context.Background()
	h, err := c.Health(ctx)
	if err != nil {
		t.Fatalf("real dcredact Health failed: %v", err)
	}
	if !h.OK || h.ID != ComponentID || h.Component != Identity || h.SchemaVersion == nil || *h.SchemaVersion != SchemaVersion {
		t.Fatalf("unexpected real health reply: %+v", h)
	}

	awsKey := "AKIA" + "IOSFODNN7EXAMPLE"
	ghpTok := "ghp_" + strings.Repeat("A1b2C3d4", 4) + "abcd"
	pemHdr := "-----BEGIN " + "RSA PRIVATE KEY-----"
	sample := "path: " + awsKey + "\ntoken: " + ghpTok + "\nkey: " + pemHdr + "\n"

	res, err := c.Redact(ctx, sample)
	if err != nil {
		t.Fatalf("real dcredact Redact failed: %v", err)
	}
	if !res.RedactionRan || res.RedactedSpans < 3 {
		t.Fatalf("expected >=3 redacted spans, got %+v", res)
	}
	for _, secret := range []string{awsKey, ghpTok, pemHdr} {
		if strings.Contains(res.Text, secret) {
			t.Fatalf("real dcredact leaked %q in %q", secret, res.Text)
		}
	}

	// Verify Scrubber (dcredact + fallback defense in depth) succeeds cleanly.
	scrubbed, err := (&Scrubber{Client: c, RequireBinary: true}).Scrub(ctx, sample)
	if err != nil {
		t.Fatalf("Scrubber with real dcredact failed: %v", err)
	}
	for _, secret := range []string{awsKey, ghpTok, pemHdr} {
		if strings.Contains(scrubbed, secret) {
			t.Fatalf("Scrubber leaked %q in %q", secret, scrubbed)
		}
	}
}
