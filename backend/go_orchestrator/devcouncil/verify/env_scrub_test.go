//go:build unix

package verify

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
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

// TestRepoEnvAllowRefusesCredentialBearingNames: names whose conventional
// value is a credential (a DSN, a PAT, a URL with embedded userinfo, a webhook
// URL) are refused from the repository's own config even though the name rule
// has no KEY/TOKEN/SECRET in it. Ordinary names still pass.
func TestRepoEnvAllowRefusesCredentialBearingNames(t *testing.T) {
	refused := []string{
		"DATABASE_URL", "GITHUB_PAT", "SENTRY_DSN", "DSN", "PAT", "REDIS_URL", "AMQP_URL",
		"MONGODB_URI", "POSTGRES_URL", "DB_URL", "BROKER_URL", "CACHE_URI", "DB_CONNECTION_STRING",
		"CONNECTION_STRING", "SLACK_WEBHOOK", "SLACK_WEBHOOK_URL", "WEBHOOK_URL", "database_url",
		"CI_JOB_JWT", "JWT",
		"DATABASE_URL_TEST", "DB_URL_RO", "DATABASE_URL2", "REDIS_URI_REPLICA",
		"PGPASS", "DB_PASS", "DB_PWD", "MYSQL_PWD", "PASSWD", "DB_PASSPHRASE", "PASS",
		"NETRC", "GIT_ASKPASS", "SSH_ASKPASS", "HTTP_PROXY", "https_proxy", "GOPROXY",
		"PG_URL", "JDBC_URL", "SPRING_DATASOURCE_URL", "SQLITE_URL", "MARIADB_URL", "MSSQL_URL",
		"SQLSERVER_URL", "COUCHDB_URL", "NEO4J_URI", "ELASTICSEARCH_URL", "OPENSEARCH_URL",
		"CLICKHOUSE_URL", "KAFKA_URL", "RABBITMQ_URL", "NATS_URL", "MEMCACHED_URL", "SMTP_URL",
		"LDAP_URL", "PIP_INDEX_URL", "PIP_EXTRA_INDEX_URL", "UV_INDEX_URL",
		"SENTRYDSN", "SLACKWEBHOOK", "SLACK_HOOK", "DISCORD_HOOK_URL",
	}
	allowed := []string{
		"TEAM_BUILD_FLAVOUR", "NODE_ENV", "RUST_LOG", "GOFLAGS", "CI_URL", "API_URL", "PATH_EXTRA",
		"PATCH_LEVEL", "DB_HOST", "DB_PORT", "DATABASE_NAME", "COMPASS", "REDIS_PORT", "PATTERN",
		"COMPAT_MODE", "OUTPUT_PATH", "VAULT_ADDR", "KUBECONFIG", "PWD", "OLDPWD", "BYPASS_CACHE",
		"GIT_HOOKS_DIR", "INDEX_NAME",
	}
	for _, name := range refused {
		root := t.TempDir()
		writeConfig(t, root, "verify:\n  env_allow: ["+name+"]\n")
		p := LoadEnvPolicy(root)
		if len(p.Extra) != 0 || len(p.Refused) != 1 || p.Refused[0] != name {
			t.Errorf("%s: Extra=%v Refused=%v, want refused", name, p.Extra, p.Refused)
		}
	}
	for _, name := range allowed {
		root := t.TempDir()
		writeConfig(t, root, "verify:\n  env_allow: ["+name+"]\n")
		p := LoadEnvPolicy(root)
		if len(p.Refused) != 0 || len(p.Extra) != 1 || p.Extra[0] != name {
			t.Errorf("%s: Extra=%v Refused=%v, want admitted", name, p.Extra, p.Refused)
		}
	}
}

// TestScrubEnvExtrasAreNotSubjectToTheNameRule: the credential-name rule binds
// only LoadEnvPolicy (the repository's config.yaml). proc.ScrubEnv's extra
// list is not subject to it. There is no user-level env_allow source today:
// the extras come only from EnvPolicy.Environ and a code-owned list
// (components parityEnvExtra), so this pins the seam, not a config path.
func TestScrubEnvExtrasAreNotSubjectToTheNameRule(t *testing.T) {
	environ := []string{"DATABASE_URL=postgres://u:p@h/db", "GITHUB_PAT=x", "SENTRY_DSN=https://k@s/1", "OTHER=1"}
	got := proc.ScrubEnv(environ, "DATABASE_URL", "GITHUB_PAT", "SENTRY_DSN")
	joined := strings.Join(got, "\n")
	for _, name := range []string{"DATABASE_URL", "GITHUB_PAT", "SENTRY_DSN"} {
		if !strings.Contains(joined, name+"=") {
			t.Errorf("ScrubEnv extra %s did not reach the child: %v", name, got)
		}
	}
	if strings.Contains(joined, "OTHER=") {
		t.Error("a name outside the allowlist reached the child")
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
