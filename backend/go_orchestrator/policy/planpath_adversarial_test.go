package policy

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
)

// Pinned: NormalizePlanPath collapses separators and "/./" but never resolves
// "..", never touches lookalike slashes, and never turns a non-empty entry
// into a different non-empty one beyond that.
func TestPlanPathNormalisationAdversarial(t *testing.T) {
	for in, want := range map[string]string{
		".//":         "",
		"./.":         ".",
		"//":          "/",
		"a/./b":       "a/b",
		"a/././b":     "a/b",
		"a/../b":      "a/../b",
		"src/../.env": "src/../.env",
		"a/b/":        "a/b/",
		"a\\b\\":      "a/b/",
		"a/.":         "a/.",
		"a\u2215b":    "a\u2215b",
		"a\uff0fb":    "a\uff0fb",
		".//\\.//x":   "x",
		"a/.//./b":    "a/b",
	} {
		if got := dc.NormalizePlanPath(in); got != want {
			t.Errorf("NormalizePlanPath(%q) = %q, want %q", in, got, want)
		}
	}
}

// Pinned: a forbidden entry spelled with extra separators or dot segments still
// forbids, in every spelling of the path asked about.
func TestForbiddenEntrySpellingsStillForbid(t *testing.T) {
	root := t.TempDir()
	for _, forbidden := range []string{".//lib/x.go", "lib//x.go", "./lib/./x.go", "lib\\x.go"} {
		task := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{modify("lib/*")}, ForbiddenChanges: []string{forbidden}}
		g := FileGate{Root: root, HardRules: true}
		for _, p := range []string{"lib/x.go", "./lib//x.go", "lib/../lib/x.go"} {
			if d := g.EvaluateFileChange(p, task, dc.OpModify, false); !d.Blocked() || d.Rule != RuleForbiddenChange {
				t.Errorf("forbidden %q, path %q: %s/%q", forbidden, p, d.Action, d.Rule)
			}
		}
	}
}

// A forbidden or planned entry that names a path through ".." is not resolved
// (documented). A planned entry reaches nothing it did not name, which is the
// safe direction; a forbidden entry written that way forbids nothing.
func TestDotDotInPlanEntries(t *testing.T) {
	root := t.TempDir()
	g := FileGate{Root: root, HardRules: true}
	widen := &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{modify("src/../lib/x.go")}}
	if d := g.EvaluateFileChange("lib/x.go", widen, dc.OpModify, false); !d.Blocked() {
		t.Errorf("planned src/../lib/x.go widened to lib/x.go: %s", d.Action)
	}
	if d := g.EvaluateFileChange("src/../.env", &dc.Task{ID: "T", PlannedFiles: []dc.PlannedFile{modify("src/../.env")}}, dc.OpModify, false); !d.Blocked() {
		t.Errorf("planned src/../.env allowed: %s", d.Action)
	}
}
