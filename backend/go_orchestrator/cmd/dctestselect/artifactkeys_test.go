package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// artifact keys on the unusable-plan path and with a null-valued plan.
func TestArtifactKeysAreNeverNullOnAnyPath(t *testing.T) {
	want := []string{"selected", "widened", "full", "selected_phase_failed_only"}
	check := func(name string, dir string) {
		auditIn(t, dir)
		body, err := os.ReadFile(filepath.Join(dir, "selection-recall.json"))
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		var m map[string]json.RawMessage
		if err := json.Unmarshal(body, &m); err != nil {
			t.Fatal(err)
		}
		for _, k := range want {
			if _, ok := m[k]; !ok {
				t.Errorf("%s: key %s absent", name, k)
			}
		}
		for k, v := range m {
			if strings.Contains(strings.ToLower(k), "recall") || k == "missed" || k == "unclassified" {
				t.Errorf("%s: old-shape key %s=%s", name, k, v)
			}
		}
		if string(m["widened"]) == "null" || string(m["selected"]) == "null" {
			t.Errorf("%s: widened=%s selected=%s", name, m["widened"], m["selected"])
		}
		var full struct{ Failed json.RawMessage }
		json.Unmarshal(m["full"], &full)
		if string(full.Failed) == "null" {
			t.Errorf("%s: full.failed null", name)
		}
	}
	d1 := t.TempDir() // no plan at all
	check("no plan", d1)
	d2 := phaseDir(t, selectedCa, plantedMiss, goPass, finished(101))
	body, _ := os.ReadFile(filepath.Join(d2, "plan.json"))
	var m map[string]any
	json.Unmarshal(body, &m)
	m["widened"] = nil
	nb, _ := json.Marshal(m)
	os.WriteFile(filepath.Join(d2, "plan.json"), nb, 0o644)
	check("plan with widened:null", d2)
}
