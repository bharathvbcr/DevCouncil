//go:build unix

package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// fakeServer writes a shell script that answers initialize and tools/list
// with the given tools, or never answers when tools is nil.
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
			"    *'\"id\":1,'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}' ;;\n" +
			"    *'\"id\":2,'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[" + strings.Join(list, ",") + "]}}' ;;\n" +
			"  esac\n" +
			"done\n"
	}
	path := filepath.Join(t.TempDir(), "server.sh")
	if err := os.WriteFile(path, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
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
