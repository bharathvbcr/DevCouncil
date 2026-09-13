package mapcli

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// discoverBinary uses an explicitly selected DEVMAP_BINARY or an installed
// PATH binary outside the selected checkout. A repository's build artifacts
// are executable content, so capability probing requires explicit selection.
func discoverBinary(ctx context.Context, root string) (string, error) {
	// An explicit override is used or refused, never replaced. The candidate
	// list puts it first, but "first candidate that answers the probe" fell
	// through to a local build or PATH when the named binary did not — and
	// `status` then reported another kernel's name for a deliberate test of
	// this one. Measured with the built client: DEVMAP_BINARY naming a script
	// that printed garbage, hung, or exited 3 was answered by ~/.cargo/bin/devmap.
	if env := os.Getenv("DEVMAP_BINARY"); env != "" {
		binary := env
		if abs, err := filepath.Abs(env); err == nil {
			binary = abs
		}
		info, err := os.Stat(binary)
		if err != nil {
			return "", fmt.Errorf("DEVMAP_BINARY names %s: %w", binary, err)
		}
		if info.IsDir() {
			return "", fmt.Errorf("DEVMAP_BINARY names %s, which is a directory", binary)
		}
		if err := devmap.New(binary, root).Probe(ctx); err != nil {
			return "", fmt.Errorf(
				"DEVMAP_BINARY names %s, which did not answer the capability probe: %w "+
					"(an explicit override is used or refused, never replaced by another kernel)",
				binary, err)
		}
		return binary, nil
	}
	for _, candidate := range binaryCandidates(root) {
		if capable(ctx, candidate, root) {
			return candidate, nil
		}
	}
	return "", ErrNoBinary
}

// binaryCandidates excludes repository-owned executables before any probe.
// The exclusion itself lives in proc.LookPathOutside, which every process
// boundary that resolves a program by name shares: a second copy here is
// exactly what drifts when one of them is tightened.
func binaryCandidates(root string) []string {
	candidate, err := proc.LookPathOutside("devmap", root)
	if err != nil {
		return nil
	}
	return []string{candidate}
}

// capable reports whether the binary answers the capability probe.
//
// The probe is the client's own, not a reimplementation: the question "is this
// kernel usable" already has one owner, and a second copy here would drift from
// it exactly when a new capability is added.
func capable(ctx context.Context, binary, root string) bool {
	client := devmap.New(binary, root)
	return client.Probe(ctx) == nil
}

// resolveBinary fills in env.Binary if it is not already set, so one invocation
// probes at most once no matter how many helpers ask for the kernel.
func resolveBinary(ctx context.Context, env *Env) error {
	if env.Binary != "" {
		return nil
	}
	binary, err := discoverBinary(ctx, env.Root)
	if err != nil {
		return err
	}
	env.Binary = binary
	return nil
}

// clientFor resolves a kernel and returns a client bound to it.
func clientFor(ctx context.Context, env *Env) (*devmap.Client, error) {
	if err := resolveBinary(ctx, env); err != nil {
		return nil, err
	}
	return devmap.New(env.Binary, env.Root), nil
}
