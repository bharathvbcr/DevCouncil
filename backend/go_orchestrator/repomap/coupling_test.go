package repomap

import "testing"

// TestInheritanceIsCoupling.
//
// `couplingKinds` was {calls, references, imports}. The kernel's graph also
// carries `inherits` (EdgeKind::Extends), `implements` and `routes_to`
// (EdgeKind::HandlesRoute) — code_graph.rs:154-157 — and a cross-area link
// whose only edge was one of those was silently "not adjacent".
//
// The empirical case, reproduced against the real binary on a two-directory
// Ruby corpus with `core/base.rb` declaring BaseRecord and `web/user.rb`
// declaring `class UserRecord < BaseRecord`:
//
//	Counter({('contains','extracted'): 6, ('inherits','extracted'): 1})
//	inherits extracted | web/user.rb::UserRecord -> core/base.rb::BaseRecord
//
// One class directly inherits from the other and the graph has exactly one
// edge saying so. Ruby classes are autoloaded rather than required in the
// common case, so no `imports` edge exists to carry the same fact — and 24 of
// the 35 languages the extractor handles emit no import edges at all,
// including Java, C#, Swift, Kotlin and Scala, where inheritance is the
// primary way one area binds to another.
//
// This is the write gate's question, so the cost is not abstract: the gate asks
// "is this file near the plan", and under the narrow set it would refuse an
// edit spanning a subclass and the base class it derives from.
func TestInheritanceIsCoupling(t *testing.T) {
	node := func(id, kind, p, area string) map[string]any {
		return map[string]any{"id": id, "kind": kind, "path": p, "area": area}
	}
	edge := func(src, tgt, kind string) map[string]any {
		return map[string]any{"source": src, "target": tgt, "kind": kind,
			"confidence": ConfidenceExtracted}
	}

	for _, kind := range []string{"inherits", "implements", "routes_to"} {
		t.Run(kind, func(t *testing.T) {
			m := load(t, map[string]any{
				"nodes": []map[string]any{
					node("web/user.rb", "file", "web/user.rb", "web"),
					node("core/base.rb", "file", "core/base.rb", "core"),
				},
				"edges": []map[string]any{edge("web/user.rb", "core/base.rb", kind)},
			})
			if m == nil {
				t.Fatal("the document was rejected")
			}
			if !m.AreNeighbors("web", "core") {
				t.Errorf("an area whose only edge to another is %q is reported as not adjacent; "+
					"the gate would refuse an edit spanning the two", kind)
			}
			// Symmetric, like every other coupling: the gate's question has no
			// direction, and a base class is as coupled to its subclass as the
			// subclass is to it.
			if !m.AreNeighbors("core", "web") {
				t.Errorf("%q adjacency is not symmetric", kind)
			}
		})
	}
}

// TestStructuralRelationsAreStillNotCoupling pins the other half of the
// widening. `contains` and `member_of` are relations inside one file and never
// cross an area; admitting them would make the neighbour rule vacuous rather
// than more honest, which is the failure mode the original narrow set existed
// to avoid.
func TestStructuralRelationsAreStillNotCoupling(t *testing.T) {
	node := func(id, kind, p, area string) map[string]any {
		return map[string]any{"id": id, "kind": kind, "path": p, "area": area}
	}
	for _, kind := range []string{"contains", "member_of", "defines"} {
		t.Run(kind, func(t *testing.T) {
			m := load(t, map[string]any{
				"nodes": []map[string]any{
					node("a/x.go", "file", "a/x.go", "a"),
					node("b/y.go", "file", "b/y.go", "b"),
				},
				"edges": []map[string]any{{"source": "a/x.go", "target": "b/y.go",
					"kind": kind, "confidence": ConfidenceExtracted}},
			})
			if m == nil {
				t.Fatal("the document was rejected")
			}
			if m.AreNeighbors("a", "b") {
				t.Errorf("%q is a structural relation, not a dependency, and must not create adjacency", kind)
			}
		})
	}
}

// TestWidenedCouplingStillRequiresExtractedConfidence. The widening adds edge
// kinds, not credulity: an inherits edge the analyser could not resolve is
// still a guess, and a scope decision resting on a guess is what the
// confidence floor exists to prevent.
func TestWidenedCouplingStillRequiresExtractedConfidence(t *testing.T) {
	m := load(t, map[string]any{
		"nodes": []map[string]any{
			{"id": "a/x.go", "kind": "file", "path": "a/x.go", "area": "a"},
			{"id": "b/y.go", "kind": "file", "path": "b/y.go", "area": "b"},
		},
		"edges": []map[string]any{{"source": "a/x.go", "target": "b/y.go",
			"kind": "inherits", "confidence": "ambiguous"}},
	})
	if m == nil {
		t.Fatal("the document was rejected")
	}
	if m.AreNeighbors("a", "b") {
		t.Error("an ambiguous inherits edge must not create adjacency")
	}
	if m.Stats().AmbiguousSkipped != 1 {
		t.Errorf("AmbiguousSkipped = %d, want 1; the rejection must be counted, not silent",
			m.Stats().AmbiguousSkipped)
	}
}
