//go:build !unix

package main

import (
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gussetfn"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/policy"
)

// runGussetCheck says the check could not run: gusset is unix-only. A check
// that could not run exits non-zero, never as a pass.
func runGussetCheck() int {
	console.Errorf("gusset-check: %v\n", gussetfn.ErrUnsupported)
	return 2
}

// policyMatcher is fnmatch: there is no engine on this platform.
func policyMatcher() policy.Matcher { return nil }
