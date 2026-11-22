package main

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/gate"
)

func writeRepoConfigAt(t *testing.T, root, doc string) string {
	t.Helper()
	cfg := filepath.Join(root, ".devcouncil", "config.yaml")
	if err := os.WriteFile(cfg, []byte(doc), 0o644); err != nil {
		t.Fatal(err)
	}
	// A rewrite inside one mtime tick of the last must still be seen; the
	// documents below also differ in size, which is the other half of the key.
	bump := time.Now().Add(time.Duration(len(doc)) * time.Second)
	if err := os.Chtimes(cfg, bump, bump); err != nil {
		t.Fatal(err)
	}
	return cfg
}

func policyCheck(t *testing.T, reg *devcouncil.Registry, path string) any {
	t.Helper()
	out, err := reg.Call(context.Background(), "devcouncil_policy_check_write", map[string]any{"path": path})
	if err != nil {
		t.Fatalf("policy_check_write %s: %v", path, err)
	}
	return out
}

// allowed returns the verdict of a policy_check_write answer. A refusal to
// answer (not_initialized) is not a verdict, so it is reported separately.
func allowed(t *testing.T, out any) (verdict bool, answered bool) {
	t.Helper()
	m, ok := out.(map[string]any)
	if !ok {
		return false, false
	}
	v, _ := m["allowed"].(bool)
	return v, m["ok"] == true
}

// The MCP host says, on stderr and when it builds its gate, every Safety flag
// an operator has moved off its safest value, and says when a repository config
// stopped the gate being built. Weakened() used to be read only by a report the
// MCP host never printed, so an env override was invisible.
func TestMCPGateAnnouncesWeakenedFlagsAndRefusals(t *testing.T) {
	t.Run("operator env override is named with its origin", func(t *testing.T) {
		t.Setenv(flags.EnvKey(flags.PolicyHardRules), "false")
		var out bytes.Buffer
		g, err := openAnnouncedGate(gateRepo(t, nil), &out, "devcouncil mcp")
		if err != nil || g == nil {
			t.Fatalf("an operator env override must still build a gate: %v", err)
		}
		for _, want := range []string{"weakened safety flags", flags.PolicyHardRules, "(" + string(flags.OriginEnv) + ")"} {
			if !strings.Contains(out.String(), want) {
				t.Errorf("startup notice %q does not contain %q", out.String(), want)
			}
		}
	})
	t.Run("a strict gate says nothing", func(t *testing.T) {
		var out bytes.Buffer
		if _, err := openAnnouncedGate(gateRepo(t, nil), &out, "devcouncil mcp"); err != nil {
			t.Fatal(err)
		}
		if out.Len() != 0 {
			t.Errorf("a fully strict gate printed %q", out.String())
		}
	})
	t.Run("a repository config that loosens the gate is refused on stderr", func(t *testing.T) {
		root := gateRepo(t, nil)
		cfg := writeRepoConfigAt(t, root, "policy.hard_rules.enabled: false\n")
		var out bytes.Buffer
		g, err := openAnnouncedGate(root, &out, "devcouncil mcp")
		if err == nil || g != nil {
			t.Fatalf("a loosening repository config built a gate (err %v)", err)
		}
		for _, want := range []string{"write gate unavailable", cfg, flags.PolicyHardRules} {
			if !strings.Contains(out.String(), want) {
				t.Errorf("refusal notice %q does not contain %q", out.String(), want)
			}
		}
	})
}

// A running MCP host follows config.yaml: an edit reaches the next
// policy_check_write without a restart, and a config that cannot be honoured
// (one that loosens a Safety flag, or does not parse) fails closed — the tool
// refuses to answer, it never answers as if the gate were off — and a fix
// recovers on the same server. The rebuild is keyed on gatescfg.ConfigKey, the
// key gates.mode is already re-read on.
func TestMCPPolicyCheckFollowsConfigEditsAndFailsClosed(t *testing.T) {
	root := gateRepo(t, nil)
	writeRepoConfigAt(t, root, "harness:\n  posture: dev\n")
	var stderr bytes.Buffer
	reg := devcouncil.NewRegistry(root, nil, nil)
	reg.OpenGate = func() (*gate.Gate, error) { return openAnnouncedGate(root, &stderr, "devcouncil mcp") }

	const unplanned = "src/new_file.go"
	// dev demotes the scope refusal of an unplanned write to an allow.
	if ok, answered := allowed(t, policyCheck(t, reg, unplanned)); !answered || !ok {
		t.Fatalf("under the dev posture an unplanned write should be allowed (answered=%v allowed=%v)", answered, ok)
	}
	// The posture is tightened by editing the file; no restart.
	writeRepoConfigAt(t, root, "harness:\n  posture: strict\n  # tightened\n")
	if ok, answered := allowed(t, policyCheck(t, reg, unplanned)); !answered || ok {
		t.Fatalf("after posture: strict the next check must refuse the unplanned write (answered=%v allowed=%v)", answered, ok)
	}

	failClosed := func(label, doc, wantInErr string) {
		t.Helper()
		writeRepoConfigAt(t, root, doc)
		out := policyCheck(t, reg, ".env")
		if ok, answered := allowed(t, out); answered || ok {
			t.Fatalf("%s: got %+v, want no verdict (fail closed)", label, out)
		}
		p, isErr := out.(devcouncil.ErrorPayload)
		if !isErr || p.Code != "not_initialized" || !strings.Contains(p.Error, wantInErr) {
			t.Fatalf("%s: payload %+v, want not_initialized naming %q", label, out, wantInErr)
		}
	}
	failClosed("loosening config", "harness:\n  posture: yolo\n  # loosen\n", flags.HarnessPosture)
	// Repairing the file recovers on the same server.
	writeRepoConfigAt(t, root, "harness:\n  posture: strict\n")
	if ok, answered := allowed(t, policyCheck(t, reg, unplanned)); !answered || ok {
		t.Fatalf("after the fix the gate must answer again and refuse (answered=%v allowed=%v)", answered, ok)
	}
	failClosed("unparseable config", "harness: [unterminated\n  posture: strict\n", "config.yaml")
	writeRepoConfigAt(t, root, "harness:\n  posture: strict\npolicy.hard_rules.enabled: true\n")
	if ok, answered := allowed(t, policyCheck(t, reg, ".env")); !answered || ok {
		t.Fatalf("after the second fix .env must be refused by a running gate (answered=%v allowed=%v)", answered, ok)
	}
	if !strings.Contains(stderr.String(), "write gate unavailable") {
		t.Errorf("rebuild failures were not reported on stderr: %q", stderr.String())
	}
}

// An edit that leaves the stat key alone must not rebuild: the host does not
// parse the file on every tool call.
func TestMCPPolicyCheckDoesNotRebuildAnUnchangedConfig(t *testing.T) {
	root := gateRepo(t, nil)
	builds := 0
	reg := devcouncil.NewRegistry(root, nil, nil)
	reg.OpenGate = func() (*gate.Gate, error) { builds++; return openGate(root) }
	for range 3 {
		policyCheck(t, reg, "src/a.go")
	}
	if builds != 1 {
		t.Fatalf("an unchanged config rebuilt the gate %d times, want 1", builds)
	}
}

// runMCP serves newMCPRegistry's registry, which builds the gate before the
// first request: the weakened-flag line is on stderr at startup.
func TestMCPStartupBuildsTheGateAndAnnouncesIt(t *testing.T) {
	t.Setenv(flags.EnvKey(flags.PolicyHardRules), "false")
	var out bytes.Buffer
	newMCPRegistry(gateRepo(t, nil), nil, &out)
	if !strings.Contains(out.String(), "devcouncil mcp: write gate running with weakened safety flags") ||
		!strings.Contains(out.String(), flags.PolicyHardRules) {
		t.Fatalf("nothing about the weakened flag was printed at startup: %q", out.String())
	}
}

// The dev tool path builds the same announced gate.
func TestToolPathAnnouncesWeakenedFlagsToo(t *testing.T) {
	t.Setenv(flags.EnvKey(flags.PolicyHardRules), "false")
	var out bytes.Buffer
	if _, err := openAnnouncedGate(gateRepo(t, nil), &out, "devcouncil tool"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "devcouncil tool: write gate running with weakened") {
		t.Fatalf("tool path notice = %q", out.String())
	}
}

// Every transition between absent, a regular file, a live symlink and a
// dangling symlink must rebuild the gate on a running host, and each state must
// answer as a fresh registry would. The stat key used to give "absent" and
// "stat failed" the same value and followed symlinks, so a dangling link kept
// the old default gate answering allowed=true.
func TestMCPGateFollowsConfigFileTypeTransitions(t *testing.T) {
	root := gateRepo(t, nil)
	cfg := filepath.Join(root, ".devcouncil", "config.yaml")
	target := filepath.Join(t.TempDir(), "real.yaml")
	if err := os.WriteFile(target, []byte("harness:\n  posture: strict\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	set := map[string]func(){
		"absent":   func() { _ = os.Remove(cfg) },
		"regular":  func() { _ = os.Remove(cfg); writeRepoConfigAt(t, root, "harness:\n  posture: strict\n") },
		"dangling": func() { _ = os.Remove(cfg); _ = os.Symlink(filepath.Join(t.TempDir(), "gone.yaml"), cfg) },
		"live":     func() { _ = os.Remove(cfg); _ = os.Symlink(target, cfg) },
	}
	fresh := func() (any, any) {
		r := devcouncil.NewRegistry(root, nil, nil)
		r.OpenGate = func() (*gate.Gate, error) { return openGate(root) }
		return policyCheck(t, r, "src/new_file.go"), nil
	}
	reg := devcouncil.NewRegistry(root, nil, nil)
	reg.OpenGate = func() (*gate.Gate, error) { return openGate(root) }
	states := []string{"absent", "regular", "dangling", "absent", "dangling", "regular", "live", "dangling", "live", "regular", "absent", "live", "absent"}
	for i, s := range states {
		set[s]()
		got := policyCheck(t, reg, "src/new_file.go")
		want, _ := fresh()
		gv, ga := allowed(t, got)
		wv, wa := allowed(t, want)
		if gv != wv || ga != wa {
			t.Fatalf("step %d -> %s: running host answered (allowed=%v answered=%v), a fresh registry (allowed=%v answered=%v)", i, s, gv, ga, wv, wa)
		}
	}
}

// A named pipe at config.yaml is refused without opening it. It used to be
// opened by the retired-key pre-read, which blocks until a writer appears, with
// the registry's gate lock held.
func TestMCPPolicyCheckRefusesAConfigFIFOWithoutBlocking(t *testing.T) {
	root := gateRepo(t, nil)
	cfg := filepath.Join(root, ".devcouncil", "config.yaml")
	if err := os.Remove(cfg); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(cfg, 0o644); err != nil {
		t.Skipf("no FIFOs here: %v", err)
	}
	reg := devcouncil.NewRegistry(root, nil, nil)
	reg.OpenGate = func() (*gate.Gate, error) { return openGate(root) }
	done := make(chan any, 1)
	go func() { done <- policyCheck(t, reg, ".env") }()
	select {
	case out := <-done:
		if ok, answered := allowed(t, out); answered || ok {
			t.Fatalf("a FIFO config produced a verdict: %+v", out)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("policy_check_write blocked on a FIFO at config.yaml")
	}
}
