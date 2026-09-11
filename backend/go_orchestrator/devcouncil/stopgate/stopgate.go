// Package stopgate is the Phase-5 port of Python execution/stop_gate decision
// shapes. Hosts (Claude/Cursor hooks, Manvi) call Evaluate; the heavy verify
// work is delegated to package verify. A check that could not run is never a
// pass — Evaluate surfaces skip reasons on the result.
package stopgate

import (
	"context"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

// Result mirrors the Python StopGateResult fields consumers branch on.
type Result struct {
	OK               bool     `json:"ok"`
	Allow            bool     `json:"allow"`
	Reason           string   `json:"reason"`
	TaskID           string   `json:"task_id,omitempty"`
	Status           string   `json:"status,omitempty"`
	BlockingGapCount int      `json:"blocking_gap_count"`
	Skipped          bool     `json:"skipped"`
	SkipReason       string   `json:"skip_reason,omitempty"`
	VerifiedAt       string   `json:"verified_at,omitempty"`
	NextActions      []string `json:"next_actions,omitempty"`
}

// Input is what a host hook supplies.
type Input struct {
	Root     string
	Store    *store.Client
	TaskID   string
	GateMode string
	Sandbox  string
	// SkipVerify forces a skipped result (e.g. gate mode off, no lease).
	SkipVerify bool
	SkipReason string
}

// Evaluate runs verification (unless skipped) and maps it to a stop decision.
//
// Invariant: SkipVerify / store-unavailable / verify error → Allow=false with
// Skipped=true and an explicit reason. Never Allow=true because a check did
// not run.
func Evaluate(ctx context.Context, in Input) Result {
	now := time.Now().UTC().Format(time.RFC3339Nano)
	if in.SkipVerify {
		reason := in.SkipReason
		if reason == "" {
			reason = "stop gate skipped by host"
		}
		return Result{
			OK: true, Allow: false, Skipped: true, SkipReason: reason,
			TaskID: in.TaskID, Reason: reason, VerifiedAt: now,
		}
	}
	if in.Store == nil {
		return Result{
			OK: false, Allow: false, Skipped: true,
			SkipReason: "DevCouncil store unavailable",
			TaskID:     in.TaskID, Reason: "DevCouncil store unavailable",
			VerifiedAt: now,
		}
	}
	if in.TaskID == "" {
		return Result{
			OK: false, Allow: false, Skipped: true,
			SkipReason: "no task_id", Reason: "no task_id", VerifiedAt: now,
		}
	}
	gateMode := in.GateMode
	if gateMode == "" {
		gateMode = "enforce"
	}
	sandbox := in.Sandbox
	if sandbox == "" {
		sandbox = "local"
	}
	mcp, gaps, err := verify.VerifyTask(ctx, in.Root, in.Store, in.TaskID, gateMode, sandbox)
	if err != nil {
		return Result{
			OK: false, Allow: false, Skipped: true,
			SkipReason: "verify could not run: " + err.Error(),
			TaskID:     in.TaskID, Reason: err.Error(), VerifiedAt: now,
		}
	}
	actions := make([]string, 0, len(mcp.NextActions))
	for _, a := range mcp.NextActions {
		actions = append(actions, a.Action)
	}
	blocking := 0
	for _, g := range gaps {
		if g.Blocking {
			blocking++
		}
	}
	allow := mcp.Passed && gateMode != "off"
	if gateMode == "off" {
		allow = true
	}
	reason := "verified"
	if !mcp.Passed {
		reason = "blocked by verification"
	}
	return Result{
		OK: true, Allow: allow, Reason: reason, TaskID: in.TaskID,
		Status: mcp.Status, BlockingGapCount: blocking,
		VerifiedAt: now, NextActions: actions,
	}
}
