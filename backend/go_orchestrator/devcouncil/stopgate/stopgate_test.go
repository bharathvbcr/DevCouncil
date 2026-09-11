package stopgate_test

import (
	"context"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/stopgate"
)

func TestSkippedNeverAllows(t *testing.T) {
	r := stopgate.Evaluate(context.Background(), stopgate.Input{
		SkipVerify: true, SkipReason: "no lease", TaskID: "T1",
	})
	if r.Allow {
		t.Fatal("skipped stop gate must not allow")
	}
	if !r.Skipped || r.SkipReason == "" {
		t.Fatalf("expected skip reason, got %+v", r)
	}
}

func TestMissingStoreNeverAllows(t *testing.T) {
	r := stopgate.Evaluate(context.Background(), stopgate.Input{TaskID: "T1"})
	if r.Allow {
		t.Fatal("missing store must not allow")
	}
	if !r.Skipped {
		t.Fatal("missing store must be skipped, not a silent pass")
	}
}
