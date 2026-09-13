package integrate

import (
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// A host this command cannot configure must be refused before anything is
// written or spawned. Until `checkHost` existed, an unknown name fell through
// to a note on an otherwise ordinary receipt: `integrate banana --apply`
// exited 0, reported success, and spawned `devmap integrate banana` first.
func TestUnknownHostIsRefusedAndWritesNothing(t *testing.T) {
	for _, host := range []string{"banana", "vscode", "GEMINI-2", "  "} {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			receipt, err := Run(Options{Root: root, Host: host, Mode: ModeApply})
			if err == nil {
				t.Fatalf("accepted unsupported host %q (receipt %+v)", host, receipt)
			}
			if receipt != nil {
				t.Fatalf("returned a receipt for a refused host: %+v", receipt)
			}
			entries, readErr := os.ReadDir(root)
			if readErr != nil {
				t.Fatal(readErr)
			}
			if len(entries) != 0 {
				t.Fatalf("refused host still wrote %d entr(ies)", len(entries))
			}
		})
	}
}

// A name that used to work is answered with where it went, not just that it is
// invalid. Someone typing `integrate gemini` has run it before.
func TestRetiredHostsExplainTheSuccessor(t *testing.T) {
	// Each retired name must be answered with the thing to do instead, not
	// merely that it is unknown.
	for host, want := range map[string]string{
		"gemini": "antigravity",
		"aider":  "no mcp server",
	} {
		t.Run(host, func(t *testing.T) {
			_, err := Run(Options{Root: t.TempDir(), Host: host, Mode: ModeApply})
			if err == nil {
				t.Fatalf("%s was accepted", host)
			}
			if !strings.Contains(strings.ToLower(err.Error()), want) {
				t.Fatalf("refusal does not point at %q: %v", want, err)
			}
		})
	}
}

func TestRetiredHostsAreNotOffered(t *testing.T) {
	for _, host := range []string{"gemini", "aider"} {
		if slices.Contains(Hosts, host) {
			t.Fatalf("%s is still advertised in Hosts", host)
		}
		if _, known := retiredHosts[host]; !known {
			t.Fatalf("%s was dropped without an explanation for callers", host)
		}
	}
	// And no retired name may reappear in `Hosts` later. `checkHost` looks in
	// `Hosts` first, so a name in both is accepted with its explanation
	// unreachable — the successor it points at would never be printed.
	for host := range retiredHosts {
		if slices.Contains(Hosts, host) {
			t.Fatalf("%s is both advertised and retired; its explanation is unreachable", host)
		}
	}
}

// Dropping a host from installation says nothing about removal. A
// `.gemini/settings.json` already on disk must stay cleanable by the tool that
// no longer writes it, or retiring an adapter strands every registration it
// ever made.
func TestRetiringAHostKeepsItsCleanupPath(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, ".gemini", "settings.json")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	const before = `{"theme":"dark","hooks":{"BeforeTool":[{"hooks":[{"command":"dev hook pre-tool-use"},{"command":"keep-me"}]}]}}`
	if err := os.WriteFile(path, []byte(before), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := Uninstall(UninstallOptions{Root: root, Client: "gemini", Mode: ModeApply}); err != nil {
		t.Fatalf("cleanup refused a retired host: %v", err)
	}
	after, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	text := string(after)
	if strings.Contains(text, "dev hook") {
		t.Fatalf("registration survived cleanup: %s", text)
	}
	if !strings.Contains(text, "keep-me") || !strings.Contains(text, "dark") {
		t.Fatalf("cleanup damaged unrelated settings: %s", text)
	}
}

// The two lists that disagreed are the reason a name could be accepted here
// and refused by the binary this command then spawns.
//
// This check used to restate the Rust side as a literal slice, which made it
// blind to the only drift it exists to catch: a host added to `Host::parse`
// alone moved nothing on this side, so the comparison stayed green while
// `integrate <name>` was accepted here and refused by `devmap`. The Rust side
// is therefore read out of the Rust source.
//
// The binary is not asked instead, because it has no surface that answers.
// `devmap integrate` takes its host as a bare `String` with no clap value
// parser, and the help text it prints is a doc comment that itself lists three
// of the six — deriving from it would fail this test for a reason that is not
// drift.
func TestAdvertisedHostsMatchTheRustIntegrator(t *testing.T) {
	rust, err := rustIntegratorHosts(readRustIntegrator(t))
	if err != nil {
		t.Fatalf("cannot read the hosts `Host::parse` accepts: %v", err)
	}
	got := slices.Clone(Hosts)
	slices.Sort(got)
	slices.Sort(rust)
	if !slices.Equal(got, rust) {
		t.Fatalf("host lists have drifted:\n  go:   %v\n  rust: %v", got, rust)
	}
}

// The drift the literal slice could not see. Injecting an arm into the real
// source stands in for a host added on the Rust side and nowhere else: against
// a hardcoded mirror this changes nothing and the check stays green, which is
// how the two lists came apart in the first place. Against the parse it is a
// mismatch.
func TestARustOnlyHostAdditionIsCaught(t *testing.T) {
	// Injected at the one anchor the reader already requires to exist, so this
	// cannot rot independently of it.
	const added = "gremlin"
	source := strings.Replace(
		readRustIntegrator(t),
		"match name {",
		"match name {\n            \""+added+"\" => Ok(Self::Gremlin),",
		1,
	)

	rust, err := rustIntegratorHosts(source)
	if err != nil {
		t.Fatalf("cannot read the hosts `Host::parse` accepts: %v", err)
	}
	if !slices.Contains(rust, added) {
		t.Fatalf("a host added to `Host::parse` was not read back: %v", rust)
	}
	got := slices.Clone(Hosts)
	slices.Sort(got)
	slices.Sort(rust)
	if slices.Equal(got, rust) {
		t.Fatalf("a Rust-only host addition still compares equal to Hosts: %v", rust)
	}
}

// The refusal message names all six hosts in prose. A reader that collected
// quoted strings out of the function would keep agreeing with `Hosts` after
// every arm had been deleted — a check that can no longer run reporting what a
// check that ran and passed reports.
func TestTheRefusalMessageIsNotReadAsArms(t *testing.T) {
	const armless = `
impl Host {
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        match name {
            other => bail!(
                "unsupported host {other:?}; expected cursor, claude, codex, \
                 antigravity, opencode, or warp"
            ),
        }
    }
}
`
	if rust, err := rustIntegratorHosts(armless); err == nil {
		t.Fatalf("read %v out of a `parse` with no arms left", rust)
	}
}

// Losing the shape this is read from must fail, not quietly compare `Hosts`
// against an empty list — which every name would then look like drift against,
// or, if the comparison were the other way round, none of them would.
func TestAnUnreadableParseFailsClosed(t *testing.T) {
	for name, source := range map[string]string{
		"renamed function": "impl Host { pub fn from_name(name: &str) -> Result<Self> { Self::lookup(name) } }",
		"no match on name": "impl Host { pub fn parse(name: &str) -> Result<Self> { Self::lookup(name) } }",
		"empty source":     "",
	} {
		t.Run(name, func(t *testing.T) {
			if rust, err := rustIntegratorHosts(source); err == nil {
				t.Fatalf("accepted a source with nothing to read: %v", rust)
			}
		})
	}
}

// readRustIntegrator returns the source of the Rust integrator this command
// spawns. An unreadable source fails: it is committed beside this package, so
// its absence is a broken checkout rather than an environment this test may
// pass in without having checked anything.
func readRustIntegrator(t *testing.T) string {
	t.Helper()
	path := filepath.Join(testsupport.RustWorkspace(t), "devmap-cli", "src", "integrate.rs")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("cannot read the Rust integrator at %s: %v", path, err)
	}
	return string(raw)
}

// rustStringLiteral matches one Rust string literal, escapes included.
var rustStringLiteral = regexp.MustCompile(`"((?:[^"\\]|\\.)*)"`)

// rustIntegratorHosts returns the host names `Host::parse` accepts, read from
// the arms of its `match` rather than restated.
//
// Arms are matched, not literals: the catch-all's `bail!` names every host in
// prose, so scanning stops at the catch-all and only the pattern side of `=>`
// is read. An or-pattern (`"warp" | "oz" =>`) contributes both names.
//
// Returns an error rather than an empty slice when the shape is gone, so a
// rename upstream fails this check instead of silently emptying one side of it.
func rustIntegratorHosts(source string) ([]string, error) {
	fn := strings.Index(source, "fn parse(")
	if fn < 0 {
		return nil, errors.New("no `fn parse(` in the source")
	}
	const opener = "match name {"
	rel := strings.Index(source[fn:], opener)
	if rel < 0 {
		return nil, errors.New("`fn parse(` does not open a `match name {`")
	}

	var hosts []string
	depth := 1 // already inside the `match`
	for _, line := range strings.Split(source[fn+rel+len(opener):], "\n") {
		trimmed := strings.TrimSpace(line)
		if pattern, _, isArm := strings.Cut(trimmed, "=>"); isArm {
			if !strings.HasPrefix(trimmed, `"`) {
				// The catch-all binds a name instead of matching a literal.
				// Everything past it is the refusal, not an accepted host.
				break
			}
			for _, m := range rustStringLiteral.FindAllStringSubmatch(pattern, -1) {
				hosts = append(hosts, m[1])
			}
		}
		if depth += strings.Count(line, "{") - strings.Count(line, "}"); depth <= 0 {
			break
		}
	}
	if len(hosts) == 0 {
		return nil, errors.New("`Host::parse` matched no string literals; the shape read here has changed")
	}
	return hosts, nil
}

// The other direction of the same table. `checkHost` refuses a name that is not
// in `Hosts` before `Run` ever reaches the switch, so an adapter keyed by a
// name nobody advertises is unreachable: it cannot be invoked, and it cannot
// be noticed. Retiring a host by deleting it from `Hosts` alone leaves exactly
// that.
//
// Not symmetric with the check above, and deliberately so: cursor, claude and
// codex are advertised without appearing here, because their own adapters
// write more than a server document.
func TestEveryServerDocumentBelongsToAnAdvertisedHost(t *testing.T) {
	for host := range hostMcpDocs {
		if !slices.Contains(Hosts, host) {
			t.Fatalf("%s has a server document but is not advertised; nothing can reach it", host)
		}
	}
}

// Advertising a host is a promise that this command can configure it, and the
// check that used to stand here could not tell whether that promise held: it
// asked `checkHost` about every name in `Hosts`, and `checkHost` answers by
// looking in `Hosts`. It asserted that each member of a list is in that list,
// so it passed for every possible state of the code — including the state it
// existed to catch.
//
// `Run` has that failure for real. A name in `Hosts` with no adapter behind it
// reaches the switch's default and is refused there as "advertised but has no
// adapter", and that is now reachable by an ordinary edit: a host added to
// `Host::parse` arrives here as a red drift check whose obvious fix is to
// append the name to `Hosts`, which does nothing to give it an adapter. So
// every advertised host is driven through `Run` instead of asked about.
//
// Check mode is what keeps this hermetic. It reports the files it would touch
// and returns before the DevMap composition, so nothing is spawned and nothing
// is written.
func TestEveryAdvertisedHostHasAnAdapter(t *testing.T) {
	for _, host := range Hosts {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			receipt, err := Run(Options{Root: root, Host: host, Mode: ModeCheck})
			if err != nil {
				t.Fatalf("%s is advertised but cannot be configured: %v", host, err)
			}
			if len(receipt.Files) == 0 {
				t.Fatalf("%s named no files: nothing behind the name ran", host)
			}
			entries, readErr := os.ReadDir(root)
			if readErr != nil {
				t.Fatal(readErr)
			}
			if len(entries) != 0 {
				t.Fatalf("%s wrote %d entr(ies) in check mode", host, len(entries))
			}
		})
	}
}
