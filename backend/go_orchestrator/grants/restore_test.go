package grants

import (
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// A ledger file is not a trusted input. It outlives the run that wrote it, it
// sits in the repository where anything can edit it, and it is read back as
// grants that clear refusals. Restore therefore has to apply the checks Issue
// applies — and two of them it did not.
//
// The shape of both defects is the same: a record that Issue would refuse to
// mint came back through Restore as a live grant.

func restoreLedger(t *testing.T, p Policy) *Ledger {
	t.Helper()
	l := NewLedger(p)
	l.Now = func() time.Time { return time.Date(2026, 8, 24, 12, 0, 0, 0, time.UTC) }
	return l
}

// An agent grant is bounded by the task the agent holds. Issue refuses one that
// names no task; Restore used not to, and the result was a grant covering every
// task and every path in the repository.
func TestRestoreRefusesAnAgentGrantThatNamesNoTask(t *testing.T) {
	l := restoreLedger(t, DefaultPolicy())
	now := l.now()

	unscoped := Grant{
		ID:      "GRANT-0001",
		Grantor: Grantor{Authority: Agent, ID: "executor"},
		Reason:  "restored from a previous run",
		Scope: Scope{
			// No TaskID. Issue refuses exactly this.
			Rules: []policy.RuleID{policy.RuleUnplannedScope},
		},
		IssuedAt:  now.Add(-time.Minute),
		ExpiresAt: now.Add(10 * time.Minute),
	}

	refused := l.Restore([]Grant{unscoped})
	if len(refused) != 1 {
		t.Fatalf("an unscoped agent grant must be refused on restore; refusals=%v", refused)
	}
	if !strings.Contains(refused[0], "names no task") {
		t.Errorf("refusal should say why: %q", refused[0])
	}

	// And it must not clear anything.
	d := policy.Decision{
		Action: policy.Deny, Rule: policy.RuleUnplannedScope,
		Severity: policy.Soft, Target: "any/other/file.go", TaskID: "SOME-OTHER-TASK",
	}
	if _, used := l.Apply(d); used {
		t.Fatal("the refused grant still cleared a decision on an unrelated task")
	}

	// The same grant, correctly scoped, restores — so this is a scoping check
	// rather than a blanket refusal of restored agent grants.
	scoped := unscoped
	scoped.Scope.TaskID = "TASK-001"
	if refused := l.Restore([]Grant{scoped}); len(refused) != 0 {
		t.Fatalf("a task-scoped agent grant should restore; refusals=%v", refused)
	}
}

// The tightest ceiling an operator can set used to be the one that switched the
// expiry check off, because the guard read `ceiling > 0`.
func TestRestoreEnforcesTheCeilingEvenWhenItIsZero(t *testing.T) {
	for _, tc := range []struct {
		name    string
		ceiling time.Duration
	}{
		{"zero ceiling", 0},
		{"negative ceiling is not a longer one", -time.Hour},
	} {
		t.Run(tc.name, func(t *testing.T) {
			p := DefaultPolicy()
			p.AgentMaxTTL = tc.ceiling
			l := restoreLedger(t, p)
			now := l.now()

			forever := Grant{
				ID:        "GRANT-0001",
				Grantor:   Grantor{Authority: Agent, ID: "executor"},
				Reason:    "restored from a previous run",
				Scope:     Scope{TaskID: "TASK-001", Rules: []policy.RuleID{policy.RuleUnplannedScope}},
				IssuedAt:  now.Add(-time.Minute),
				ExpiresAt: now.Add(100 * 365 * 24 * time.Hour),
			}
			refused := l.Restore([]Grant{forever})
			if len(refused) != 1 {
				t.Fatalf("a ceiling of %s can only mint a grant that expires at once; "+
					"a hundred-year record must not restore. refusals=%v", tc.ceiling, refused)
			}
			if !strings.Contains(refused[0], "ceiling") {
				t.Errorf("refusal should name the ceiling: %q", refused[0])
			}
		})
	}
}

// Issue is the other half of the same ceiling. A request with no lifetime takes
// the ceiling, and a non-positive ceiling then passes `ttl > ceiling`, so Issue
// used to append and return a grant whose ExpiresAt was already behind it, with
// a nil error: the caller was told a grant exists, the ledger carried a dead
// record, and nothing named the misconfiguration. A ceiling that admits no
// lifetime is refused with ErrTTL, and no grant is recorded.
func TestIssueRefusesANonPositiveCeiling(t *testing.T) {
	for _, tc := range []struct {
		name      string
		authority Authority
		ceiling   time.Duration
		ttl       time.Duration
	}{
		{"agent, zero ceiling, default lifetime", Agent, 0, 0},
		{"agent, negative ceiling, default lifetime", Agent, -15 * time.Minute, 0},
		{"human, zero ceiling, default lifetime", Human, 0, 0},
		{"human, negative ceiling, default lifetime", Human, -time.Hour, 0},
		{"agent, negative ceiling, explicit lifetime", Agent, -15 * time.Minute, time.Minute},
	} {
		t.Run(tc.name, func(t *testing.T) {
			p := DefaultPolicy()
			if tc.authority == Agent {
				p.AgentMaxTTL = tc.ceiling
			} else {
				p.HumanMaxTTL = tc.ceiling
			}
			l := restoreLedger(t, p)
			req := Request{
				Grantor:   Grantor{Authority: tc.authority, ID: "issuer"},
				Reason:    "needed for the fix",
				Scope:     Scope{TaskID: "TASK-001", Rules: []policy.RuleID{policy.RuleUnplannedScope}},
				TTL:       tc.ttl,
				AgentTask: "TASK-001",
			}
			g, err := l.Issue(req)
			if !errors.Is(err, ErrTTL) {
				t.Fatalf("Issue under a %s ceiling = (%+v, %v), want ErrTTL", tc.ceiling, g, err)
			}
			if !strings.Contains(err.Error(), "ceiling") {
				t.Errorf("refusal should name the ceiling: %q", err)
			}
			if g.ID != "" {
				t.Errorf("a refused Issue returned a grant: %+v", g)
			}
			if all := l.All(); len(all) != 0 {
				t.Errorf("a refused Issue recorded %d grant(s): %+v", len(all), all)
			}
		})
	}

	// The smallest positive ceiling still issues, so this is a bound on the
	// configuration, not a refusal of short grants.
	p := DefaultPolicy()
	p.AgentMaxTTL = time.Second
	l := restoreLedger(t, p)
	g, err := l.Issue(Request{
		Grantor:   Grantor{Authority: Agent, ID: "executor"},
		Reason:    "needed for the fix",
		Scope:     Scope{TaskID: "TASK-001", Rules: []policy.RuleID{policy.RuleUnplannedScope}},
		AgentTask: "TASK-001",
	})
	if err != nil {
		t.Fatalf("a one-second ceiling must still issue: %v", err)
	}
	if !g.ExpiresAt.After(g.IssuedAt) {
		t.Fatalf("issued grant expires at %s, not after %s", g.ExpiresAt, g.IssuedAt)
	}
}

// The converse, so the two fixes above are not simply "refuse more": an
// ordinary, well-formed record still restores and still works.
func TestRestoreStillAcceptsAWellFormedGrant(t *testing.T) {
	l := restoreLedger(t, DefaultPolicy())
	now := l.now()

	good := Grant{
		ID:        "GRANT-0007",
		Grantor:   Grantor{Authority: Agent, ID: "executor"},
		Reason:    "widening scope for the test beside the file being fixed",
		Scope:     Scope{TaskID: "TASK-001", Rules: []policy.RuleID{policy.RuleUnplannedScope}, Paths: []string{"src/**"}},
		IssuedAt:  now.Add(-time.Minute),
		ExpiresAt: now.Add(5 * time.Minute),
	}
	if refused := l.Restore([]Grant{good}); len(refused) != 0 {
		t.Fatalf("a well-formed agent grant must restore; refusals=%v", refused)
	}
	d := policy.Decision{
		Action: policy.Deny, Rule: policy.RuleUnplannedScope,
		Severity: policy.Soft, Target: "src/helper.go", TaskID: "TASK-001",
	}
	cleared, used := l.Apply(d)
	if !used {
		t.Fatal("the restored grant should clear the decision it was scoped for")
	}
	if cleared.GrantID != "GRANT-0007" {
		t.Errorf("cleared by %q, want GRANT-0007", cleared.GrantID)
	}
}
