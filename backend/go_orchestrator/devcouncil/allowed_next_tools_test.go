package devcouncil_test

// The seam between what a verify tells an agent to do next and what this host
// can actually be asked to do.
//
// verify.AllowedNextToolsForVerify spells the list itself, because package
// devcouncil imports verify and reading the registry back would be an import
// cycle. A second copy of a list is only safe if something holds the two to
// each other, and this package is the one place that can see both.
//
// The drift it exists to catch was real (GAP-P7-NEXT-TOOLS-DRIFT): the list
// carried the Python MCP surface's names — devcouncil_read_file,
// devcouncil_write_file, devcouncil_apply_patch, devcouncil_run_command,
// devcouncil_get_evidence, devcouncil_update_task_scope — and six of the eight
// were served by nothing here. A blocked verify handed the agent a repair
// contract whose every next step was a tool call that fails.

import (
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// TestAllowedNextToolsAreAllServedByThisHost is the direction that matters: a
// tool named as a next step must exist.
func TestAllowedNextToolsAreAllServedByThisHost(t *testing.T) {
	served := map[string]bool{}
	for _, spec := range devcouncil.NewRegistry(t.TempDir(), nil, nil).Specs() {
		served[spec.Name] = true
	}

	allowed := verify.AllowedNextToolsForVerify()
	if len(allowed) == 0 {
		t.Fatal("a blocked verify that names no next tool leaves the agent nothing to call")
	}
	for _, name := range allowed {
		if !served[name] {
			t.Errorf("verify names %s as an allowed next tool, but this host serves no such tool; "+
				"an agent following the repair contract would call into nothing", name)
		}
	}
}

// TestEveryServedToolIsAnAllowedNextTool is the other direction, and it is an
// assertion about this host specifically rather than a general principle.
//
// All eight tools here are ones an agent may legitimately reach for after a
// verify — read the diff, read the gaps, re-verify, renew or release the lease,
// check a write against policy, take the next task. Nothing in the set is
// unsafe at that moment, so withholding one would be an arbitrary restriction
// that the next person to add a tool would have to rediscover. If a tool is
// ever added that genuinely must not follow a verify, this test is where that
// decision gets written down rather than silently made by omission.
func TestEveryServedToolIsAnAllowedNextTool(t *testing.T) {
	allowed := map[string]bool{}
	for _, name := range verify.AllowedNextToolsForVerify() {
		if allowed[name] {
			t.Errorf("%s is named twice", name)
		}
		allowed[name] = true
	}

	for _, spec := range devcouncil.NewRegistry(t.TempDir(), nil, nil).Specs() {
		if !allowed[spec.Name] {
			t.Errorf("this host serves %s but a verify does not name it as an allowed next step; "+
				"if that is deliberate, say so here", spec.Name)
		}
	}
}
