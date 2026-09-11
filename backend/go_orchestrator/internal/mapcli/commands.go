package mapcli

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// statusPayload is `dcmap status`.
//
// It carries the kernel's self-report plus the resolved binary, because "which
// devmap answered this?" is the first question when a status looks wrong and
// the Python surface never printed it — a stale ~/.cargo/bin install reporting
// on a repository it had never seen was indistinguishable from a current one.
type statusPayload struct {
	Binary       string `json:"binary"`
	DBPath       string `json:"db_path"`
	GenerationID int    `json:"generation_id"`
	NodeCount    int    `json:"node_count"`
	EdgeCount    int    `json:"edge_count"`
	PendingCount int    `json:"pending_count"`
	Quarantined  int    `json:"quarantined_count"`
	// KernelFresh is the kernel's view of its own store. It is deliberately
	// NOT called "fresh": the Python surface reports two different freshness
	// questions side by side, and on this repository they disagree — the
	// kernel says fresh while `map: STALE` because tracked file contents moved
	// since the artifact was written. Reporting one bare "freshness" would
	// present the narrower check as if it answered the broader one.
	//
	// The map-vs-working-tree question needs `devmap freshness --expect-*`
	// against the artifact's stamps and is not answered here yet; it is absent
	// from the payload rather than guessed, so a consumer cannot mistake a
	// missing check for a passing one.
	KernelFresh    bool   `json:"kernel_fresh"`
	DegradedReason string `json:"degraded_reason"`
}

func (s statusPayload) Human(w io.Writer) {
	fmt.Fprintf(w, "kernel      %s\n", s.Binary)
	fmt.Fprintf(w, "store       %s\n", s.DBPath)
	fmt.Fprintf(w, "generation  %d\n", s.GenerationID)
	fmt.Fprintf(w, "symbols     %d\n", s.NodeCount)
	fmt.Fprintf(w, "edges       %d\n", s.EdgeCount)
	fmt.Fprintf(w, "pending     %d\n", s.PendingCount)
	fmt.Fprintf(w, "quarantined %d\n", s.Quarantined)
	fresh := "not fresh"
	if s.KernelFresh {
		fresh = "fresh"
	}
	fmt.Fprintf(w, "kernel-view %s\n", fresh)
	if s.DegradedReason != "" {
		fmt.Fprintf(w, "degraded    %s\n", s.DegradedReason)
	}
}

func runStatus(ctx context.Context, env *Env, args []string) (Payload, error) {
	if len(args) > 0 {
		return nil, fmt.Errorf("status takes no arguments, got %d", len(args))
	}
	client, err := clientFor(ctx, env)
	if err != nil {
		return nil, err
	}
	status, err := client.Status(ctx)
	if err != nil {
		return nil, err
	}
	return fromStatus(env.Binary, status), nil
}

func fromStatus(binary string, s *devmap.Status) statusPayload {
	degraded := ""
	if s.DegradedReason != nil {
		degraded = *s.DegradedReason
	}
	return statusPayload{
		Binary:         binary,
		DBPath:         s.DBPath,
		GenerationID:   s.GenerationID,
		NodeCount:      s.NodeCount,
		EdgeCount:      s.EdgeCount,
		PendingCount:   s.PendingCount,
		Quarantined:    s.Quarantined,
		KernelFresh:    s.IsFresh,
		DegradedReason: degraded,
	}
}

// doctorPayload is `dcmap doctor`: is this repository's map usable, and if not,
// what is the remedy?
type doctorPayload struct {
	OK      bool     `json:"ok"`
	Binary  string   `json:"binary"`
	Checks  []check  `json:"checks"`
	Remedy  string   `json:"remedy"`
	Reasons []string `json:"reasons"`
}

type check struct {
	Name   string `json:"name"`
	Passed bool   `json:"passed"`
	Detail string `json:"detail"`
}

func (d doctorPayload) Human(w io.Writer) {
	for _, c := range d.Checks {
		mark := "FAIL"
		if c.Passed {
			mark = "ok"
		}
		fmt.Fprintf(w, "[%-4s] %-12s %s\n", mark, c.Name, c.Detail)
	}
	if !d.OK && d.Remedy != "" {
		fmt.Fprintf(w, "\nfix: %s\n", d.Remedy)
	}
}

// runDoctor reports every check it ran, passing or failing.
//
// It deliberately does not stop at the first failure. A doctor that reports
// only "no kernel" hides that the store is also empty, so the operator fixes
// one thing, re-runs, and learns about the next — the report exists to make one
// pass enough.
func runDoctor(ctx context.Context, env *Env, args []string) (Payload, error) {
	if len(args) > 0 {
		return nil, fmt.Errorf("doctor takes no arguments, got %d", len(args))
	}
	out := doctorPayload{OK: true}

	binary, err := discoverBinary(ctx, env.Root)
	if err != nil {
		out.OK = false
		out.Checks = append(out.Checks, check{
			Name: "kernel", Passed: false,
			Detail: "no devmap binary answered the capability probe",
		})
		out.Reasons = append(out.Reasons, "kernel missing or incapable")
		out.Remedy = "cargo install --path rust/devmap-cli, or set DEVMAP_BINARY"
		// Without a kernel the remaining checks have nothing to ask, so the
		// report ends here rather than inventing verdicts it did not measure.
		return out, nil
	}
	env.Binary = binary
	out.Binary = binary
	out.Checks = append(out.Checks, check{Name: "kernel", Passed: true, Detail: binary})

	client := devmap.New(binary, env.Root)
	status, err := client.Status(ctx)
	if err != nil {
		out.OK = false
		out.Checks = append(out.Checks, check{Name: "store", Passed: false, Detail: err.Error()})
		out.Reasons = append(out.Reasons, "store unreadable")
		out.Remedy = "devmap build"
		return out, nil
	}
	out.Checks = append(out.Checks, check{
		Name: "store", Passed: true,
		Detail: fmt.Sprintf("%s (generation %d)", status.DBPath, status.GenerationID),
	})

	generationOK := status.GenerationID > 0 && status.NodeCount > 0
	detail := fmt.Sprintf("%d symbols, %d edges", status.NodeCount, status.EdgeCount)
	if !generationOK {
		detail = "no committed generation with symbols"
		out.OK = false
		out.Reasons = append(out.Reasons, "no usable generation")
		out.Remedy = "devmap build"
	}
	out.Checks = append(out.Checks, check{Name: "generation", Passed: generationOK, Detail: detail})

	if status.DegradedReason != nil && *status.DegradedReason != "" {
		out.OK = false
		out.Reasons = append(out.Reasons, *status.DegradedReason)
		out.Checks = append(out.Checks, check{Name: "degraded", Passed: false, Detail: *status.DegradedReason})
		if out.Remedy == "" {
			// Not "devmap build". Some degradations are coverage gaps from
			// grammars this build does not link, which the kernel's own message
			// calls permanent — prescribing a rebuild for those sends the
			// operator round a loop that cannot terminate.
			out.Remedy = "a rebuild (`devmap build`) clears transient parse failures only; " +
				"coverage gaps from unlinked grammars are permanent for this build"
		}
	}
	return out, nil
}

// passthrough hands a subcommand to the kernel unchanged.
//
// The kernel's stdout is this process's stdout and its stderr is this process's
// stderr, so its JSON reaches the caller byte-for-byte and nothing this CLI
// might add can contaminate it. That is why the returned payload is nil: the
// single stdout artifact has already been written by the child, and [emit]
// must not append a second object beside it.
func passthrough(ctx context.Context, env *Env, name string, args []string) (Payload, error) {
	if err := resolveBinary(ctx, env); err != nil {
		return nil, err
	}

	// --json goes BEFORE the subcommand. The kernel declares it on the root
	// command without clap's `global = true`, so `devmap search foo --json` is
	// an "unexpected argument" parse error rather than a JSON search — verified
	// against the binary, not inferred from the flag being listed under
	// "Options". The client's own decode() places it the same way.
	var argv []string
	if env.JSON {
		argv = append(argv, "--json")
	}
	argv = append(argv, name)
	argv = append(argv, args...)

	// A delegated command can be slow — `explore` and `clones` walk the whole
	// generation — so the bound is generous, but it exists. An unbounded
	// passthrough would make this CLI the one process boundary in the module
	// that can hang forever, which is what internal/proc's gate is there to
	// prevent.
	ctx, cancel := context.WithTimeout(ctx, passthroughTimeout)
	defer cancel()

	cmd := exec.CommandContext(ctx, env.Binary, argv...)
	// The deadline has to reach anything the kernel spawned, not just the
	// kernel: a grandchild holding the inherited stdout pipe keeps Wait blocked
	// long after the direct child is killed. See proc.ConfigureGroup.
	proc.ConfigureGroup(cmd)
	cmd.Dir = env.Root
	cmd.Stdout = passthroughStdout
	cmd.Stderr = env.Stderr
	cmd.Stdin = nil
	cmd.WaitDelay = 2 * time.Second

	// RunBounded rather than cmd.Run: CommandContext arms its killer only once
	// the child exists and WaitDelay only bounds Wait, so a Start blocked in
	// the kernel sits outside both. Running the whole call under the context
	// closes that gap.
	runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		return nil, fmt.Errorf("devmap %s did not return within %s", name, passthroughTimeout)
	}
	if runErr != nil {
		return nil, fmt.Errorf("devmap %s: %w", name, runErr)
	}
	return nil, nil
}

// passthroughTimeout bounds one delegated invocation.
const passthroughTimeout = 10 * time.Minute

// passthroughStdout is the real stdout a delegated child writes to. It is a
// package variable so a test can capture a delegated invocation without the
// child having to know it is being observed.
var passthroughStdout io.Writer = os.Stdout
