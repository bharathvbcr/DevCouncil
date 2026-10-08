package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/repomap"
)

// gateRepo returns a repository root whose DevMap state directory is
// .devcouncil, holding graph (nil for none) as its code graph.
//
// Five areas, one coupling: a calls b. The widest area neighbours one of five,
// so the map is not reported permissive and an allow from the neighbour rung
// is an allow that meant something.
func gateRepo(t *testing.T, graph []byte) string {
	t.Helper()
	t.Setenv(repomap.DevmapHomeEnv, "")
	root := t.TempDir()
	graphDir := filepath.Join(root, ".devcouncil", "graph")
	if err := os.MkdirAll(graphDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if graph != nil {
		if err := os.WriteFile(filepath.Join(graphDir, "code_graph.json"), graph, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	// strict: the default posture (dev) demotes a soft scope denial to an
	// allow, which would hide the scope decision these tests are about.
	cfg := "harness:\n  posture: strict\n"
	if err := os.WriteFile(filepath.Join(root, ".devcouncil", "config.yaml"), []byte(cfg), 0o644); err != nil {
		t.Fatal(err)
	}
	return root
}

func fiveAreaGraph(t *testing.T) []byte {
	t.Helper()
	var nodes []map[string]any
	for _, f := range []string{"a/x.go", "b/y.go", "c/z.go", "d/w.go", "e/v.go"} {
		area := filepath.Dir(f)
		nodes = append(nodes,
			map[string]any{"id": f, "kind": "file", "path": f, "area": area},
			map[string]any{"id": f + "::F", "kind": "function", "path": f, "area": area})
	}
	raw, err := json.Marshal(map[string]any{
		"schema_version": 2,
		"nodes":          nodes,
		"edges": []map[string]any{{
			"source": "a/x.go::F", "target": "b/y.go::F",
			"kind": "calls", "confidence": repomap.ConfidenceExtracted,
		}},
	})
	if err != nil {
		t.Fatal(err)
	}
	return raw
}

var plannedInA = &dc.Task{ID: "TASK-1", PlannedFiles: []dc.PlannedFile{{Path: "a/x.go", AllowedChange: dc.ChangeModify}}}

func writeDecision(t *testing.T, root, path string) policy.Decision {
	t.Helper()
	g, err := openGate(root)
	if err != nil {
		t.Fatalf("openGate: %v", err)
	}
	d, err := g.EvaluateWrite(path, plannedInA, dc.OpModify)
	if err != nil {
		t.Fatalf("EvaluateWrite(%s): %v", path, err)
	}
	return d
}

// The gate used to be built with a nil map, so this write was judged by the
// same-directory fallback (b is not a's directory) and refused with
// repo_map.unavailable.
func TestOpenGateAllowsAnAdjacentSubsystemThroughTheNeighbourRung(t *testing.T) {
	d := writeDecision(t, gateRepo(t, fiveAreaGraph(t)), "b/y.go")
	if d.Blocked() {
		t.Fatalf("a write into a's neighbour b was refused: %s (%s) degraded=%v", d.Rule, d.Reason, d.Degraded)
	}
	if d.Reason != "File is in a neighboring subsystem of a planned file." {
		t.Fatalf("allowed by the wrong rung: %q", d.Reason)
	}
	if len(d.Degraded) != 0 {
		t.Fatalf("degraded = %v, want none", d.Degraded)
	}
}

func TestOpenGateDeniesANonAdjacentSubsystemWithTheMap(t *testing.T) {
	d := writeDecision(t, gateRepo(t, fiveAreaGraph(t)), "c/z.go")
	if !d.Blocked() || d.Rule != policy.RuleUnplannedScope {
		t.Fatalf("a write into c, which a does not touch, = %s %s (%s)", d.Action, d.Rule, d.Reason)
	}
	if slices.Contains(d.Degraded, "repo_map.unavailable") {
		t.Fatalf("the map was loaded but the decision says it was not: %v", d.Degraded)
	}
}

// Without a usable map the rung degrades loudly — recorded on the decision —
// and to the same-directory fallback, which is narrower than the neighbour
// relation. So a write the map would allow is refused, never the reverse.
func TestOpenGateWithoutAUsableMapDegradesAndNeverWidens(t *testing.T) {
	for name, graph := range map[string][]byte{
		"missing":       nil,
		"corrupt":       []byte(`{"nodes": [`),
		"not a graph":   []byte(`"just a string"`),
		"empty":         []byte(`{"nodes":[],"edges":[]}`),
		"not even JSON": []byte("\x00\xff"),
	} {
		root := gateRepo(t, graph)
		for _, path := range []string{"b/y.go", "c/z.go"} {
			d := writeDecision(t, root, path)
			if !d.Blocked() {
				t.Errorf("%s map: a write into %s was allowed: %s", name, path, d.Reason)
			}
			if !slices.Contains(d.Degraded, "repo_map.unavailable") {
				t.Errorf("%s map: %s refusal does not record repo_map.unavailable: %v", name, path, d.Degraded)
			}
		}
		// The planned file itself is still writable: degradation does not
		// turn into refusing everything.
		if d := writeDecision(t, root, "a/x.go"); d.Blocked() {
			t.Errorf("%s map: the planned file was refused: %s (%s)", name, d.Rule, d.Reason)
		}
	}
}

// fnmatch is the one matcher policy decisions are made with
// (docs/gusset-candidates.md): a nil Matcher is fnmatch. The gate used to be
// handed the Gusset engine on unix, which answered the same questions about
// twice as slowly and could refuse a decision under path.engine_unavailable.
func TestOpenGateDecidesWithFnmatch(t *testing.T) {
	g, err := openGate(gateRepo(t, nil))
	if err != nil {
		t.Fatalf("openGate: %v", err)
	}
	if g.Matcher != nil {
		t.Fatalf("gate matcher is %T; policy decisions must use fnmatch (nil)", g.Matcher)
	}
}
