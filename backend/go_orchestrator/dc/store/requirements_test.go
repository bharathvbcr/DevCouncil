package store

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// plantTaskWithLinks seeds a store and writes one task carrying both the
// requirement and acceptance-criterion links, the way DevCouncil's planner
// writes them.
//
// The row is planted through the sqlite3 CLI rather than through this client
// because planning is DevCouncil's job: what is under test is whether the two
// planes agree about a row that already exists, not how it came to exist.
func plantTaskWithLinks(t *testing.T, reqs, acs string) (*Client, string) {
	t.Helper()
	bin := testsupport.DCStore(t)
	db := filepath.Join(t.TempDir(), "state.sqlite")
	c := New(bin, db)

	if _, err := c.ReadyTasks(context.Background()); err != nil {
		t.Fatalf("seeding the store: %v", err)
	}

	sqlite, err := exec.LookPath("sqlite3")
	if err != nil {
		testsupport.Unavailable(t, "sqlite3 is not on PATH, so no task row can be planted")
	}
	stmt := "INSERT INTO tasks (id, title, description, status, " +
		"requirement_ids_json, acceptance_criterion_ids_json) VALUES " +
		"('TASK-1', 'planted', '', 'ready', '" + reqs + "', '" + acs + "');"
	// Bound to the test's context so a sqlite3 that hangs is killed with the
	// test rather than outliving it.
	// #nosec G204 -- sqlite is resolved from PATH by exec.LookPath, db is this
	// test's own t.TempDir, and stmt is assembled from literals and this
	// package's own fixtures.
	cmd := exec.CommandContext(t.Context(), sqlite, db, stmt)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("planting the task: %v\n%s", err, out)
	}
	return c, db
}

// The link from a task to the requirements it exists to satisfy has to survive
// the process boundary, or nothing above it can check requirement coverage.
//
// Before this, the Rust store created both columns and selected neither, and
// this client had no field to put them in. Every task on the Go side reported
// no requirements — not an empty list, but silence, which a coverage gate
// cannot tell apart from a task that genuinely satisfies none.
func TestATasksRequirementsCrossTheBoundary(t *testing.T) {
	c, _ := plantTaskWithLinks(t, `["REQ-1","REQ-2"]`, `["AC-1","AC-2","AC-3"]`)

	task, err := c.Task(context.Background(), "TASK-1")
	if err != nil {
		t.Fatalf("reading the task: %v", err)
	}
	if task == nil {
		t.Fatal("the planted task read back as unknown")
	}

	want := []string{"REQ-1", "REQ-2"}
	if len(task.RequirementIDs) != len(want) {
		t.Fatalf("requirement ids = %v, want %v", task.RequirementIDs, want)
	}
	for i, id := range want {
		if task.RequirementIDs[i] != id {
			t.Errorf("requirement id %d = %q, want %q", i, task.RequirementIDs[i], id)
		}
	}

	wantAC := []string{"AC-1", "AC-2", "AC-3"}
	if len(task.AcceptanceCriterionIDs) != len(wantAC) {
		t.Fatalf("acceptance criterion ids = %v, want %v", task.AcceptanceCriterionIDs, wantAC)
	}
	for i, id := range wantAC {
		if task.AcceptanceCriterionIDs[i] != id {
			t.Errorf("acceptance criterion id %d = %q, want %q", i, task.AcceptanceCriterionIDs[i], id)
		}
	}
}

// plantRequirements writes requirement rows through sqlite3, as the planner
// would, and points TASK-1 at reqIDs.
func plantRequirements(t *testing.T, db, reqIDs string, rows ...string) {
	t.Helper()
	sqlite, err := exec.LookPath("sqlite3")
	if err != nil {
		testsupport.Unavailable(t, "sqlite3 is not on PATH, so no requirement row can be planted")
	}
	stmt := "UPDATE tasks SET requirement_ids_json = '" + reqIDs + "' WHERE id = 'TASK-1';"
	for _, row := range rows {
		stmt += "INSERT INTO requirements (id, title, description, priority, source, " +
			"acceptance_criteria_json) VALUES " + row + ";"
	}
	// #nosec G204 -- sqlite is resolved by exec.LookPath, db is this test's
	// own t.TempDir, and stmt is built from this file's literals.
	cmd := exec.CommandContext(t.Context(), sqlite, db, stmt)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("planting requirements: %v\n%s", err, out)
	}
}

// The verifier dispatches on each criterion's verification method, which it
// can only do if the requirement rows reach this side decoded.
func TestRequirementsCrossTheBoundaryDecoded(t *testing.T) {
	c, db := plantTaskWithLinks(t, `[]`, `["AC-1","AC-2"]`)
	plantRequirements(t, db, `["REQ-1","REQ-GONE"]`,
		`('REQ-1','sums','','high','user','[{"id":"AC-1","description":"adds","verification_method":"unit_test"},`+
			`{"id":"AC-2","description":"by hand","verification_method":"manual","required":false}]')`)

	linked, err := c.Requirements(context.Background(), "TASK-1")
	if err != nil {
		t.Fatalf("reading requirements: %v", err)
	}
	if len(linked.Requirements) != 1 || linked.Requirements[0].ID != "REQ-1" {
		t.Fatalf("requirements = %+v, want [REQ-1]", linked.Requirements)
	}
	acs := linked.Requirements[0].AcceptanceCriteria
	if len(acs) != 2 || acs[0].Method != dc.VerifyUnitTest || acs[1].Method != dc.VerifyManual {
		t.Fatalf("criteria = %+v", acs)
	}
	if !acs[0].Required || acs[1].Required {
		t.Errorf("required flags = %v,%v, want true,false", acs[0].Required, acs[1].Required)
	}
	if len(linked.Missing) != 1 || linked.Missing[0] != "REQ-GONE" {
		t.Errorf("missing = %v, want [REQ-GONE]", linked.Missing)
	}
}

// A stored criterion no gate can discharge is an error, not a criterion
// quietly left out of the list the verifier dispatches on.
func TestARequirementCarryingAnUndischargeableMethodIsAnError(t *testing.T) {
	c, db := plantTaskWithLinks(t, `[]`, `["AC-1"]`)
	plantRequirements(t, db, `["REQ-1"]`,
		`('REQ-1','reads well','','high','user','[{"id":"AC-1","description":"x","verification_method":"llm_review"}]')`)

	if _, err := c.Requirements(context.Background(), "TASK-1"); err == nil ||
		!strings.Contains(err.Error(), "llm_review") {
		t.Fatalf("err = %v, want a refusal naming llm_review", err)
	}
	if _, err := c.Requirements(context.Background(), "NO-SUCH-TASK"); err == nil {
		t.Fatal("an unknown task read as one with no requirements")
	}
}

// The domain type the policy gate evaluates carries them too.
//
// Domain() is the seam between the wire form and the type the rest of the
// harness uses; a field decoded here but dropped there would be no better than
// never decoding it.
func TestTheDomainTaskCarriesItsRequirements(t *testing.T) {
	c, _ := plantTaskWithLinks(t, `["REQ-7"]`, `["AC-9"]`)

	task, err := c.Task(context.Background(), "TASK-1")
	if err != nil {
		t.Fatalf("reading the task: %v", err)
	}
	// A nil task with no error is Task's documented answer for an unknown id,
	// so it means the planting above did not take. Reported rather than
	// dereferenced: the assertions below would otherwise panic on a nil map
	// field, and "invalid memory address" does not say which step failed.
	if task == nil {
		t.Fatal("the planted TASK-1 was not found; the row never landed")
	}
	domain := task.Domain()
	if len(domain.RequirementIDs) != 1 || domain.RequirementIDs[0] != "REQ-7" {
		t.Errorf("domain requirement ids = %v, want [REQ-7]", domain.RequirementIDs)
	}
	if len(domain.AcceptanceCriterionIDs) != 1 || domain.AcceptanceCriterionIDs[0] != "AC-9" {
		t.Errorf("domain acceptance criterion ids = %v, want [AC-9]", domain.AcceptanceCriterionIDs)
	}
}

// A task linked to nothing reads as an empty list, which is a different answer
// from "this plane does not report requirements".
func TestAnUnlinkedTaskReportsNoRequirementsRatherThanFailing(t *testing.T) {
	bin := testsupport.DCStore(t)
	db := filepath.Join(t.TempDir(), "state.sqlite")
	c := New(bin, db)
	if _, err := c.ReadyTasks(context.Background()); err != nil {
		t.Fatalf("seeding the store: %v", err)
	}
	sqlite, err := exec.LookPath("sqlite3")
	if err != nil {
		testsupport.Unavailable(t, "sqlite3 is not on PATH, so no task row can be planted")
	}
	// #nosec G204 -- sqlite is resolved from PATH by exec.LookPath, db is this
	// test's own t.TempDir, and the statement is a literal.
	cmd := exec.CommandContext(t.Context(), sqlite, db,
		"INSERT INTO tasks (id, title, description, status) VALUES ('TASK-1','planted','','ready');")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("planting the task: %v\n%s", err, out)
	}

	task, err := c.Task(context.Background(), "TASK-1")
	if err != nil {
		t.Fatalf("reading the task: %v", err)
	}
	// See above: nil with no error means the planted row is not there, which
	// is a different failure from a task that reports no requirements — and
	// this test exists to tell those two apart.
	if task == nil {
		t.Fatal("the planted TASK-1 was not found; the row never landed")
	}
	if len(task.RequirementIDs) != 0 {
		t.Errorf("requirement ids = %v, want empty", task.RequirementIDs)
	}
	if len(task.AcceptanceCriterionIDs) != 0 {
		t.Errorf("acceptance criterion ids = %v, want empty", task.AcceptanceCriterionIDs)
	}
}

// Guards the environment assumption the two planting helpers rest on: if
// sqlite3 disappears, the tests above must not quietly stop proving anything.
func TestTheStoreFileIsRealAndPlantable(t *testing.T) {
	_, db := plantTaskWithLinks(t, `["REQ-1"]`, `["AC-1"]`)
	if _, err := os.Stat(db); err != nil {
		t.Fatalf("the store file the tests planted into is missing: %v", err)
	}
}
