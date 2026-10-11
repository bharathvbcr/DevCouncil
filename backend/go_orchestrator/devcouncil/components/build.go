package components

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// Prober runs a freshly built binary and returns what it printed, stdout and
// stderr together. It is the seam the health checks go through; tests
// replace it.
type Prober interface {
	Probe(bin string, args []string) ([]byte, error)
}

// ExecProber runs the binary, bounded.
type ExecProber struct{}

func (ExecProber) Probe(bin string, args []string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(console.Context(), time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, bin, args...)
	proc.ConfigureGroup(cmd)
	var out proc.CappedBuffer
	out.Limit = 1 << 20
	cmd.Stdout = &out
	cmd.Stderr = &out
	cmd.WaitDelay = 2 * time.Second
	runErr, timedOut := proc.RunBoundedWithCleanup(ctx, cmd.Run)
	if timedOut {
		return out.Bytes(), fmt.Errorf("timed out after 1m")
	}
	if runErr == nil && out.Overflowed() {
		// A capped prefix is not the whole answer; never judge health on one.
		return out.Bytes(), fmt.Errorf("printed more than %d bytes", out.Limit)
	}
	return out.Bytes(), runErr
}

// profile is the cargo profile checkout builds use: release unless PROFILE
// (or Options.Profile) says debug. Release is the default deliberately: a
// debug build of the verifier is slow enough to change how a turn feels, and
// a harness that prefers an installed component over a local build will pick
// this one up for every run.
func (o Options) profile() (string, error) {
	p := strings.TrimSpace(o.Profile)
	if p == "" {
		p = strings.TrimSpace(os.Getenv("PROFILE"))
	}
	switch p {
	case "", "release":
		return "release", nil
	case "debug":
		return "debug", nil
	}
	return "", fmt.Errorf("PROFILE must be 'release' or 'debug', got %q", p)
}

func cargoBuildArgs(c Component, profile string) []string {
	args := []string{"build"}
	if profile == "release" {
		args = append(args, "--release")
	}
	return append(args, "-p", c.Package, "--bin", c.Binary)
}

// cargoTargetDir is where cargo puts a build run in workspace: the
// workspace's own target/ unless CARGO_TARGET_DIR moves it. Lanes build into
// their own target directory, and an installer that ignored that would
// install whatever stale binary sat in rust/target instead.
func cargoTargetDir(workspace string) string {
	if v := strings.TrimSpace(os.Getenv("CARGO_TARGET_DIR")); v != "" {
		if filepath.IsAbs(v) {
			return v
		}
		return filepath.Join(workspace, v)
	}
	return filepath.Join(workspace, "target")
}

// rustComponents maps rust IDs to catalog entries, in the order given.
func rustComponents(ids []string) ([]Component, error) {
	cs := make([]Component, 0, len(ids))
	for _, id := range ids {
		c, ok := Lookup(id)
		if !ok || c.Kind != KindRust {
			return nil, fmt.Errorf("internal: %q is not a rust component", id)
		}
		cs = append(cs, c)
	}
	return cs, nil
}

// installRustFromCheckout builds each component from the checkout's rust/
// workspace, health-checks every build, and only then installs any of them.
//
// An install that replaces a working component with one that cannot answer
// is worse than no install: the harness would degrade every gate that
// component backs, and the operator would have no reason to suspect the
// binary they just installed. So the check runs first, against the build
// output, and a failure stops the run with the old binaries still in place.
func installRustFromCheckout(src, prefix string, ids []string, o Options) ([]string, error) {
	profile, err := o.profile()
	if err != nil {
		return nil, err
	}
	cs, err := rustComponents(ids)
	if err != nil {
		return nil, err
	}
	workspace := filepath.Join(src, "rust")
	run := o.runner()
	for _, c := range cs {
		console.Errorf("building %s (package %s, %s)\n", c.ID, c.Package, profile)
		if err := run.Run("cargo", cargoBuildArgs(c, profile), workspace, nil); err != nil {
			return nil, fmt.Errorf("build %s: %w", c.ID, err)
		}
	}
	builtDir := filepath.Join(cargoTargetDir(workspace), profile)
	// Check a private copy and install that copy. The target directory is
	// shared: a second cargo build landing between a check of target/ and a
	// copy out of it would install bytes nobody checked.
	stage, err := os.MkdirTemp("", "devcouncil-stage-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(stage)
	for _, c := range cs {
		built := filepath.Join(builtDir, rustBinaryName(c.Binary))
		if _, err := os.Stat(built); err != nil {
			return nil, fmt.Errorf("expected %s to exist after the build: %w", built, err)
		}
		staged := filepath.Join(stage, rustBinaryName(c.Binary))
		if err := installAtomic(built, staged); err != nil {
			return nil, fmt.Errorf("stage %s: %w", c.ID, err)
		}
		if err := healthCheck(c.ID, staged, o.prober()); err != nil {
			return nil, err
		}
		console.Errorf("  %s: healthy\n", c.ID)
	}
	bindir := filepath.Join(prefix, "bin")
	installed := make([]string, 0, len(cs))
	for _, c := range cs {
		dest := filepath.Join(bindir, rustBinaryName(c.Binary))
		if err := installAtomic(filepath.Join(stage, rustBinaryName(c.Binary)), dest); err != nil {
			return installed, fmt.Errorf("install %s: %w", c.ID, err)
		}
		console.Errorf("installed %s -> %s\n", c.ID, dest)
		installed = append(installed, c.ID)
	}
	return installed, nil
}

// healthCheck asks a built binary the questions a harness will ask it.
func healthCheck(id, bin string, p Prober) error {
	scratch, err := os.MkdirTemp("", "devcouncil-health-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(scratch)
	ask := func(args ...string) (string, error) {
		out, err := p.Probe(bin, args)
		if err != nil {
			return "", fmt.Errorf("%s %s failed: %w: %s", id, strings.Join(args, " "), err, bytes.TrimSpace(out))
		}
		return string(out), nil
	}
	reports := func(reply string, wants ...string) bool {
		for _, w := range wants {
			if !strings.Contains(reply, w) {
				return false
			}
		}
		return true
	}
	versionReply := func(args ...string) error {
		reply, err := ask(args...)
		if err != nil {
			return err
		}
		if !strings.Contains(reply, `"id":"`+id+`"`) && !strings.Contains(reply, `"version":`) {
			return fmt.Errorf("%s %s did not report id and version: %s", id, strings.Join(args, " "), reply)
		}
		return nil
	}
	switch id {
	case "dcstore":
		if err := versionReply("--version"); err != nil {
			return err
		}
		db := filepath.Join(scratch, "state.sqlite")
		if _, err := ask("--db", db, "ready"); err != nil {
			return err
		}
		reply, err := ask("--db", db, "health")
		if err != nil {
			return err
		}
		// The exclusion index is the lease's mutual exclusion. A store that
		// will not assert it is a store two builders can both win a task
		// from, and an older build omits the key rather than answering false.
		if !reports(reply, `"exclusion_index":"verified"`) {
			return fmt.Errorf("dcstore health did not verify the exclusion index: %s", reply)
		}
		if !reports(reply, `"id":"dcstore"`) {
			return fmt.Errorf("dcstore health did not report id: %s", reply)
		}
	case "dcverify":
		if err := versionReply("--version"); err != nil {
			return err
		}
		reply, err := ask("health")
		if err != nil {
			return err
		}
		if !reports(reply, `"verifier":"dc-verify"`) {
			return fmt.Errorf("dcverify did not identify itself: %s", reply)
		}
		if !reports(reply, `"id":"dcverify"`) {
			return fmt.Errorf("dcverify health did not report id: %s", reply)
		}
	case "dcgrep":
		if err := versionReply("--version"); err != nil {
			return err
		}
		reply, err := ask("health")
		if err != nil {
			return err
		}
		if !reports(reply, `"ok":true`) {
			return fmt.Errorf("dcgrep health failed: %s", reply)
		}
		if !reports(reply, `"id":"dcgrep"`) {
			return fmt.Errorf("dcgrep health did not report id: %s", reply)
		}
	case "devmap":
		if _, err := ask("--version"); err != nil {
			return err
		}
		if _, err := ask("version"); err != nil {
			return err
		}
		if err := versionReply("version", "--json"); err != nil {
			return err
		}
	default:
		return fmt.Errorf("no health check for %s", id)
	}
	return nil
}

// installAtomic copies src beside dest, then renames it over dest.
//
// Copying onto a path a process is currently executing can fail outright,
// and on the platforms where it does not, it rewrites the file underneath
// that process. A rename within one filesystem swaps the directory entry, so
// anything already running keeps the inode it started with and the next
// exec gets the new binary whole. safefile.WriteAtomic is the repository's one
// owner of that discipline: a temporary name of its own (two installers
// racing on one destination each rename a whole binary rather than one
// stealing the other's half-written copy), synced before the rename.
func installAtomic(src, dest string) error {
	data, err := os.ReadFile(src)
	if err != nil {
		return err
	}
	return safefile.WriteAtomic(dest, data, 0o755)
}

// resolutionNotes says, for each installed binary, whether PATH finds the
// copy just installed. An older copy earlier in PATH shadows the install,
// and every symptom afterwards looks like the new build is broken, so this
// names it.
func resolutionNotes(bindir string, names []string, lookPath func(string) (string, error)) []string {
	var notes []string
	for _, name := range names {
		want := filepath.Join(bindir, name)
		found, err := lookPath(name)
		switch {
		case err != nil:
			notes = append(notes, fmt.Sprintf("note: %s is not on PATH — add %s to PATH", name, bindir))
		case !samePath(found, want):
			notes = append(notes, fmt.Sprintf("warning: %s resolves to %s, not %s — an earlier PATH entry shadows this install", name, found, want))
		}
	}
	return notes
}

// engineCgoEnv runs the checkout's rust/gusset-engine/cgo-env.sh — the one
// owner of building the engine and of the flags that link it — and returns
// the KEY=VALUE lines it prints. Its progress and errors go to stderr.
func engineCgoEnv(src string) ([]string, error) {
	script := filepath.Join(src, "rust", "gusset-engine", "cgo-env.sh")
	ctx, cancel := context.WithTimeout(console.Context(), 20*time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, "bash", script)
	proc.ConfigureGroup(cmd)
	cmd.Dir = src
	cmd.Stderr = console.Stderr()
	cmd.WaitDelay = 2 * time.Second
	var out proc.CappedBuffer
	out.Limit = 64 << 10
	cmd.Stdout = &out
	runErr, timedOut := proc.RunBoundedWithCleanup(ctx, cmd.Run)
	switch {
	case timedOut:
		return nil, fmt.Errorf("%s timed out after 20m", script)
	case runErr != nil:
		return nil, fmt.Errorf("%s: %w", script, runErr)
	case out.Overflowed():
		return nil, fmt.Errorf("%s printed more than %d bytes", script, out.Limit)
	}
	return parseCgoEnv(out.String())
}

// parseCgoEnv reads cgo-env.sh's KEY=VALUE lines. Anything else is refused:
// a line this cannot read is a flag the link would silently go without.
func parseCgoEnv(text string) ([]string, error) {
	var env []string
	for _, line := range strings.Split(strings.TrimSpace(text), "\n") {
		key, _, ok := strings.Cut(line, "=")
		if !ok || key == "" || strings.ContainsAny(key, " \t") {
			return nil, fmt.Errorf("cgo-env.sh printed %q, not KEY=VALUE", line)
		}
		env = append(env, line)
	}
	if !slices.ContainsFunc(env, func(kv string) bool { return strings.HasPrefix(kv, "CGO_LDFLAGS=") }) {
		return nil, fmt.Errorf("cgo-env.sh printed no CGO_LDFLAGS")
	}
	return env, nil
}

// samePath reports whether two paths name one file. A prefix reached through
// a symlink (macOS's /tmp is /private/tmp) is the directory PATH names by its
// real path, and reporting that as shadowing sends the operator after a
// problem that does not exist. Paths that cannot be examined compare as text.
func samePath(a, b string) bool {
	if filepath.Clean(a) == filepath.Clean(b) {
		return true
	}
	ai, aerr := os.Stat(a)
	bi, berr := os.Stat(b)
	return aerr == nil && berr == nil && os.SameFile(ai, bi)
}

// signForMacOS ad-hoc signs a freshly built host. A missing or failing
// codesign is a note, not an install failure: the linker's own signature
// still lets the binary run.
func signForMacOS(bin string, run Runner) {
	if runtime.GOOS != "darwin" {
		return
	}
	if _, err := exec.LookPath("codesign"); err != nil {
		console.Errorf("note: codesign not found; %s is unsigned\n", bin)
		return
	}
	if err := run.Run("codesign", []string{"--force", "--sign", "-", bin}, "", nil); err != nil {
		console.Errorf("note: ad-hoc codesign failed for %s: %v\n", bin, err)
	}
}

// localModelHint is the Ollama guidance printed after a macOS host install.
// Local model size is bounded by unified memory, so it recommends a size
// that will fit.
func localModelHint(ramBytes uint64, arm64, haveOllama bool) string {
	gb := ramBytes / 1024 / 1024 / 1024
	model := "qwen2.5-coder:7b"
	switch {
	case gb >= 48:
		model = "qwen2.5-coder:32b"
	case gb >= 24:
		model = "qwen2.5-coder:14b"
	}
	chip := "Intel"
	if arm64 {
		chip = "Apple Silicon"
	}
	var b strings.Builder
	fmt.Fprintf(&b, "macOS (%s, %d GB) detected. To run a local model with Ollama:\n", chip, gb)
	if !haveOllama {
		b.WriteString("  1. Install Ollama:        brew install ollama   (then: ollama serve)\n")
	}
	fmt.Fprintf(&b, "  2. Pull a model:          ollama pull %s\n", model)
	b.WriteString("  3. Raise the context:     export OLLAMA_NUM_CTX=16384\n")
	return b.String()
}

func printLocalModelHint() {
	if runtime.GOOS != "darwin" {
		return
	}
	ctx, cancel := context.WithTimeout(console.Context(), 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "sysctl", "-n", "hw.memsize")
	proc.ConfigureGroup(cmd)
	cmd.WaitDelay = time.Second
	var out bytes.Buffer
	cmd.Stdout = &out
	if err, timedOut := proc.RunBounded(ctx, cmd.Run); err != nil || timedOut {
		return
	}
	ram, err := strconv.ParseUint(strings.TrimSpace(out.String()), 10, 64)
	if err != nil {
		return
	}
	_, lookErr := exec.LookPath("ollama")
	console.Errorln()
	console.Errorf("%s", localModelHint(ram, runtime.GOARCH == "arm64", lookErr == nil))
}
