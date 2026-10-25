package dc

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"strings"
	"testing"
)

// validReviewRecord is a record DecodeReviewRecord accepts, for tests to
// mutate one field at a time.
func validReviewRecord() map[string]any {
	return map[string]any{
		"schema":                  1,
		"task_id":                 "TASK-1",
		"acceptance_criterion_id": "AC-1",
		"subject_hash":            "git-tree:4b825dc642cb6eb9a060e54bf8d69288fbee4904",
		"harness":                 "manvi 0.4.0",
		"criterion_sha256":        strings.Repeat("d", 64),
		"rubric": map[string]any{
			"path":   "docs/standards/feature-spec.md",
			"sha256": strings.Repeat("a", 64),
		},
		"isolation": map[string]any{"probe": "passed", "detail": "child saw no CLAUDE.md"},
		"reviewers": []any{
			map[string]any{"id": "critic-a", "persona": "design_critic", "model": "m", "status": "completed"},
			map[string]any{"id": "critic-b", "persona": "design_critic", "model": "m", "status": "completed"},
		},
		"findings": []any{
			map[string]any{"id": "F1", "severity": "major", "summary": "unstated retry bound", "resolution": "fixed"},
		},
		// A valid-shape signature (64 zero bytes) under a key id. Validate
		// checks only the shape; whether it verifies is VerifySignature's
		// question, tested separately.
		"key_id":    "manvi-prod",
		"signature": base64.StdEncoding.EncodeToString(make([]byte, ed25519.SignatureSize)),
	}
}

func encode(t *testing.T, v any) []byte {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestAValidReviewRecordDecodes(t *testing.T) {
	r, err := DecodeReviewRecord(encode(t, validReviewRecord()))
	if err != nil {
		t.Fatal(err)
	}
	if r.CompletedReviewers() != 2 || r.Findings[0].Resolution != ResolutionFixed || r.Isolation.Probe != IsolationPassed {
		t.Errorf("decoded %+v", r)
	}
}

// Every shape the reader would otherwise have to guess at is refused, and the
// refusal names the field.
func TestAMalformedReviewRecordIsRefusedNamingTheField(t *testing.T) {
	cases := []struct {
		name   string
		mutate func(m map[string]any)
		want   string
	}{
		{"schema", func(m map[string]any) { m["schema"] = 2 }, "schema 2"},
		{"unknown field", func(m map[string]any) { m["approved"] = true }, "approved"},
		{"blank task", func(m map[string]any) { m["task_id"] = " " }, "task_id"},
		{"newline in criterion", func(m map[string]any) { m["acceptance_criterion_id"] = "AC\n1" }, "control character"},
		{"subject alphabet", func(m map[string]any) { m["subject_hash"] = "GIT-TREE:AB" }, "subject_hash"},
		{"rubric hash", func(m map[string]any) { m["rubric"].(map[string]any)["sha256"] = "abc" }, "rubric.sha256"},
		{"rubric hash case", func(m map[string]any) { m["rubric"].(map[string]any)["sha256"] = strings.Repeat("A", 64) }, "rubric.sha256"},
		{"no rubric path", func(m map[string]any) { m["rubric"].(map[string]any)["path"] = "" }, "rubric.path"},
		{"probe", func(m map[string]any) { m["isolation"].(map[string]any)["probe"] = "ok" }, "isolation.probe"},
		{"no reviewers", func(m map[string]any) { m["reviewers"] = []any{} }, "0 reviewers"},
		{"duplicate reviewer", func(m map[string]any) {
			m["reviewers"] = []any{
				map[string]any{"id": "a", "persona": "p", "status": "completed"},
				map[string]any{"id": "a", "persona": "p", "status": "completed"},
			}
		}, "twice"},
		{"reviewer status", func(m map[string]any) {
			m["reviewers"].([]any)[0].(map[string]any)["status"] = "done"
		}, "status"},
		{"severity", func(m map[string]any) {
			m["findings"].([]any)[0].(map[string]any)["severity"] = "critical"
		}, "severity"},
		{"resolution", func(m map[string]any) {
			m["findings"].([]any)[0].(map[string]any)["resolution"] = "wontfix"
		}, "resolution"},
		{"contested without note", func(m map[string]any) {
			m["findings"].([]any)[0].(map[string]any)["resolution"] = "contested"
		}, "no note"},
		{"no summary", func(m map[string]any) {
			m["findings"].([]any)[0].(map[string]any)["summary"] = ""
		}, "no summary"},
		{"duplicate finding", func(m map[string]any) {
			f := m["findings"].([]any)[0]
			m["findings"] = []any{f, f}
		}, "finding \"F1\" twice"},
		{"oversized text", func(m map[string]any) {
			m["findings"].([]any)[0].(map[string]any)["summary"] = strings.Repeat("x", 5000)
		}, "over 4096"},
		{"blank key_id", func(m map[string]any) { m["key_id"] = " " }, "key_id"},
		{"no signature", func(m map[string]any) { m["signature"] = "" }, "signature is 0 bytes"},
		{"signature not base64", func(m map[string]any) { m["signature"] = "!!!!" }, "not base64"},
		{"signature wrong length", func(m map[string]any) {
			m["signature"] = base64.StdEncoding.EncodeToString(make([]byte, 10))
		}, "not 64"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			m := validReviewRecord()
			c.mutate(m)
			_, err := DecodeReviewRecord(encode(t, m))
			if err == nil || !strings.Contains(err.Error(), c.want) {
				t.Errorf("err = %v, want one containing %q", err, c.want)
			}
		})
	}
}

func TestTrailingDataAfterARecordIsRefused(t *testing.T) {
	data := append(encode(t, validReviewRecord()), []byte(` {"schema":1}`)...)
	if _, err := DecodeReviewRecord(data); err == nil || !strings.Contains(err.Error(), "trailing") {
		t.Errorf("err = %v", err)
	}
}

func TestAContestedFindingWithANoteDecodes(t *testing.T) {
	m := validReviewRecord()
	f := m["findings"].([]any)[0].(map[string]any)
	f["severity"], f["resolution"], f["note"] = "blocker", "contested", "the bound is in retry.go:40"
	if _, err := DecodeReviewRecord(encode(t, m)); err != nil {
		t.Fatal(err)
	}
}
