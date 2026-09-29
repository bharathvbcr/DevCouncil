//go:build unix

package main

import (
	"context"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gussetfn"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
	"github.com/bharathvbcr/gusset"
)

func runGussetCheck() int {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	err := gussetfn.SelfTest(ctx)
	// The check panics on purpose; what Rust logged about it, and anything
	// else, belongs on stderr rather than in a ring nobody reads.
	if _, drainErr := gussetfn.DrainLogs(console.Stderr()); drainErr != nil {
		console.Errorf("gusset-check: draining Rust logs: %v\n", drainErr)
	}
	if err != nil {
		console.Errorf("gusset-check: %v\n", err)
		return 1
	}
	st := gusset.Stats()
	console.Printf("gusset-check: ok (parity, match-any, panic firewall; rust live=%dB peak=%dB allocs=%d; go threads=%d)\n",
		st.LiveBytes, st.PeakBytes, st.AllocCount, gusset.Threads())
	return 0
}

// policyMatcher is the engine: this host always links it on unix.
//
// The engine is checked once here so a broken one is named at startup. The
// matcher is installed either way: a gate that cannot ask it denies each
// decision under path.engine_unavailable or command.engine_unavailable,
// with the cause in the reason, rather than quietly deciding with fnmatch.
func policyMatcher() policy.Matcher {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := gussetfn.Check(ctx); err != nil {
		console.Errorf("devcouncil: the gusset policy engine failed its check (%v); policy decisions will be refused under path.engine_unavailable / command.engine_unavailable until it recovers\n", err)
	}
	return gussetfn.Matcher{}
}
