package mapcli

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// discoverBinary resolves the devmap kernel, in the order the Python seam uses
// (`devmap_engine.find_engine_binary`): DEVMAP_BINARY, then builds under the
// repository's rust-port target directories, then PATH.
//
// PATH is deliberately *last*. On a developer machine PATH resolves
// ~/.cargo/bin/devmap — whatever was last `cargo install`ed — which is
// independent of the working tree and routinely months old. Preferring a local
// build means a kernel change is exercised by the very next command instead of
// silently measuring a stale install.
//
// "Newest" is by modification time, but newest alone is not enough: a build can
// be new and still lack a capability this CLI needs. Each candidate is probed,
// and the first that answers correctly wins, so a fresh but incapable build
// falls through to an older capable one rather than failing the invocation.
func discoverBinary(ctx context.Context, root string) (string, error) {
	for _, candidate := range binaryCandidates(root) {
		if capable(ctx, candidate, root) {
			return candidate, nil
		}
	}
	return "", ErrNoBinary
}

// binaryCandidates lists possible kernels, most-preferred first.
func binaryCandidates(root string) []string {
	var out []string
	seen := map[string]bool{}
	add := func(p string) {
		if p == "" || seen[p] {
			return
		}
		if info, err := os.Stat(p); err != nil || info.IsDir() {
			return
		}
		seen[p] = true
		out = append(out, p)
	}

	// An explicit override beats everything, including capability: an operator
	// naming a binary is entitled to have that binary used, and a silent
	// substitution would make a deliberate test of a specific build a lie.
	if env := os.Getenv("DEVMAP_BINARY"); env != "" {
		if abs, err := filepath.Abs(env); err == nil {
			add(abs)
		} else {
			add(env)
		}
	}

	for _, built := range localBuilds(root) {
		add(built)
	}

	if p, err := exec.LookPath("devmap"); err == nil {
		add(p)
	}
	return out
}

// localBuilds returns devmap binaries under the repository's rust-port target
// directories, newest first.
//
// Both `target` and `target-<lane>` are searched: concurrent fix lanes in this
// repository each build into their own CARGO_TARGET_DIR to avoid serializing on
// one lock, so the lane directories are where a current build usually is.
func localBuilds(root string) []string {
	if root == "" {
		return nil
	}
	type found struct {
		path string
		mod  time.Time
	}
	var hits []found

	targetParents, err := filepath.Glob(filepath.Join(root, "rust-port", "target*"))
	if err != nil {
		return nil
	}
	for _, parent := range targetParents {
		// release before debug only as a tie-break; mtime decides overall,
		// because a debug build made after a release build is the newer code.
		for _, profile := range []string{"release", "debug"} {
			candidate := filepath.Join(parent, profile, "devmap")
			info, err := os.Stat(candidate)
			if err != nil || info.IsDir() {
				continue
			}
			hits = append(hits, found{path: candidate, mod: info.ModTime()})
		}
	}
	sort.SliceStable(hits, func(i, j int) bool { return hits[i].mod.After(hits[j].mod) })

	paths := make([]string, 0, len(hits))
	for _, h := range hits {
		paths = append(paths, h.path)
	}
	return paths
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
