package devcouncil_test

import (
	"context"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// enforcingGate is a write gate whose soft rules block, so a scope answer is
// visible in `allowed`. Under the shipped dev posture task.absent is demoted to
// an allow, and a test reading only `allowed` could not tell a decision taken
// against the task from one taken without it.
func enforcingGate(t *testing.T, root string) *gate.Gate {
	t.Helper()
	reg, err := flags.NewHarnessRegistry("")
	if err != nil {
		t.Fatal(err)
	}
	for key, value := range map[string]string{
		flags.PolicyFileMode:  flags.ModeEnforce,
		flags.PolicyHardRules: "true",
	} {
		if err := reg.Set(flags.Human, key, value); err != nil {
			t.Fatal(err)
		}
	}
	g, err := gate.New(reg, root, nil)
	if err != nil {
		t.Fatal(err)
	}
	return g
}

// scopedStore is a real task store holding TASK-1, which plans src/a.go for
// modification and — the security pin — lists .env as a planned file too.
// Planted through sqlite3, as the store's own seeders do: planning is not this
// package's job. Local rather than in testsupport because dc/store's internal
// tests import testsupport, so testsupport cannot import dc/store.
func scopedStore(t *testing.T) *store.Client {
	t.Helper()
	client := store.New(testsupport.DCStore(t), filepath.Join(t.TempDir(), "state.sqlite"))
	if _, err := client.ActiveLeases(context.Background()); err != nil {
		t.Fatal(err)
	}
	planned := `[{"path":"src/a.go","allowed_change":"modify"},{"path":".env","allowed_change":"modify"}]`
	sql := "INSERT INTO tasks (id,title,description,planned_files_json,expected_tests_json,status) " +
		"VALUES ('TASK-1','scope','','" + planned + "','[]','ready');"
	if out, err := exec.Command(testsupport.Tool(t, "sqlite3"), client.DB, sql).CombinedOutput(); err != nil {
		t.Fatalf("task fixture: %v %s", err, out)
	}
	return client
}

func policyCheck(t *testing.T, reg *devcouncil.Registry, args map[string]any) any {
	t.Helper()
	out, err := reg.Call(context.Background(), "devcouncil_policy_check_write", args)
	if err != nil {
		t.Fatalf("%v: %v", args, err)
	}
	return out
}

func verdictOf(t *testing.T, args map[string]any, out any) (bool, policy.Decision) {
	t.Helper()
	m, ok := out.(map[string]any)
	if !ok || m["ok"] != true {
		t.Fatalf("%v: got %T %+v, want an ok verdict", args, out, out)
	}
	d, ok := m["decision"].(policy.Decision)
	if !ok {
		t.Fatalf("%v: decision is %T, want policy.Decision", args, m["decision"])
	}
	allowed, _ := m["allowed"].(bool)
	return allowed, d
}

// The tool advertised task_id and operation and read neither: it judged every
// path with no task and as a modify, so the ladder stopped at task.absent and
// the scope question the skills send agents here to ask was never asked.
func TestPolicyCheckJudgesTheNamedTasksScope(t *testing.T) {
	root := t.TempDir()
	reg := devcouncil.NewRegistry(root, scopedStore(t), enforcingGate(t, root))

	cases := []struct {
		args    map[string]any
		allowed bool
		rule    policy.RuleID
	}{
		// In the plan: allowed, and allowed *because of* TASK-1.
		{map[string]any{"path": "src/a.go", "task_id": "TASK-1"}, true, policy.RuleNone},
		{map[string]any{"path": "src/a.go", "task_id": "TASK-1", "operation": "modify"}, true, policy.RuleNone},
		// The task was consulted, not just present: what it does not plan is
		// refused on a scope rung rather than on task.absent.
		{map[string]any{"path": "docs/x.md", "task_id": "TASK-1"}, false, policy.RuleUnplannedScope},
		// operation reaches the ladder: src/a.go is planned for modify only.
		{map[string]any{"path": "src/a.go", "task_id": "TASK-1", "operation": "delete"}, false, policy.RuleOperation},
		{map[string]any{"path": "src/a.go", "task_id": "TASK-1", "operation": "create"}, false, policy.RuleOperation},
		// No task named: unchanged, the ladder still stops at task.absent.
		{map[string]any{"path": "src/a.go"}, false, policy.RuleNoTask},
		// The security pin. TASK-1 lists .env as a planned file, and the
		// secret rung runs before the task is looked at, so naming the task
		// cannot make a credential path writable.
		{map[string]any{"path": ".env", "task_id": "TASK-1"}, false, policy.RuleSecretPath},
	}
	for _, tc := range cases {
		allowed, d := verdictOf(t, tc.args, policyCheck(t, reg, tc.args))
		if allowed != tc.allowed || d.Rule != tc.rule {
			t.Errorf("%v: allowed=%v rule=%s, want allowed=%v rule=%s (decision %+v)",
				tc.args, allowed, d.Rule, tc.allowed, tc.rule, d)
		}
		if _, named := tc.args["task_id"]; named && d.Rule != policy.RuleSecretPath && d.TaskID != "TASK-1" {
			t.Errorf("%v: decision task_id = %q, want TASK-1", tc.args, d.TaskID)
		}
	}
}

// A task_id that cannot be loaded is its own answer. Falling through to a nil
// task would report task.absent — a statement about scope — for a store that
// was missing, broken, or did not know the id.
func TestPolicyCheckFailsClosedWhenTheTaskCannotBeLoaded(t *testing.T) {
	root := t.TempDir()
	g := enforcingGate(t, root)
	args := map[string]any{"path": "src/a.go", "task_id": "TASK-1"}

	t.Run("no store", func(t *testing.T) {
		out := policyCheck(t, devcouncil.NewRegistry(root, nil, g), args)
		payload, ok := out.(devcouncil.ErrorPayload)
		if !ok || payload.OK || payload.Code != "not_initialized" {
			t.Fatalf("got %T %+v, want ok=false code=not_initialized", out, out)
		}
	})
	t.Run("store unreadable", func(t *testing.T) {
		broken := store.New(filepath.Join(t.TempDir(), "no-such-dcstore"), filepath.Join(t.TempDir(), "state.sqlite"))
		out := policyCheck(t, devcouncil.NewRegistry(root, broken, g), args)
		payload, ok := out.(devcouncil.ErrorPayload)
		if !ok || payload.OK || payload.Code != "store_error" {
			t.Fatalf("got %T %+v, want ok=false code=store_error", out, out)
		}
	})
	t.Run("unknown task", func(t *testing.T) {
		reg := devcouncil.NewRegistry(root, scopedStore(t), g)
		out := policyCheck(t, reg, map[string]any{"path": "src/a.go", "task_id": "TASK-404"})
		m, ok := out.(map[string]any)
		if !ok || m["ok"] != false || m["code"] != "not_found" || m["task_id"] != "TASK-404" {
			t.Fatalf("got %T %+v, want ok=false code=not_found task_id=TASK-404", out, out)
		}
	})
}

// Nothing enforces a tool schema at runtime. An argument this tool would drop,
// or a value it cannot use, is refused before the gate or the store is touched
// — the shape TestVerifyTaskRefusesArgumentsOutsideItsSchema pins for
// devcouncil_verify_task.
func TestPolicyCheckRefusesArgumentsOutsideItsSchema(t *testing.T) {
	reg := devcouncil.NewRegistry(t.TempDir(), nil, nil)
	for _, tc := range []struct {
		args map[string]any
		name string
	}{
		{map[string]any{"path": "src/a.go", "lease_token": "tok"}, "lease_token"},
		{map[string]any{"path": "src/a.go", "paths": []any{"src/b.go"}}, "paths"},
		{map[string]any{"path": "src/a.go", "operation": "rename"}, "rename"},
		// Manvi's preview sends write; this schema never offered it.
		{map[string]any{"path": "src/a.go", "operation": "write"}, "write"},
		{map[string]any{"path": "src/a.go", "operation": "MODIFY"}, "MODIFY"},
		{map[string]any{"path": "src/a.go", "operation": ""}, "operation"},
		{map[string]any{"path": "src/a.go", "task_id": float64(7)}, "task_id"},
		{map[string]any{"path": "src/a.go", "task_id": nil}, "task_id"},
		{map[string]any{"path": "src/a.go", "task_id": ""}, "task_id"},
		{map[string]any{"path": []any{"src/a.go"}}, "path"},
	} {
		out := policyCheck(t, reg, tc.args)
		payload, ok := out.(devcouncil.ErrorPayload)
		if !ok {
			t.Fatalf("%v: got %T %+v, want an ErrorPayload", tc.args, out, out)
		}
		if payload.OK || payload.Code != "invalid_argument" {
			t.Errorf("%v: payload = %+v, want ok=false code=invalid_argument", tc.args, payload)
		}
		if !strings.Contains(payload.Error, tc.name) {
			t.Errorf("%v: the refusal must name %q: %q", tc.args, tc.name, payload.Error)
		}
	}
}
