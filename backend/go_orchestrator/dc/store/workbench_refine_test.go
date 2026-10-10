package store

import (
	"encoding/json"
	"errors"
	"fmt"
	"testing"
)

// The Go client forwards refinement proposals through the ordinary
// enhancements.* methods; what it must keep true across the process boundary
// is that the Rust store's typed refusals survive as WorkbenchError codes and
// that an accepted refinement lands on the board.
func TestWorkbenchRefinementProposalRoundTripAndTypedRefusals(t *testing.T) {
	c := client(t)
	t.Cleanup(c.Close)
	call := func(method, input string) json.RawMessage {
		t.Helper()
		out, err := c.Workbench(t.Context(), method, json.RawMessage(input))
		if err != nil {
			t.Fatalf("%s: %v", method, err)
		}
		return out
	}
	code := func(method, input string) string {
		t.Helper()
		_, err := c.Workbench(t.Context(), method, json.RawMessage(input))
		var refusal *WorkbenchError
		if !errors.As(err, &refusal) {
			t.Fatalf("%s: want a typed refusal, got %v", method, err)
		}
		return refusal.Code
	}
	call("repositories.put", `{"request_id":"r","id":"r1","expected_revision":0,"name":"Manvi","identity_key":"local:r1"}`)
	call("items.put", `{"request_id":"t","id":"t1","expected_revision":0,"title":"Fix E42","description":"Keep E42.","repository_ids":["r1"],"primary_repository_id":"r1"}`)
	call("items.put", `{"request_id":"w","id":"twin","expected_revision":0,"title":"Fix E42 too","repository_ids":["r1"],"primary_repository_id":"r1"}`)

	create := `{"request_id":"c","id":"e1","expected_revision":0,"task_id":"t1","source_revision":%d,"kind":"refine","fields":["acceptance_criteria","split_into","duplicate_of"],"provider":"local","model":"m"}`
	if got := code("enhancements.create", fmt.Sprintf(create, 7)); got != "revision_conflict" {
		t.Fatalf("stale source revision: %s", got)
	}
	call("enhancements.create", fmt.Sprintf(create, 1))
	if got := code("enhancements.create", `{"request_id":"c2","id":"e2","expected_revision":0,"task_id":"t1","source_revision":1,"kind":"refine","fields":["duplicate_of"],"provider":"local","model":"m"}`); got != "busy" {
		t.Fatalf("second generation: %s", got)
	}
	call("enhancements.complete", `{"request_id":"d","id":"e1","expected_revision":1,"acceptance_criteria":["E42 is reported once"],"split_into":[{"title":"Part A","description":"Keep E42."}],"duplicate_of":[{"id":"twin","reason":"same"}]}`)
	call("enhancements.accept", `{"request_id":"a","id":"e1","expected_revision":2,"expected_task_revision":1,"fields":["acceptance_criteria","split_into","duplicate_of"],"split_ids":["part-a"]}`)

	var task struct {
		Item struct {
			Revision           int      `json:"revision"`
			AcceptanceCriteria []string `json:"acceptance_criteria"`
			Links              []struct {
				Kind   string `json:"kind"`
				ItemID string `json:"item_id"`
			} `json:"links"`
		} `json:"item"`
	}
	if err := json.Unmarshal(call("items.get", `{"id":"t1"}`), &task); err != nil {
		t.Fatal(err)
	}
	if task.Item.Revision != 2 || len(task.Item.AcceptanceCriteria) != 1 || len(task.Item.Links) != 1 || task.Item.Links[0].Kind != "duplicate_of" {
		t.Fatalf("accepted refinement did not land: %+v", task.Item)
	}
	call("items.get", `{"id":"part-a"}`)
}
