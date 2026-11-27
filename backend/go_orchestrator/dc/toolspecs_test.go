package dc

import (
	"encoding/json"
	"slices"
	"testing"
)

func TestSharedToolSpecsAndNames(t *testing.T) {
	wantNames := []string{
		"devcouncil_get_diff",
		"devcouncil_checkout_task",
		"devcouncil_renew_lease",
		"devcouncil_release_task",
		"devcouncil_next_task",
		"devcouncil_verify_task",
		"devcouncil_get_gaps",
		"devcouncil_policy_check_write",
	}

	names := SharedToolNames()
	if !slices.Equal(names, wantNames) {
		t.Fatalf("SharedToolNames() = %v, want %v", names, wantNames)
	}

	specs := SharedToolSpecs()
	if len(specs) != len(wantNames) {
		t.Fatalf("len(SharedToolSpecs()) = %d, want %d", len(specs), len(wantNames))
	}

	for i, spec := range specs {
		if spec.Name != wantNames[i] {
			t.Errorf("specs[%d].Name = %q, want %q", i, spec.Name, wantNames[i])
		}
		if spec.Description == "" {
			t.Errorf("%s has empty description", spec.Name)
		}
		var schema map[string]any
		if err := json.Unmarshal(spec.InputSchema, &schema); err != nil {
			t.Fatalf("%s has invalid InputSchema JSON: %v", spec.Name, err)
		}
		if schema["additionalProperties"] != false {
			t.Errorf("%s InputSchema additionalProperties = %v, want false", spec.Name, schema["additionalProperties"])
		}
		props, _ := schema["properties"].(map[string]any)
		for _, selector := range []string{"repo_path", "root"} {
			prop, ok := props[selector].(map[string]any)
			if !ok || prop["type"] != "string" {
				t.Errorf("%s InputSchema missing optional string property %q: %v", spec.Name, selector, props[selector])
			}
		}
		if reqs, ok := schema["required"].([]any); ok {
			for _, r := range reqs {
				if r == "repo_path" || r == "root" {
					t.Errorf("%s InputSchema marks %q as required; must remain optional", spec.Name, r)
				}
			}
		}
		ann := spec.Behaviour.Annotations()
		if ann["readOnlyHint"] != spec.Behaviour.ReadOnly ||
			ann["destructiveHint"] != spec.Behaviour.Destructive ||
			ann["idempotentHint"] != spec.Behaviour.Idempotent ||
			ann["openWorldHint"] != spec.Behaviour.OpenWorld {
			t.Errorf("%s Annotations() = %v, does not match Behaviour %+v", spec.Name, ann, spec.Behaviour)
		}

		byName, ok := SharedToolSpecByName(spec.Name)
		if !ok {
			t.Fatalf("SharedToolSpecByName(%q) returned false", spec.Name)
		}
		if byName.Name != spec.Name || string(byName.InputSchema) != string(spec.InputSchema) {
			t.Errorf("SharedToolSpecByName(%q) mismatch: got %+v, want %+v", spec.Name, byName, spec)
		}
	}

	if _, ok := SharedToolSpecByName("devcouncil_unknown"); ok {
		t.Fatal("SharedToolSpecByName(\"devcouncil_unknown\") unexpectedly returned true")
	}

	// Mutating returned slice/schema must not affect future lookups.
	specs[0].Name = "mutated"
	specs[0].InputSchema[0] = 'X'
	fresh, ok := SharedToolSpecByName("devcouncil_get_diff")
	if !ok || fresh.Name != "devcouncil_get_diff" || fresh.InputSchema[0] != '{' {
		t.Fatalf("SharedToolSpecs mutation leaked into canonical table: %+v", fresh)
	}
}
