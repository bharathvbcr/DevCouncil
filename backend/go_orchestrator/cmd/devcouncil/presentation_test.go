package main

import (
	"encoding/json"
	"os"
	"reflect"
	"testing"
)

func TestInstallJSONDryRunIsOneReceipt(t *testing.T) {
	stdout, restore := swapStdout(t)
	code := dispatch([]string{"install", "devmap", "--dry-run", "--json", "--prefix", t.TempDir()})
	restore()
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stdout.String())
	}
	var result struct {
		OK       bool     `json:"ok"`
		DryRun   bool     `json:"dry_run"`
		Commands []string `json:"commands"`
	}
	if err := json.Unmarshal(stdout.Bytes(), &result); err != nil {
		t.Fatalf("JSON flag produced non-JSON: %s (%v)", stdout.String(), err)
	}
	if !result.OK || !result.DryRun || len(result.Commands) == 0 {
		t.Fatalf("incomplete plan receipt: %s", stdout.String())
	}
}

func TestJSONOutputFailureCannotReportSuccess(t *testing.T) {
	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := reader.Close(); err != nil {
		t.Fatal(err)
	}
	before := os.Stdout
	os.Stdout = writer
	t.Cleanup(func() { os.Stdout = before; _ = writer.Close() })
	if code := dispatch([]string{"skills", "list", "--json"}); code == 0 {
		t.Fatal("a JSON write to a closed pipe reported success")
	}
}

func TestPresentationPolicyPreservesProtocolAndFlagValues(t *testing.T) {
	cases := []struct {
		args           []string
		protocol, json bool
		progress       string
		rest           []string
	}{
		{[]string{"skills", "list", "--progress=always"}, false, false, "always", []string{"skills", "list"}},
		{[]string{"gate", "status", "--json", "--progress", "never"}, false, true, "never", []string{"gate", "status", "--json"}},
		{[]string{"skills", "scaffold", "--skill", "--progress", "--check"}, false, true, "auto", []string{"skills", "scaffold", "--skill", "--progress", "--check"}},
		{[]string{"map", "--progress", "always", "search", "needle"}, true, false, "auto", []string{"map", "--progress", "always", "search", "needle"}},
		{[]string{"hook", "session-start", "--progress", "never"}, true, false, "auto", []string{"hook", "session-start", "--progress", "never"}},
		{[]string{"mcp"}, true, false, "auto", []string{"mcp"}},
		{[]string{"--progress", "never", "ast", "needle"}, true, false, "never", []string{"ast", "needle", "--progress", "never"}},
	}
	for _, tc := range cases {
		rest, p, err := presentationArgs(tc.args)
		if err != nil || p.Protocol != tc.protocol || p.JSON != tc.json || p.Progress != tc.progress || !reflect.DeepEqual(rest, tc.rest) {
			t.Fatalf("%v: rest=%v policy=%+v err=%v", tc.args, rest, p, err)
		}
	}
	for _, args := range [][]string{{"skills", "list", "--progress"}, {"skills", "list", "--progress=bogus"}} {
		if _, _, err := presentationArgs(args); err == nil {
			t.Fatalf("accepted %v", args)
		}
	}
	for _, command := range []string{"verify", "install", "uninstall", "disable", "enable", "gate", "skills", "integrate", "integrations"} {
		_, p, err := presentationArgs([]string{command})
		if err != nil || p.Protocol || p.Title == "" || p.Activity == "" {
			t.Fatalf("missing finite-command policy for %s", command)
		}
	}
}
