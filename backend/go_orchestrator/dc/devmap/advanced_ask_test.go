package devmap

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const askStatus = `{"host_contract_version":1,"schema_version":19,"expected_schema_version":19,"schema_relation":"current","reader_ready":true,"query_ready":true,"db_path":"db","generation_id":7,"is_fresh":true,"capabilities":{"ask_evidence":true}}`

const askPack = `{"files":[{"file_path":"a.go","role":"implementation","score":0.9,"units":[{"file_path":"a.go","symbol_name":"A","source_span":"func A() {}"}]}],"resolution":"Available","shown":1,"total":3,"hidden":2,"truncated":true,"tokens_used":40,"walk_incomplete":"weak match","related_tests":{"items":[],"resolution":"Available","shown":0,"total":0,"hidden":0,"truncated":false,"tokens_used":0}}`

// recordingDevmap answers status and ask, and writes every argument vector it
// was run with to a file, so a test can see the exact command line.
func recordingDevmap(t *testing.T, status, ask string) (*Client, string) {
	t.Helper()
	dir := t.TempDir()
	argsLog := filepath.Join(dir, "args")
	script := "#!/bin/sh\n" +
		"printf '%s\\n' \"$*\" >> '" + argsLog + "'\n" +
		"case \" $* \" in\n" +
		"  *\" status \"*) cat <<'EOF'\n" + status + "\nEOF\n;;\n" +
		"  *\" ask \"*) cat <<'EOF'\n" + ask + "\nEOF\n;;\n" +
		"esac\n"
	path := filepath.Join(dir, "devmap")
	if err := os.WriteFile(path, []byte(script), 0o600); err != nil {
		t.Fatal(err)
	}
	// #nosec G302 -- this test fixture must be executable and is private to its temp directory.
	if err := os.Chmod(path, 0o700); err != nil {
		t.Fatal(err)
	}
	return New(path, dir), argsLog
}

func TestAskEvidenceRunsAskWithEvidenceAndKeepsTheEnvelope(t *testing.T) {
	c, argsLog := recordingDevmap(t, askStatus, askPack)
	res, err := c.Advanced(context.Background(), AdvancedQuery{Kind: QueryAskEvidence, Query: "-where are batches run", Budget: 500})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(res.Data), `"walk_incomplete":"weak match"`) || !strings.Contains(string(res.Data), `"total":3`) {
		t.Fatalf("the producer's completeness fields were lost: %s", res.Data)
	}
	raw, err := os.ReadFile(argsLog)
	if err != nil {
		t.Fatal(err)
	}
	var askLine string
	for _, line := range strings.Split(string(raw), "\n") {
		if _, after, ok := strings.Cut(" "+line, " ask "); ok {
			askLine = "ask " + after
		}
	}
	// The question is fenced behind --, so a leading dash is a question and not
	// a flag, and no depth is passed to a command that has none.
	if askLine != "ask --evidence --budget 500 -- -where are batches run" {
		t.Fatalf("ask ran as %q", askLine)
	}
}

func TestAskEvidenceRefusesWhatItCannotBound(t *testing.T) {
	c, _ := recordingDevmap(t, askStatus, askPack)
	for _, q := range []AdvancedQuery{
		{Kind: QueryAskEvidence, Query: "x", Depth: 2, Budget: 10},
		{Kind: QueryAskEvidence, Query: "   ", Budget: 10},
		{Kind: QueryAskEvidence, Query: strings.Repeat("x", 4097), Budget: 10},
	} {
		if _, err := c.Advanced(context.Background(), q); err == nil {
			t.Errorf("accepted %+v", q)
		}
	}
}

func TestAskEvidenceNeedsTheCapabilityAdvertised(t *testing.T) {
	without := strings.Replace(askStatus, `"ask_evidence":true`, `"explore":true`, 1)
	c, _ := recordingDevmap(t, without, askPack)
	_, err := c.Advanced(context.Background(), AdvancedQuery{Kind: QueryAskEvidence, Query: "x", Budget: 10})
	if err == nil || !strings.Contains(err.Error(), `did not advertise capability "ask_evidence"`) {
		t.Fatalf("an older devmap without ask_evidence was queried: %v", err)
	}
}

func TestAskEvidenceRefusesImpossibleCounts(t *testing.T) {
	for name, pack := range map[string]string{
		"shown disagrees with the units": strings.Replace(askPack, `"shown":1`, `"shown":2`, 1),
		"no files list":                  strings.Replace(askPack, `"files":`, `"hits":`, 1),
		"truncated but nothing hidden": strings.Replace(strings.Replace(askPack, `"hidden":2`, `"hidden":0`, 1),
			`"total":3`, `"total":1`, 1),
		"related tests malformed": strings.Replace(askPack, `"related_tests":{"items":[]`, `"related_tests":{"items":{}`, 1),
	} {
		t.Run(name, func(t *testing.T) {
			c, _ := recordingDevmap(t, askStatus, pack)
			_, err := c.Advanced(context.Background(), AdvancedQuery{Kind: QueryAskEvidence, Query: "x", Budget: 10})
			if err == nil || !strings.Contains(err.Error(), "completeness") {
				t.Fatalf("an envelope whose counts cannot be true was not refused for its counts: %v", err)
			}
		})
	}
}

// TestAskEvidenceAcceptsARealPack pins the validator to devmap's own output,
// captured from devmap 0.2.5 on 2026-11-20 (`devmap --json ask --evidence
// --budget 400 "lease renewal"` in this repository). A hand-written pack only
// agrees with what this file believes; this one is what the producer says, and
// it has more units than files.
func TestAskEvidenceAcceptsARealPack(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("testdata", "ask_evidence_pack.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := validateAdvancedEnvelope(QueryAskEvidence, data); err != nil {
		t.Fatalf("a real evidence pack was refused: %v", err)
	}
}
