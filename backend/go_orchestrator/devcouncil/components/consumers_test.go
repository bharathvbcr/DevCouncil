package components

import (
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// needNode returns node's path. The fixtures below are node scripts because
// the check they stand in for is one; without node they cannot run, and the
// skip says so rather than passing.
func needNode(t *testing.T) string {
	t.Helper()
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("node is not on PATH; the schema-parity fixtures are node scripts")
	}
	return node
}

// onlyNode resolves node and nothing else, so the post-install PATH notes
// stay out of the way of what these tests assert.
func onlyNode(name string) (string, error) {
	if name == "node" {
		return exec.LookPath("node")
	}
	return "", errors.New("not found")
}

// siblings makes <tmp>/DevCouncil (a checkout) and, when script is not empty,
// <tmp>/GitPulse/scripts/check-schema-parity.mjs holding it.
func siblings(t *testing.T, script string) (src, gitpulse string) {
	t.Helper()
	parent := t.TempDir()
	src = filepath.Join(parent, "DevCouncil")
	if err := osMkdirCheckout(src); err != nil {
		t.Fatal(err)
	}
	gitpulse = filepath.Join(parent, "GitPulse")
	if script == "" {
		return src, gitpulse
	}
	writeParityScript(t, gitpulse, script)
	return src, gitpulse
}

func writeParityScript(t *testing.T, root, script string) {
	t.Helper()
	path := filepath.Join(root, filepath.FromSlash(parityScript))
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(script), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestConsumersAreNotCheckedWithoutAGitPulseCheckout(t *testing.T) {
	t.Setenv(GitPulseRootEnv, "")
	src, gitpulse := siblings(t, "")
	r := consumerCheck{src: src, lookPath: onlyNode}.run()
	if r.Checked || r.Passed() {
		t.Fatalf("a check that never ran reported %+v", r)
	}
	if !strings.Contains(r.Reason, gitpulse) || !strings.Contains(r.Reason, GitPulseRootEnv) {
		t.Fatalf("reason does not say where it looked or how to point it: %q", r.Reason)
	}
}

func TestConsumersAreNotCheckedFromAGitPulseWithoutTheCheck(t *testing.T) {
	t.Setenv(GitPulseRootEnv, "")
	src, gitpulse := siblings(t, "")
	if err := os.MkdirAll(gitpulse, 0o755); err != nil {
		t.Fatal(err)
	}
	r := consumerCheck{src: src, lookPath: onlyNode}.run()
	if r.Checked || !strings.Contains(r.Reason, "older than the parity check") {
		t.Fatalf("report = %+v", r)
	}
}

// An explicit choice that does not hold the check is the answer; it must not
// quietly fall back to a sibling that does.
func TestAnExplicitGitPulseThatLacksTheCheckIsNotReplacedByTheSibling(t *testing.T) {
	src, _ := siblings(t, `console.log("{}")`)
	elsewhere := t.TempDir()
	for _, tc := range []struct{ flag, env, want string }{
		{flag: elsewhere, want: "--gitpulse=" + elsewhere},
		{env: elsewhere, want: GitPulseRootEnv + "=" + elsewhere},
	} {
		t.Setenv(GitPulseRootEnv, tc.env)
		r := consumerCheck{src: src, gitpulse: tc.flag, lookPath: onlyNode}.run()
		if r.Checked || !strings.Contains(r.Reason, tc.want) {
			t.Fatalf("flag=%q env=%q: report = %+v", tc.flag, tc.env, r)
		}
	}
}

func TestConsumersAreNotCheckedWithoutNode(t *testing.T) {
	t.Setenv(GitPulseRootEnv, "")
	src, _ := siblings(t, `console.log("{}")`)
	r := consumerCheck{src: src, lookPath: noPath}.run()
	if r.Checked || !strings.Contains(r.Reason, "node is not on PATH") {
		t.Fatalf("report = %+v", r)
	}
}

func TestConsumersAreNotCheckedWithoutAnOwnerCheckout(t *testing.T) {
	r := consumerCheck{src: "", lookPath: onlyNode}.run()
	if r.Checked || !strings.Contains(r.Reason, "no DevCouncil checkout") {
		t.Fatalf("report = %+v", r)
	}
}

func TestACheckThatHangsIsNotCheckedAndIsKilled(t *testing.T) {
	needNode(t)
	t.Setenv(GitPulseRootEnv, "")
	src, _ := siblings(t, `setTimeout(() => {}, 60000)`)
	start := time.Now()
	r := consumerCheck{src: src, lookPath: onlyNode, timeout: 500 * time.Millisecond}.run()
	if r.Checked || r.Passed() || !strings.Contains(r.Reason, "did not finish within 500ms") {
		t.Fatalf("report = %+v", r)
	}
	if elapsed := time.Since(start); elapsed > 10*time.Second {
		t.Fatalf("a 500ms bound took %s", elapsed)
	}
}

func TestACheckThatPrintsTooMuchIsNotChecked(t *testing.T) {
	needNode(t)
	t.Setenv(GitPulseRootEnv, "")
	src, _ := siblings(t, `process.stdout.write("x".repeat(4096))`)
	r := consumerCheck{src: src, lookPath: onlyNode, limit: 1024}.run()
	if r.Checked || !strings.Contains(r.Reason, "more than 1024 bytes") {
		t.Fatalf("report = %+v", r)
	}
}

// staleScript prints what check-schema-parity.mjs --json prints when the
// installed gitpulse-mcp lags, and exits 1 as it does. The devcouncil field
// echoes GITPULSE_DEVCOUNCIL_ROOT so a test can see the install pointed the
// check at the checkout it installed from.
const staleScript = `const root = process.env.GITPULSE_DEVCOUNCIL_ROOT ?? "";
console.log(JSON.stringify({
  ok: false,
  owner: { store: 27, dcStore: 9, workbench: 13 },
  devcouncil: root,
  rows: [
    { reader: "GitPulse vendored crates", verdict: "match", detail: "devmap store 27, dc-store 9, workbench 13" },
    { reader: "gitpulse-mcp (PATH)", verdict: "stale", detail: "devmap store 26, DevCouncil writes 27" },
    { reader: "Jarvis pin (upstream.lock.json)", verdict: "absent", detail: "no checkout found" },
  ],
  summary: "FAIL: 1 match · 1 stale · 1 absent (not checked)",
}));
process.exitCode = 1;
`

// installWithChecker runs a real `install devmap` from a fake checkout, with
// the cargo build and health check faked and node real.
func installWithChecker(t *testing.T, src string, extra ...string) (code int, stdout, stderr string) {
	t.Helper()
	t.Setenv("CARGO_TARGET_DIR", "")
	t.Setenv("PROFILE", "")
	base := Options{SourceRoot: src, Runner: &cargoRunner{t: t, src: src}, Prober: healthyProber{}, LookPath: onlyNode}
	out, restoreOut := swapStdout(t)
	errOut, restoreErr := swapStderr(t)
	code = runInstall(append([]string{"devmap", "--prefix", t.TempDir()}, extra...), base)
	restoreErr()
	restoreOut()
	return code, out.String(), errOut.String()
}

func TestInstallNamesAStaleConsumerAndItsRemedyButStillSucceeds(t *testing.T) {
	needNode(t)
	t.Setenv(GitPulseRootEnv, "")
	src, gitpulse := siblings(t, staleScript)
	code, _, stderr := installWithChecker(t, src)
	if code != 0 {
		t.Fatalf("a lagging consumer failed the default install: exit %d\n%s", code, stderr)
	}
	for _, want := range []string{
		"FOLLOW-UP: 1 schema consumer(s)",
		"gitpulse-mcp (PATH) (stale): in " + gitpulse + ": bun run mcp:install",
		"stale         gitpulse-mcp (PATH) — devmap store 26, DevCouncil writes 27",
		"absent        Jarvis pin (upstream.lock.json)",
	} {
		if !strings.Contains(stderr, want) {
			t.Fatalf("install output lacks %q:\n%s", want, stderr)
		}
	}
	if strings.Contains(stderr, "NOT CHECKED") {
		t.Fatalf("a check that ran was reported as not run:\n%s", stderr)
	}
}

func TestStrictConsumersFailsTheInstallOnAStaleConsumer(t *testing.T) {
	needNode(t)
	t.Setenv(GitPulseRootEnv, "")
	src, _ := siblings(t, staleScript)
	if code, _, stderr := installWithChecker(t, src, "--strict-consumers"); code != 1 {
		t.Fatalf("exit %d\n%s", code, stderr)
	}
}

// Strict means examined and matching. A consumer check that could not run
// must not let a strict install through.
func TestStrictConsumersFailsTheInstallWhenTheCheckCouldNotRun(t *testing.T) {
	t.Setenv(GitPulseRootEnv, "")
	src, _ := siblings(t, "")
	code, _, stderr := installWithChecker(t, src, "--strict-consumers")
	if code != 1 || !strings.Contains(stderr, "schema consumers: NOT CHECKED") {
		t.Fatalf("exit %d\n%s", code, stderr)
	}
}

func TestStrictConsumersPassesWhenEveryPresentReaderMatches(t *testing.T) {
	needNode(t)
	t.Setenv(GitPulseRootEnv, "")
	src, _ := siblings(t, `console.log(JSON.stringify({ ok: true, owner: { store: 27, dcStore: 9, workbench: 13 }, devcouncil: "x",
  rows: [{ reader: "devmap (PATH)", verdict: "match", detail: "devmap store 27" }], summary: "OK: 1 match" }))`)
	if code, _, stderr := installWithChecker(t, src, "--strict-consumers"); code != 0 {
		t.Fatalf("exit %d\n%s", code, stderr)
	}
}

func TestInstallWithoutGitPulseSaysTheConsumersWereNotChecked(t *testing.T) {
	t.Setenv(GitPulseRootEnv, "")
	src, gitpulse := siblings(t, "")
	code, _, stderr := installWithChecker(t, src)
	if code != 0 {
		t.Fatalf("exit %d\n%s", code, stderr)
	}
	if !strings.Contains(stderr, "schema consumers: NOT CHECKED — no GitPulse checkout at "+gitpulse) {
		t.Fatalf("install output does not say the consumers were not checked:\n%s", stderr)
	}
	for _, pass := range []string{"OK:", "match"} {
		if strings.Contains(stderr, pass) {
			t.Fatalf("an unchecked report contains %q, which reads as a pass:\n%s", pass, stderr)
		}
	}
}

func TestInstallWithAHungCheckSaysNotChecked(t *testing.T) {
	needNode(t)
	t.Setenv(GitPulseRootEnv, "")
	src, _ := siblings(t, `setTimeout(() => {}, 60000)`)
	// The production bound is minutes; drive the same path with a short one.
	r := consumerCheck{src: src, lookPath: onlyNode, timeout: 300 * time.Millisecond}.run()
	out, restore := swapStderr(t)
	PrintConsumerReport(r)
	restore()
	if !strings.Contains(out.String(), "schema consumers: NOT CHECKED — the check did not finish within 300ms") {
		t.Fatalf("report printed:\n%s", out.String())
	}
}

func TestTheCheckIsPointedAtTheInstallingCheckout(t *testing.T) {
	needNode(t)
	t.Setenv(GitPulseRootEnv, "")
	t.Setenv("GITPULSE_DEVCOUNCIL_ROOT", "/somewhere/else")
	src, _ := siblings(t, staleScript)
	code, stdout, stderr := installWithChecker(t, src, "--json")
	if code != 0 {
		t.Fatalf("exit %d\n%s", code, stderr)
	}
	var receipt struct {
		OK        bool `json:"ok"`
		Consumers struct {
			Checked    bool          `json:"checked"`
			OK         bool          `json:"ok"`
			DevCouncil string        `json:"devcouncil"`
			Rows       []ConsumerRow `json:"rows"`
		} `json:"consumers"`
	}
	if err := json.Unmarshal([]byte(stdout), &receipt); err != nil {
		t.Fatalf("--json printed non-JSON: %v\n%s", err, stdout)
	}
	if !receipt.OK || !receipt.Consumers.Checked || receipt.Consumers.OK || len(receipt.Consumers.Rows) != 3 {
		t.Fatalf("receipt = %+v\n%s", receipt, stdout)
	}
	if receipt.Consumers.DevCouncil != src {
		t.Fatalf("the check judged %q, not the installing checkout %q", receipt.Consumers.DevCouncil, src)
	}
}

// envEchoScript answers as the real check does and reports, in its rows, what
// it found in its own environment: whether the planted secrets reached it,
// whether it can still find its tools, and which checkouts it was pointed at.
const envEchoScript = `const env = process.env;
const has = (name) => Object.prototype.hasOwnProperty.call(env, name);
console.log(JSON.stringify({
  ok: true,
  owner: { store: 27, dcStore: 9, workbench: 13 },
  devcouncil: env.GITPULSE_DEVCOUNCIL_ROOT ?? "",
  rows: [
    { reader: "planted", verdict: "match", detail: ["DEVCOUNCIL_PLANTED_SECRET", "GITHUB_TOKEN", "NODE_OPTIONS"].filter(has).join(",") },
    { reader: "path", verdict: "match", detail: has("PATH") ? "present" : "missing" },
    { reader: "manvi", verdict: "match", detail: env.GITPULSE_MANVI_ROOT ?? "" },
    { reader: "argv", verdict: "match", detail: JSON.stringify(process.argv.slice(2)) },
  ],
  summary: "OK",
}));
`

// The check runs code from another repository. It gets the environment it
// needs to find its tools and the checkouts it reads, never the caller's
// credentials; and the checkout it judges is the one this install names, not
// whatever the caller's environment happened to say.
func TestTheCheckDoesNotInheritTheCallersEnvironment(t *testing.T) {
	needNode(t)
	t.Setenv(GitPulseRootEnv, "")
	t.Setenv("DEVCOUNCIL_PLANTED_SECRET", "s3cret-must-not-leak")
	t.Setenv("GITHUB_TOKEN", "ghp_planted")
	// NODE_OPTIONS would let the caller's environment inject code into node.
	t.Setenv("NODE_OPTIONS", "--no-warnings")
	t.Setenv("GITPULSE_DEVCOUNCIL_ROOT", "/somewhere/else")
	manvi := t.TempDir()
	t.Setenv("GITPULSE_MANVI_ROOT", manvi)
	src, _ := siblings(t, envEchoScript)

	r := consumerCheck{src: src, lookPath: onlyNode}.run()
	if !r.Checked || !r.Passed() {
		t.Fatalf("report = %+v", r)
	}
	got := map[string]string{}
	for _, row := range r.Rows {
		got[row.Reader] = row.Detail
	}
	if got["planted"] != "" {
		t.Fatalf("the caller's environment reached the check: %s", got["planted"])
	}
	if got["path"] != "present" {
		t.Fatalf("the check cannot find its tools: PATH %s", got["path"])
	}
	if got["manvi"] != manvi {
		t.Fatalf("the check lost where Manvi lives: %q, want %q", got["manvi"], manvi)
	}
	if r.DevCouncil != src {
		t.Fatalf("the check judged %q, not the installing checkout %q", r.DevCouncil, src)
	}
	if got["argv"] != `["--json"]` {
		t.Fatalf("the check was started with %s", got["argv"])
	}
}

func TestParseParityNeverReadsANonAnswerAsAVerdict(t *testing.T) {
	good := `{"ok":true,"owner":{"store":27},"rows":[{"reader":"devmap (PATH)","verdict":"match","detail":"devmap store 27"}],"summary":"OK: 1 match"}`
	for _, tc := range []struct {
		name   string
		code   int
		stdout string
		want   string
	}{
		{"usage", 2, "", "exited 2 (usage error)"},
		{"crash", 13, good, "exited 13"},
		{"no json", 1, "Error: Cannot find module './usage.mjs'", "without a JSON answer"},
		{"empty", 0, "", "without a JSON answer"},
		{"no ok field", 0, `{"rows":[]}`, "without a JSON answer"},
		{"exit and answer disagree", 1, good, "exited 1 but its answer says ok=true"},
		{"unknown verdict", 0, strings.Replace(good, `"match"`, `"fine"`, 1), `unknown verdict "fine"`},
		{"owner unreadable", 1, `{"ok":false,"error":"CURRENT_SCHEMA_VERSION not found in devmap-store/src/schema.rs","rows":[]}`, "owner unreadable: CURRENT_SCHEMA_VERSION not found"},
	} {
		r := parseParity("/gp", tc.code, []byte(tc.stdout), "")
		if r.Checked || r.Passed() || !strings.Contains(r.Reason, tc.want) {
			t.Fatalf("%s: report = %+v", tc.name, r)
		}
	}
	// A line printed before the answer does not hide it.
	r := parseParity("/gp", 0, []byte("warming up\n"+good+"\n"), "")
	if !r.Checked || !r.Passed() || len(r.Rows) != 1 {
		t.Fatalf("report = %+v", r)
	}
	// ok:false with no lagging row (every reader absent) is not a pass.
	allAbsent := `{"ok":false,"owner":{"store":27},"rows":[{"reader":"devmap (PATH)","verdict":"absent","detail":"not on PATH"}],"summary":"FAIL: 1 absent (not checked)"}`
	r = parseParity("/gp", 1, []byte(allAbsent), "")
	if !r.Checked || r.Passed() || len(r.Lagging()) != 0 {
		t.Fatalf("report = %+v", r)
	}
}

func TestEveryReaderTheCheckNamesHasARemedy(t *testing.T) {
	for _, reader := range []string{
		"GitPulse vendored crates", "devmap (PATH)", "dcstore (PATH)", "gitpulse-mcp (PATH)",
		"GitPulse.app (its bundled gitpulse-mcp)", "Manvi pin (scripts/module-pins.txt)", "Jarvis pin (upstream.lock.json)",
		"GitPulse copy of Manvi's serve contract",
	} {
		if got := remedy(reader, "/gp"); strings.Contains(got, "names no remedy") {
			t.Fatalf("%s has no remedy", reader)
		}
	}
	for _, pin := range []string{"Manvi pin (scripts/module-pins.txt)", "Jarvis pin (upstream.lock.json)"} {
		if !strings.Contains(remedy(pin, "/gp"), "ask before bumping") {
			t.Fatalf("%s remedy does not ask before a pin bump", pin)
		}
	}
}

func TestStrictConsumersRefusesADryRun(t *testing.T) {
	_, restore := swapStderr(t)
	code := RunInstall([]string{"devmap", "--dry-run", "--strict-consumers", "--prefix", t.TempDir()})
	restore()
	if code != 2 {
		t.Fatalf("exit %d", code)
	}
}
