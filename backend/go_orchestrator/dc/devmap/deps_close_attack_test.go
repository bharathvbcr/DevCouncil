package devmap

import (
	"context"
	"strings"
	"testing"
)

// Closing probe 2 against lane/deps-outbound @ 41a0d9be. Failing tests are
// findings; nothing is fixed here.

// A note beside a walk_incomplete answer: the answer stays degraded, the note
// still arrives, and the degradation text is the producer's own.
func TestCloseAttackNoteAndPartialTogether(t *testing.T) {
	c := fake(t, map[string]string{
		"status": healthyStatus,
		"deps":   `{"hidden":0,"items":[],"note":"x has 3 inbound edge(s)","walk_incomplete":"capped"}`,
	})
	r, err := c.Deps(context.Background(), "x.py")
	if err != nil {
		t.Fatal(err)
	}
	if r.Clean() {
		t.Fatalf("a partial answer with a note became clean: %+v", r)
	}
	if !strings.Contains(r.Note, "3 inbound") {
		t.Fatalf("note lost beside walk_incomplete: %+v", r)
	}
	if len(r.Degraded) != 1 || !strings.Contains(r.Degraded[0], "capped") {
		t.Fatalf("degradation wrong: %+v", r.Degraded)
	}
}

// An empty-string note and an empty-string walk_incomplete are absent, not
// degraded; a JSON null is the same.
func TestCloseAttackEmptyAndNullAnnotationsAreAbsent(t *testing.T) {
	for _, body := range []string{
		`{"hidden":0,"items":[],"note":"","walk_incomplete":""}`,
		`{"hidden":0,"items":[],"note":null,"walk_incomplete":null}`,
	} {
		c := fake(t, map[string]string{"status": healthyStatus, "deps": body})
		r, err := c.Deps(context.Background(), "x.py")
		if err != nil {
			t.Fatalf("%s: %v", body, err)
		}
		if r.Note != "" || !r.Clean() {
			t.Fatalf("%s decoded to %+v", body, r)
		}
	}
}

// Fields the client does not know decode without error and do not disturb the
// ones it does.
func TestCloseAttackUnknownFieldsStillDecode(t *testing.T) {
	c := fake(t, map[string]string{
		"status": healthyStatus,
		"deps": `{"hidden":0,"items":[{"source_file":"a.py","source_symbol":"f","target_file":"b.py","target_symbol":"g","edge_kind":"Calls","confidence":1,"future":{"x":1}}],` +
			`"note":"n","rungs":{"deterministic":1},"resolution":{"state":"Available"},"extra":[1,2]}`,
	})
	r, err := c.Deps(context.Background(), "a.py")
	if err != nil {
		t.Fatal(err)
	}
	if len(r.Items) != 1 || r.Note != "n" {
		t.Fatalf("%+v", r)
	}
}

// A non-string note is a protocol violation the decoder must not pass off as
// an empty, clean answer.
func TestCloseAttackNonStringNoteIsNotSilentlyClean(t *testing.T) {
	c := fake(t, map[string]string{
		"status": healthyStatus,
		"deps":   `{"hidden":0,"items":[],"note":{"a":1}}`,
	})
	r, err := c.Deps(context.Background(), "x.py")
	if err == nil && r.Clean() && r.Note == "" {
		t.Fatalf("a malformed note decoded as a clean, note-less answer: %+v", r)
	}
}
