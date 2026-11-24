//go:build unix

package main

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// fakeServer writes a shell script that answers initialize and tools/list
// with the given tools, or never answers when tools is nil. It picks the
// request out by its "id" member and does not care where that member sits in
// the line: the script has no JSON parser, so it matches the member followed
// by either a comma or the closing brace.
func fakeServer(t *testing.T, tools []string) string {
	t.Helper()
	body := "#!/bin/sh\nexec sleep 30\n"
	if tools != nil {
		var list []string
		for _, name := range tools {
			list = append(list, `{"name":"`+name+`"}`)
		}
		body = "#!/bin/sh\n" +
			"while IFS= read -r line; do\n" +
			"  case \"$line\" in\n" +
			"    *'\"id\":1,'*|*'\"id\":1}'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}' ;;\n" +
			"    *'\"id\":2,'*|*'\"id\":2}'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[" + strings.Join(list, ",") + "]}}' ;;\n" +
			"  esac\n" +
			"done\n"
	}
	path := filepath.Join(t.TempDir(), "server.sh")
	if err := os.WriteFile(path, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
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
	got := probeMCPServer(context.Background(), mcpServerSpec{name: "x", bin: fakeServer(t, []string{"a", "b"}), want: []string{"a", "b"}}, 10*time.Second)
	if got.Status != "ok" || got.Tools != 2 {
		t.Fatalf("probe = %+v", got)
	}
}

func TestProbeNamesToolsTheServerDoesNotServe(t *testing.T) {
	got := probeMCPServer(context.Background(), mcpServerSpec{name: "x", bin: fakeServer(t, []string{"a", "c"}), want: []string{"a", "b"}}, 10*time.Second)
	if got.Status != "failed" || strings.Join(got.MissingTools, ",") != "b" || strings.Join(got.ExtraTools, ",") != "c" {
		t.Fatalf("probe = %+v", got)
	}
}

func TestProbeOfAHungServerFailsWithinItsBudget(t *testing.T) {
	started := time.Now()
	got := probeMCPServer(context.Background(), mcpServerSpec{name: "x", bin: fakeServer(t, nil)}, 500*time.Millisecond)
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
