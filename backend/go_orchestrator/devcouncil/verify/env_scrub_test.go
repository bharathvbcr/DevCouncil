//go:build unix

package verify

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// envOf runs `env` through the runner and returns the child's environment as a
// map. A runner that could not run the command fails the test.
func envOf(t *testing.T, run func(string) CommandOutcome) map[string]string {
	t.Helper()
	out := run("env")
	if out.ExitCode != 0 || out.TimedOut {
		t.Fatalf("env did not run: %+v", out)
	}
	got := map[string]string{}
	for _, line := range strings.Split(out.Stdout, "\n") {
		if k, v, ok := strings.Cut(line, "="); ok {
			got[k] = v
		}
	}
	return got
}

func TestVerifyCommandsDoNotInheritHostSecrets(t *testing.T) {
	t.Setenv("AWS_SECRET_ACCESS_KEY", "planted-aws-secret")
	t.Setenv("GITHUB_TOKEN", "planted-gh-token")
	t.Setenv("ANTHROPIC_API_KEY", "planted-api-key")
	root := t.TempDir()
	for _, name := range []string{"GOCACHE", "CARGO_HOME", "RUSTUP_HOME", "TMPDIR", "LANG"} {
		t.Setenv(name, "/keep/"+name)
	}

	got := envOf(t, DefaultRunCommand(context.Background(), root))

	for _, secret := range []string{"AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN", "ANTHROPIC_API_KEY"} {
		if v, ok := got[secret]; ok {
			t.Errorf("%s leaked into the verify command's environment (%q)", secret, v)
		}
	}
	for _, name := range []string{"PATH", "HOME", "GOCACHE", "CARGO_HOME", "RUSTUP_HOME", "TMPDIR", "LANG"} {
		if got[name] == "" {
			t.Errorf("%s is missing from the verify command's environment", name)
		}
	}
	if got["GOCACHE"] != "/keep/GOCACHE" {
		t.Errorf("GOCACHE = %q, want the host's value", got["GOCACHE"])
	}
}

func TestEnvAllowFromConfigIsHonouredAndRecorded(t *testing.T) {
	t.Setenv("TEAM_BUILD_FLAVOUR", "blue")
	t.Setenv("OTHER_SETTING", "red")
	t.Setenv("DEPLOY_PASSWORD", "planted-password")
	root := t.TempDir()
	writeConfig(t, root, "verify:\n  env_allow: [TEAM_BUILD_FLAVOUR, DEPLOY_PASSWORD]\n")

	policy := LoadEnvPolicy(root)
	if policy.Err != "" {
		t.Fatalf("policy error: %s", policy.Err)
	}
	got := envOf(t, RunCommandWithEnv(context.Background(), root, policy))

	if got["TEAM_BUILD_FLAVOUR"] != "blue" {
		t.Errorf("an allowed name did not reach the child: %q", got["TEAM_BUILD_FLAVOUR"])
	}
	if _, ok := got["OTHER_SETTING"]; ok {
		t.Error("a name outside the allowlist reached the child")
	}
	if _, ok := got["DEPLOY_PASSWORD"]; ok {
		t.Error("a secret-shaped name was admitted by the repository's own config")
	}
	if len(policy.Extra) != 1 || policy.Extra[0] != "TEAM_BUILD_FLAVOUR" {
		t.Errorf("Extra = %v, want [TEAM_BUILD_FLAVOUR]", policy.Extra)
	}
	if len(policy.Refused) != 1 || policy.Refused[0] != "DEPLOY_PASSWORD" {
		t.Errorf("Refused = %v, want [DEPLOY_PASSWORD]", policy.Refused)
	}
}

func TestEnvAllowBlockListFailsLoudNotSilently(t *testing.T) {
	root := t.TempDir()
	writeConfig(t, root, "verify:\n  env_allow:\n    - FOO\n")
	policy := LoadEnvPolicy(root)
	if policy.Err == "" {
		t.Fatal("a block list under verify.env_allow was read as an empty allowlist without an error")
	}
	if len(policy.Extra) != 0 {
		t.Errorf("Extra = %v, want none", policy.Extra)
	}
}

func TestEnvAllowUnreadableConfigNarrowsAndReports(t *testing.T) {
	root := t.TempDir()
	writeConfig(t, root, "verify:\n  env_allow: [: bad\n  : : :\n")
	policy := LoadEnvPolicy(root)
	if policy.Err == "" {
		t.Fatal("an unparsable config produced no policy error")
	}
}

func writeConfig(t *testing.T, root, body string) {
	t.Helper()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestAnInvalidGateConfigBlocksTheRunAndIsNamedInEveryPayload(t *testing.T) {
	const reason = "/p/.devcouncil/config.yaml: permission denied"
	gaps, meta := Run(context.Background(), Input{
		GateMode: "enforce", GateOrigin: "invalid",
		GateConfigPath: "/p/.devcouncil/config.yaml", GateConfigError: reason,
	})
	var found *Gap
	for i := range gaps {
		if gaps[i].GapType == GateConfigInvalid {
			found = &gaps[i]
		}
	}
	if found == nil || !found.Blocking {
		t.Fatalf("no blocking %s gap in %+v", GateConfigInvalid, gaps)
	}
	if !strings.Contains(found.Description, reason) || !strings.Contains(found.Description, "/p/.devcouncil/config.yaml") {
		t.Errorf("gap does not name path and error: %q", found.Description)
	}
	mcp := ToMCP("T1", gaps, meta)
	if mcp.GateModeOrigin != "invalid" || mcp.GateModeError != reason || mcp.Status != "blocked" || mcp.Passed {
		t.Errorf("verify payload = origin %q error %q status %q passed %v", mcp.GateModeOrigin, mcp.GateModeError, mcp.Status, mcp.Passed)
	}
	cli := ToCLITask("T1", gaps, meta)
	if cli.GateModeOrigin != "invalid" || cli.GateModeError != reason {
		t.Errorf("cli payload = origin %q error %q", cli.GateModeOrigin, cli.GateModeError)
	}
}

func TestAHealthyGateConfigAddsNoGateConfigFields(t *testing.T) {
	gaps, meta := Run(context.Background(), Input{GateMode: "enforce", GateOrigin: "config"})
	for _, g := range gaps {
		if g.GapType == GateConfigInvalid {
			t.Fatalf("unexpected %s gap: %+v", GateConfigInvalid, g)
		}
	}
	if mcp := ToMCP("T1", gaps, meta); mcp.GateModeOrigin != "" || mcp.GateModeError != "" {
		t.Errorf("healthy config leaked fields: %+v", mcp)
	}
}

func TestAnUnreadableEnvAllowIsABlockingGap(t *testing.T) {
	gaps, _ := Run(context.Background(), Input{GateMode: "enforce", EnvPolicy: EnvPolicy{Err: "block list"}})
	for _, g := range gaps {
		if g.GapType == VerifyEnvConfigInvalid && g.Blocking {
			return
		}
	}
	t.Fatalf("no blocking %s gap in %+v", VerifyEnvConfigInvalid, gaps)
}
