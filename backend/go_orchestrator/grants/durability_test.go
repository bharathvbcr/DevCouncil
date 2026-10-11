package grants

import (
	"sync"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

func humanOnce(t *testing.T, l *Ledger, target string) Grant {
	t.Helper()
	blocked := denial(policy.RuleUnplannedScope, target, "TASK-1")
	req, err := SuggestRequest(blocked, Grantor{Authority: Human, ID: "op"})
	if err != nil {
		t.Fatal(err)
	}
	req.Reason = "needed"
	req.Scope.Once = true
	g, err := l.Issue(req)
	if err != nil {
		t.Fatal(err)
	}
	return g
}

// The ledger fires one hook for every mutation, and the grant it carries is the
// state after the mutation.
func TestOnChangeFiresForIssueConsumeAndRevoke(t *testing.T) {
	l := ledger(t)
	var got []Change
	l.SetOnChange(func(c Change) { got = append(got, c) })

	g := humanOnce(t, l, "a.go")
	if _, used := l.Apply(denial(policy.RuleUnplannedScope, "a.go", "TASK-1")); !used {
		t.Fatal("grant did not clear its decision")
	}
	g2 := humanOnce(t, l, "b.go")
	if err := l.Revoke(g2.ID); err != nil {
		t.Fatal(err)
	}

	want := []ChangeKind{Issued, Consumed, Issued, Revoked}
	if len(got) != len(want) {
		t.Fatalf("hook saw %d changes %v, want %v", len(got), got, want)
	}
	for i, k := range want {
		if got[i].Kind != k {
			t.Fatalf("change %d = %s, want %s", i, got[i].Kind, k)
		}
	}
	if got[1].Grant.ID != g.ID || !got[1].Grant.Consumed {
		t.Fatalf("consume change must carry the spent grant, got %+v", got[1].Grant)
	}
	if !got[3].Grant.Consumed {
		t.Fatal("revoke change must carry the consumed grant")
	}
}

func TestOnChangeIsSilentForAReusableGrantAndAFailedRevoke(t *testing.T) {
	l := ledger(t)
	var n int
	l.SetOnChange(func(Change) { n++ })
	blocked := denial(policy.RuleUnplannedScope, "a.go", "TASK-1")
	req, _ := SuggestRequest(blocked, Grantor{Authority: Human, ID: "op"})
	req.Reason = "x"
	req.Scope.Once = false
	if _, err := l.Issue(req); err != nil {
		t.Fatal(err)
	}
	l.Apply(blocked)
	if err := l.Revoke("GRANT-9999"); err == nil {
		t.Fatal("revoking an unknown grant must fail")
	}
	if n != 1 {
		t.Fatalf("hook fired %d times, want 1 (the issue only)", n)
	}
}

// The reason the hook exists: a persisted ledger reloaded after a restart must
// not resurrect a spent single-use grant or a revoked one.
func TestConsumedAndRevokedStateSurvivesARoundTrip(t *testing.T) {
	l := ledger(t)
	var saved []Grant
	l.SetOnChange(func(Change) { saved = l.All() })

	humanOnce(t, l, "a.go")
	l.Apply(denial(policy.RuleUnplannedScope, "a.go", "TASK-1"))
	g2 := humanOnce(t, l, "b.go")
	if err := l.Revoke(g2.ID); err != nil {
		t.Fatal(err)
	}

	fresh := ledger(t)
	if refused := fresh.Restore(saved); len(refused) != 0 {
		t.Fatalf("restore refused %v", refused)
	}
	for _, target := range []string{"a.go", "b.go"} {
		if _, used := fresh.Apply(denial(policy.RuleUnplannedScope, target, "TASK-1")); used {
			t.Fatalf("%s was cleared after restart by a grant that was spent or revoked", target)
		}
	}
}

func TestRestoreRefusesDuplicateAndCollidingIDs(t *testing.T) {
	src := ledger(t)
	g := humanOnce(t, src, "a.go")

	fresh := ledger(t)
	refused := fresh.Restore([]Grant{g, g})
	if len(refused) != 1 {
		t.Fatalf("a duplicate inside the file must be refused once, got %v", refused)
	}
	if n := len(fresh.All()); n != 1 {
		t.Fatalf("ledger holds %d grants, want 1", n)
	}
	if err := fresh.Revoke(g.ID); err != nil {
		t.Fatal(err)
	}
	if _, used := fresh.Apply(denial(policy.RuleUnplannedScope, "a.go", "TASK-1")); used {
		t.Fatal("a revoked grant still clears after a duplicate restore")
	}

	live := ledger(t)
	humanOnce(t, live, "c.go") // GRANT-0001 in the live ledger
	if refused := live.Restore([]Grant{g}); len(refused) != 1 {
		t.Fatalf("an ID colliding with the live ledger must be refused, got %v", refused)
	}
}

func TestRevokeConsumesEveryGrantCarryingTheID(t *testing.T) {
	l := ledger(t)
	g := humanOnce(t, l, "a.go")
	l.mu.Lock()
	l.grants = append(l.grants, g) // a duplicate that bypassed Restore
	l.mu.Unlock()
	if err := l.Revoke(g.ID); err != nil {
		t.Fatal(err)
	}
	if len(l.Active()) != 0 {
		t.Fatal("a duplicate survived Revoke")
	}
}

// The hook may call back into the ledger (the host writes All()): firing it
// under the lock would deadlock. Concurrent Apply of one single-use grant must
// also announce exactly one consume.
func TestHookMayReenterTheLedgerAndConsumeAnnouncesOnce(t *testing.T) {
	l := ledger(t)
	var mu sync.Mutex
	var consumed int
	l.SetOnChange(func(c Change) {
		_ = l.All()
		if c.Kind == Consumed {
			mu.Lock()
			consumed++
			mu.Unlock()
		}
	})
	humanOnce(t, l, "a.go")
	var wg sync.WaitGroup
	for i := 0; i < 16; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			l.Apply(denial(policy.RuleUnplannedScope, "a.go", "TASK-1"))
		}()
	}
	wg.Wait()
	if consumed != 1 {
		t.Fatalf("consume announced %d times, want 1", consumed)
	}
}
