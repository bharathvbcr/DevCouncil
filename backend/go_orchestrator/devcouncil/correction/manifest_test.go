package correction_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/correction"
)

func TestWriteRequiresRootAndTask(t *testing.T) {
	if _, _, err := correction.Write(correction.WriteOptions{TaskID: "T1"}); err == nil {
		t.Fatal("empty root must fail")
	}
	if _, _, err := correction.Write(correction.WriteOptions{Root: t.TempDir()}); err == nil {
		t.Fatal("empty task_id must fail")
	}
}

func TestWritePersistsOpenManifest(t *testing.T) {
	root := t.TempDir()
	m, path, err := correction.Write(correction.WriteOptions{
		Root:   root,
		TaskID: "TASK-1",
		Gaps: []correction.ManifestGap{{
			ID: "G1", GapType: "orphan_diff", Severity: "high",
			Blocking: true, Action: "revert extra.go", File: "extra.go",
		}},
		NextActions: []string{"revert extra.go"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if m.Status != "open" || m.TaskID != "TASK-1" || m.RetryBudget != 3 {
		t.Fatalf("manifest=%+v", m)
	}
	want := filepath.Join(root, ".devcouncil", "corrections", m.ID+".json")
	if path != want {
		t.Fatalf("path=%q want %q", path, want)
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var got correction.Manifest
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatal(err)
	}
	if got.ID != m.ID || len(got.Gaps) != 1 || got.Gaps[0].File != "extra.go" {
		t.Fatalf("disk=%+v", got)
	}
	if got.NextActions[0] != "revert extra.go" {
		t.Fatalf("next_actions=%v", got.NextActions)
	}
}

func TestWriteNilSlicesBecomeEmpty(t *testing.T) {
	root := t.TempDir()
	_, path, err := correction.Write(correction.WriteOptions{Root: root, TaskID: "T2"})
	if err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var got map[string]any
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatal(err)
	}
	if _, ok := got["gaps"].([]any); !ok {
		t.Fatalf("gaps must be an array, got %#v", got["gaps"])
	}
	if _, ok := got["next_actions"].([]any); !ok {
		t.Fatalf("next_actions must be an array, got %#v", got["next_actions"])
	}
}
