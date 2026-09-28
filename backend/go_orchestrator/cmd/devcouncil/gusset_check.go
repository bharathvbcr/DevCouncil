//go:build unix

package main

import (
	"context"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gussetfn"
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
