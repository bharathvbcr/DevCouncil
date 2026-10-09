package verify

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
)

// Source files, so Lappi — which asks only about code — would read them.
const secretSection = "diff --git a/secrets/prod.py b/secrets/prod.py\n--- a/secrets/prod.py\n+++ b/secrets/prod.py\n" +
	"@@ -1 +1 @@\n-DB_PASSWORD = 'x'\n+DB_PASSWORD = 'hunter2'\n" +
	"diff --git a/credentials/npm.go b/credentials/npm.go\n--- a/credentials/npm.go\n+++ b/credentials/npm.go\n" +
	"@@ -1 +1 @@\n-package x\n+const token = \"npm_SECRETTOKEN\"\n"

// recordingLappiAgent serves reply to every request and keeps what it was sent.
func recordingLappiAgent(t *testing.T, reply string) (string, func() []string) {
	t.Helper()
	dir, err := os.MkdirTemp("", "qd")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := os.RemoveAll(dir); err != nil {
			t.Errorf("cleanup: %v", err)
		}
	})
	path := filepath.Join(dir, "a.sock")
	l, err := net.Listen("unix", path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := l.Close(); err != nil {
			t.Errorf("close: %v", err)
		}
	})
	var mu sync.Mutex
	var seen []string
	go func() {
		for {
			conn, err := l.Accept()
			if err != nil {
				return
			}
			go func() {
				defer conn.Close()
				line, err := bufio.NewReader(conn).ReadBytes('\n')
				if err != nil {
					return
				}
				mu.Lock()
				seen = append(seen, string(line))
				mu.Unlock()
				_, _ = conn.Write([]byte(reply + "\n"))
			}()
		}
	}()
	return path, func() []string {
		mu.Lock()
		defer mu.Unlock()
		return append([]string(nil), seen...)
	}
}

// Lappi is a model. It is shown the diff by the rule get_diff applies, so a
// secret path's hunks never enter its context.
func TestLappiIsNeverShownASecretPathsHunks(t *testing.T) {
	// An answering agent, and the secret sections first, so the ask cannot
	// stop before reaching them.
	socket, sent := recordingLappiAgent(t, logicReply)
	in := runInput(askerFor(t, socket))
	in.DiffContent = secretSection + runDiff
	in.ChangedFiles = append(in.ChangedFiles, "secrets/prod.py", "credentials/npm.go")
	Run(context.Background(), in)
	requests := sent()
	if len(requests) == 0 {
		t.Fatal("Lappi was not asked about the ordinary file, so this test proves nothing")
	}
	for _, line := range requests {
		// The file's context travels base64-encoded; read what the model reads.
		var req struct {
			ContextB64 string `json:"context_b64"`
		}
		if err := json.Unmarshal([]byte(line), &req); err != nil {
			t.Fatalf("request is not JSON: %v: %s", err, line)
		}
		context, err := base64.StdEncoding.DecodeString(req.ContextB64)
		if err != nil || len(context) == 0 {
			t.Fatalf("request carries no readable context: %v: %s", err, line)
		}
		r := string(context)
		for _, s := range []string{"hunter2", "npm_SECRETTOKEN", "secrets/prod.py", "credentials/npm.go"} {
			if strings.Contains(r, s) {
				t.Errorf("Lappi was sent %q: %s", s, r)
			}
		}
	}
}

// dcverify scans the whole diff — its secret gate exists to see credentials —
// but a line it quotes from a secret path does not travel on in the gap.
func TestRigorEvidenceFromASecretPathIsWithheld(t *testing.T) {
	gaps := gapsFromFindings("T-SECRET", []dcverify.Finding{
		{Gate: dcverify.GateStubDetection, Severity: "blocking", Path: "secrets/prod.py", Line: 3,
			Evidence: "raise NotImplementedError('hunter2')", Message: "added a placeholder"},
		{Gate: dcverify.GateStubDetection, Severity: "blocking", Path: "src/a.py", Line: 4,
			Evidence: "raise NotImplementedError()", Message: "added a placeholder"},
	})
	if len(gaps) != 2 {
		t.Fatalf("want two gaps, got %+v", gaps)
	}
	if got := strings.Join(gaps[0].Evidence, "\n"); strings.Contains(got, "hunter2") ||
		!strings.Contains(got, "withheld: secret_path") || !strings.Contains(got, "secrets/prod.py:3") {
		t.Errorf("secret-path evidence was not withheld: %q", got)
	}
	if got := strings.Join(gaps[1].Evidence, "\n"); !strings.Contains(got, "NotImplementedError()") {
		t.Errorf("ordinary evidence was lost: %q", got)
	}
}
