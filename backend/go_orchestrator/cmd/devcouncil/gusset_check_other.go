//go:build !unix

package main

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gussetfn"
)

// runGussetCheck says the check could not run: gusset is unix-only. A check
// that could not run exits non-zero, never as a pass.
func runGussetCheck() int {
	console.Errorf("gusset-check: %v\n", gussetfn.ErrUnsupported)
	return 2
}
