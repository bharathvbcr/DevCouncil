package devcouncil

import (
	"errors"
	"testing"
)

func TestAnswerToolKeepsTheServersErrorClassification(t *testing.T) {
	for _, tc := range []struct {
		what    string
		payload any
		err     error
		isError bool
		code    string
		exit    int
	}{
		{"ok map", map[string]any{"ok": true}, nil, false, "", ToolExitOK},
		{"refused map", map[string]any{"ok": false, "code": "lease_conflict"}, nil, true, "lease_conflict", ToolExitRefused},
		{"not found", ErrorPayload{OK: false, Code: "not_found"}, nil, true, "not_found", ToolExitRefused},
		{"missing argument", ErrorPayload{OK: false, Code: "missing_argument"}, nil, true, "missing_argument", ToolExitUsage},
		{"undeclared argument", ErrorPayload{OK: false, Code: "invalid_argument"}, nil, true, "invalid_argument", ToolExitUsage},
		{"no store", ErrorPayload{OK: false, Code: "not_initialized"}, nil, true, "not_initialized", ToolExitNotRun},
		{"store failed", map[string]any{"ok": false, "code": "store_error"}, nil, true, "store_error", ToolExitNotRun},
		{"verify skipped", ErrorPayload{OK: false, Code: "verify_error"}, nil, true, "verify_error", ToolExitNotRun},
		{"diff ok", DiffResult{OK: true}, nil, false, "", ToolExitOK},
		{"diff not a repo", DiffResult{OK: false, Code: "not_a_git_repo"}, nil, true, "not_a_git_repo", ToolExitRefused},
		{"git undetermined", DiffResult{OK: false, Code: "git_repo_undetermined"}, nil, true, "git_repo_undetermined", ToolExitNotRun},
		{"unknown tool", nil, errors.New("unknown tool: x"), true, "tool_error", ToolExitNotRun},
		// A type outside the server's three cases keeps isError false, as
		// the server always answered it, but a shell must not read ok:false
		// as success.
		{"unflagged refusal", struct {
			OK   bool   `json:"ok"`
			Code string `json:"code"`
		}{false, "lease_conflict"}, nil, false, "lease_conflict", ToolExitRefused},
	} {
		got := AnswerTool(tc.payload, tc.err)
		if got.IsError != tc.isError || got.Code != tc.code || got.ExitStatus() != tc.exit {
			t.Errorf("%s: got isError=%v code=%q exit=%d, want %v %q %d (text %s)",
				tc.what, got.IsError, got.Code, got.ExitStatus(), tc.isError, tc.code, tc.exit, got.Text)
		}
	}
}

func TestMarshalToolPayloadKeepsV1NullForNilSlices(t *testing.T) {
	// The wire contract: a nil slice is null, as v1 encoding/json writes it.
	// A move to json/v2 would write [] and break every golden envelope.
	if got := MarshalToolPayload(struct {
		Gaps []string `json:"gaps"`
	}{}); got != `{"gaps":null}` {
		t.Fatalf("got %s", got)
	}
}

func TestTimeoutAnswerCannotReadAsAnAnswer(t *testing.T) {
	a := TimeoutAnswer("devcouncil_verify_task", "2m0s", "ENV")
	if !a.IsError || a.ExitStatus() != ToolExitNotRun {
		t.Fatalf("timeout answer = %+v exit %d", a, a.ExitStatus())
	}
}
