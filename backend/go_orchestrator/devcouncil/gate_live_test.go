package devcouncil_test

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

func call(t *testing.T, reg *devcouncil.Registry, tool string, args map[string]any) map[string]any {
	t.Helper()
	out, err := reg.Call(context.Background(), tool, args)
	if err != nil {
		t.Fatalf("%s: %v", tool, err)
	}
	raw, err := json.Marshal(out)
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal(raw, &m); err != nil {
		t.Fatal(err)
	}
	return m
}

// The gate mode is read on every call, not once when the host starts: a
// `gate set` reaches a host that is already running. And a config that is
// present but cannot be read does not mean "off" — it enforces, says so in the
// checkout, next_task and verify payloads, and blocks the verification.
func TestALiveHostFollowsTheGateConfigAndFailsClosedWhenItCannotRead(t *testing.T) {
	root := t.TempDir()
	cfg := filepath.Join(root, ".devcouncil", "config.yaml")
	if err := os.MkdirAll(filepath.Dir(cfg), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(cfg, []byte("gates:\n  mode: off\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	sql := "INSERT INTO tasks (id,title,description,planned_files_json,expected_tests_json,status) " +
		`VALUES ('TASK-G','gate','','[]','[]','ready');`
	if out, err := exec.Command(testsupport.Tool(t, "sqlite3"), client.DB, sql).CombinedOutput(); err != nil {
		t.Fatalf("task fixture: %v %s", err, out)
	}
	reg := devcouncil.NewRegistry(root, client, nil)

	if got := reg.GateState(); got.Mode != "off" {
		t.Fatalf("initial mode = %+v, want off", got)
	}
	if err := gatescfg.SetVerificationMode(cfg, "enforce"); err != nil {
		t.Fatal(err)
	}
	if got := reg.GateState(); got.Mode != "enforce" || got.Origin != "config" {
		t.Fatalf("after gate set, a running host reports %+v, want enforce/config", got)
	}
	co := call(t, reg, "devcouncil_checkout_task", map[string]any{"task_id": "TASK-G", "client_id": "c1"})
	if co["gate_mode"] != "enforce" {
		t.Fatalf("checkout gate_mode = %v, want enforce", co["gate_mode"])
	}
	if _, present := co["gate_mode_origin"]; present {
		t.Errorf("a healthy config added gate_mode_origin to checkout: %v", co)
	}

	if os.Geteuid() == 0 {
		t.Skip("root reads chmod-000 files")
	}
	if err := os.Chmod(cfg, 0); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(cfg, 0o644) })

	if got := reg.GateState(); got.Mode != "enforce" || got.Origin != "invalid" || got.Error == "" {
		t.Fatalf("unreadable config resolved to %+v, want enforce/invalid with an error", got)
	}
	next := call(t, reg, "devcouncil_next_task", map[string]any{})
	if next["gate_mode_origin"] != "invalid" || next["gate_mode_error"] == "" {
		t.Errorf("next_task payload does not name the invalid config: %v", next)
	}
	call(t, reg, "devcouncil_release_task", map[string]any{"task_id": "TASK-G", "lease_token": co["lease_token"]})
	co2 := call(t, reg, "devcouncil_checkout_task", map[string]any{"task_id": "TASK-G", "client_id": "c1", "force": true})
	if co2["gate_mode"] != "enforce" || co2["gate_mode_origin"] != "invalid" || co2["gate_mode_error"] == "" {
		t.Fatalf("checkout payload does not name the invalid config: %v", co2)
	}
	ver := call(t, reg, "devcouncil_verify_task", map[string]any{"task_id": "TASK-G", "lease_token": co2["lease_token"]})
	if ver["gate_mode_origin"] != "invalid" || ver["gate_mode_error"] == "" || ver["passed"] != false {
		t.Fatalf("verify payload = %v", ver)
	}
	blocking, _ := ver["blocking_gaps"].([]any)
	found := false
	for _, g := range blocking {
		if m, ok := g.(map[string]any); ok && m["gap_type"] == "gate_config_invalid" {
			found = true
		}
	}
	if !found {
		t.Errorf("no blocking gate_config_invalid gap: %v", ver["blocking_gaps"])
	}
}
