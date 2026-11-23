//go:build unix

package main

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"fmt"
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
		time.Sleep(30 * time.Second)
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

// The probe marshals its frames from maps, and encoding/json/v2 does not sort
// map keys, so "id" lands anywhere in the line. The fake must answer from the
// decoded id, not from where the text sits; with a substring-matching fake the
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
		cmd := exec.Command(spec.bin, spec.args...)
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
