package devcouncil_test

import (
	"context"
	"encoding/json"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// leaseRegistry is a real store with one task in the given status.
func leaseRegistry(t *testing.T, status string) *devcouncil.Registry {
	t.Helper()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	t.Cleanup(client.Close)
	if _, err := client.ActiveLeases(context.Background()); err != nil { // creates the store
		t.Fatal(err)
	}
	seed := exec.Command(testsupport.Tool(t, "sqlite3"), client.DB,
		"INSERT INTO tasks (id,title,description,status) VALUES ('T1','t','d','"+status+"');")
	if out, err := seed.CombinedOutput(); err != nil {
		t.Fatalf("seed: %v %s", err, out)
	}
	return devcouncil.NewRegistry(t.TempDir(), client, nil)
}

func call(t *testing.T, r *devcouncil.Registry, tool string, args map[string]any) map[string]any {
	t.Helper()
	out, err := r.Call(context.Background(), tool, args)
	if err != nil {
		t.Fatalf("%s: %v", tool, err)
	}
	raw, _ := json.Marshal(out)
	var m map[string]any
	if err := json.Unmarshal(raw, &m); err != nil {
		t.Fatalf("%s: %v", tool, err)
	}
	return m
}

func TestCheckoutOfAReadyTaskReturnsTheServedToolList(t *testing.T) {
	r := leaseRegistry(t, "ready")
	got := call(t, r, "devcouncil_checkout_task", map[string]any{"task_id": "T1", "client_id": "c1"})
	if got["ok"] != true {
		t.Fatalf("checkout: %v", got)
	}
	tools, _ := got["allowed_next_tools"].([]any)
	if len(tools) != len(devcouncil.ServedToolNames()) {
		t.Fatalf("allowed_next_tools = %v, want the served set", tools)
	}
}

func TestRecheckoutByTheHoldingClientReturnsTheExistingLease(t *testing.T) {
	r := leaseRegistry(t, "ready")
	first := call(t, r, "devcouncil_checkout_task", map[string]any{"task_id": "T1", "client_id": "c1"})
	again := call(t, r, "devcouncil_checkout_task", map[string]any{"task_id": "T1", "client_id": "c1"})
	if again["ok"] != true || again["lease_token"] != first["lease_token"] {
		t.Fatalf("the holder was locked out of its own lease: %v", again)
	}
	other := call(t, r, "devcouncil_checkout_task", map[string]any{"task_id": "T1", "client_id": "c2"})
	if other["ok"] != false || other["code"] != "lease_conflict" {
		t.Fatalf("a different client must still conflict: %v", other)
	}
}

func TestForceCheckoutNamesTheRevokedOwner(t *testing.T) {
	r := leaseRegistry(t, "ready")
	call(t, r, "devcouncil_checkout_task", map[string]any{"task_id": "T1", "client_id": "c1"})
	forced := call(t, r, "devcouncil_checkout_task", map[string]any{"task_id": "T1", "client_id": "c2", "force": true})
	if forced["ok"] != true || forced["revoked_lease_owner"] != "c1" {
		t.Fatalf("forced checkout did not name the lease it revoked: %v", forced)
	}
	var found bool
	for _, tool := range r.Specs() {
		if tool.Name == "devcouncil_checkout_task" {
			found = json.Valid(tool.InputSchema) && strings.Contains(string(tool.InputSchema), `"description"`)
		}
	}
	if !found {
		t.Fatal("force has no description in the checkout schema")
	}
}

func TestReleaseWithAWrongTokenIsNotOK(t *testing.T) {
	r := leaseRegistry(t, "ready")
	call(t, r, "devcouncil_checkout_task", map[string]any{"task_id": "T1", "client_id": "c1"})
	got := call(t, r, "devcouncil_release_task", map[string]any{"task_id": "T1", "lease_token": "not-the-token"})
	if got["ok"] != false || got["released"] != false {
		t.Fatalf("a release that released nothing reported success: %v", got)
	}
}

func TestGetGapsOnAnUnknownTaskIsNotFound(t *testing.T) {
	r := leaseRegistry(t, "ready")
	got := call(t, r, "devcouncil_get_gaps", map[string]any{"task_id": "NOPE"})
	if got["ok"] != false || got["code"] != "not_found" {
		t.Fatalf("an unknown task read as having no gaps: %v", got)
	}
	if known := call(t, r, "devcouncil_get_gaps", map[string]any{"task_id": "T1"}); known["ok"] != true {
		t.Fatalf("a known task with no gaps must still be ok: %v", known)
	}
}
