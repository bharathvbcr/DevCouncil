//go:build unix

package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// helperServerEnv gates TestHelperMCPServer: the test binary re-runs itself as
// the fake server, so the fake decodes frames instead of pattern-matching them.
const (
	helperServerEnv = "DEVCOUNCIL_TEST_MCP_FAKE"
	helperToolsEnv  = "DEVCOUNCIL_TEST_MCP_FAKE_TOOLS"
)

// fakeServer returns a spec for a server that answers initialize and
// tools/list with the given tools, or never answers when tools is nil.
func fakeServer(t *testing.T, tools []string, want []string) mcpServerSpec {
	t.Helper()
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	env := []string{helperServerEnv + "=1"}
	if tools != nil {
		env = append(env, helperToolsEnv+"="+strings.Join(tools, ","))
	}
	return mcpServerSpec{name: "x", bin: self, args: []string{"-test.run=^TestHelperMCPServer$"}, env: env, want: want}
}

// TestHelperMCPServer is the fake server's process body, not a test: it is
// inert unless the spec's environment asks for it.
func TestHelperMCPServer(t *testing.T) {
	if os.Getenv(helperServerEnv) != "1" {
		t.Skip("runs only as the fake MCP server's process")
	}
	tools, answers := os.LookupEnv(helperToolsEnv)
	if !answers {
		_, _ = io.Copy(io.Discard, os.Stdin)
		os.Exit(0)
	}
	var list []string
	for _, name := range strings.Split(tools, ",") {
		if name != "" {
			list = append(list, `{"name":"`+name+`"}`)
		}
	}
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(nil, 1<<20)
	out := os.Stdout
	for in.Scan() {
		var req struct {
			ID     jsontext.Value `json:"id"`
			Method string         `json:"method"`
		}
		if json.Unmarshal(in.Bytes(), &req) != nil {
			continue
		}
		switch req.Method {
		case "initialize":
			fmt.Fprintf(out, "%s\n%s\n", `{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}`, `{"jsonrpc":"2.0","id":`+string(req.ID)+`,"result":{}}`)
		case "tools/list":
			fmt.Fprintf(out, "%s\n", `{"jsonrpc":"2.0","id":`+string(req.ID)+`,"result":{"tools":[`+strings.Join(list, ",")+`]}}`)
		}
	}
	os.Exit(0)
}

// A server may receive a frame's members in any order (the probe now writes
// them deterministically, but other clients need not). The fake must answer
// from the decoded id, not from where the text sits; with a substring-matching fake the
// probe tests hung to their budget on a fraction of runs, loaded or not.
func TestFakeServerAnswersWhereverIDSitsInTheFrame(t *testing.T) {
	cases := []struct {
		frame, wantID string
	}{
		{`{"jsonrpc":"2.0","id":1,"method":"initialize"}`, "1"},
		{`{"jsonrpc":"2.0","method":"initialize","id":1}`, "1"},
		{`{"method":"initialize","id":1,"jsonrpc":"2.0"}`, "1"},
		{`{"jsonrpc":"2.0","id": 1,"method":"initialize"}`, "1"},
		{`{"jsonrpc":"2.0","method":"initialize","id" : 1 }`, "1"},
		{`{"jsonrpc":"2.0","id":2,"method":"tools/list"}`, "2"},
		{`{"jsonrpc":"2.0","method":"tools/list","id":2}`, "2"},
		{`{"id":2,"method":"tools/list","jsonrpc":"2.0"}`, "2"},
		{`{"jsonrpc":"2.0","id":12,"method":"tools/list"}`, "12"},
		{`{"jsonrpc":"2.0","method":"tools/list","id":12}`, "12"},
		{`{"jsonrpc":"2.0","method":"tools/list","params":{"id":2},"id":7}`, "7"},
		{`{"jsonrpc":"2.0","params":{"id":1},"id":9,"method":"initialize"}`, "9"},
	}
	for _, c := range cases {
		spec := fakeServer(t, []string{"a"}, nil)
		ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
		defer cancel()
		cmd := exec.CommandContext(ctx, spec.bin, spec.args...)
		cmd.WaitDelay = 2 * time.Second
		cmd.Env = append(os.Environ(), spec.env...)
		cmd.Stdin = strings.NewReader(c.frame + "\n")
		out, err := cmd.Output()
		if err != nil {
			t.Fatalf("frame %s: %v", c.frame, err)
		}
		var answered bool
		for _, line := range strings.Split(string(out), "\n") {
			var r struct {
				ID jsontext.Value `json:"id"`
			}
			if json.Unmarshal([]byte(line), &r) == nil && string(r.ID) == c.wantID {
				answered = true
			}
		}
		if !answered {
			t.Errorf("frame %s was not answered with id %s: %q", c.frame, c.wantID, out)
		}
	}
}

// sendFrame writes JSON-RPC frames built from maps. encoding/json/v2 emits a
// map's members in iteration order unless told otherwise, so the same frame
// reached a server with its members in a different order from one probe to the
// next: the id came last about one time in three, and a server that matches on
// the id as it reads the line never saw a request it could answer. The probe
// then waited out its whole budget and reported "no answer".
func TestSendFrameWritesTheSameBytesEveryTime(t *testing.T) {
	frames := map[string]map[string]any{
		"initialize": {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": map[string]any{
			"protocolVersion": mcpProbeProtocol, "capabilities": map[string]any{},
			"clientInfo": map[string]any{"name": "devcouncil-doctor", "version": "test"},
		}},
		"notification": {"jsonrpc": "2.0", "method": "notifications/initialized"},
		"tools/list":   {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
	}
	for name, frame := range frames {
		var first []byte
		for i := 0; i < 300; i++ {
			var buf bytes.Buffer
			if err := sendFrame(&buf, frame); err != nil {
				t.Fatalf("%s: %v", name, err)
			}
			if first == nil {
				first = buf.Bytes()
				if !jsontext.Value(first).IsValid() || !bytes.HasSuffix(first, []byte("\n")) {
					t.Fatalf("%s: not one JSON line: %q", name, first)
				}
				continue
			}
			if !bytes.Equal(first, buf.Bytes()) {
				t.Fatalf("%s: write %d differs from write 0:\n%s\n%s", name, i, first, buf.Bytes())
			}
		}
	}
}

func TestProbeReportsAServerThatAnswers(t *testing.T) {
	got := probeMCPServer(context.Background(), fakeServer(t, []string{"a", "b"}, []string{"a", "b"}), 10*time.Second)
	if got.Status != "ok" || got.Tools != 2 {
		t.Fatalf("probe = %+v", got)
	}
}

func TestProbeNamesToolsTheServerDoesNotServe(t *testing.T) {
	got := probeMCPServer(context.Background(), fakeServer(t, []string{"a", "c"}, []string{"a", "b"}), 10*time.Second)
	if got.Status != "failed" || strings.Join(got.MissingTools, ",") != "b" || strings.Join(got.ExtraTools, ",") != "c" {
		t.Fatalf("probe = %+v", got)
	}
}

func TestProbeOfAHungServerFailsWithinItsBudget(t *testing.T) {
	started := time.Now()
	got := probeMCPServer(context.Background(), fakeServer(t, nil, nil), 500*time.Millisecond)
	if got.Status != "failed" || !strings.Contains(got.Reason, "no answer within 500ms") {
		t.Fatalf("probe = %+v", got)
	}
	if elapsed := time.Since(started); elapsed > 10*time.Second {
		t.Fatalf("a hung server held the probe %s past a 500ms budget", elapsed)
	}
}

func TestProbeThatCouldNotStartIsNotRunNeverOK(t *testing.T) {
	for _, spec := range []mcpServerSpec{
		{name: "missing", notRun: "devmap binary not found"},
		{name: "unrunnable", bin: filepath.Join(t.TempDir(), "absent")},
	} {
		got := probeMCPServer(context.Background(), spec, time.Second)
		if got.Status != "not_run" || got.Reason == "" {
			t.Errorf("%s: probe = %+v", spec.name, got)
		}
	}
}
