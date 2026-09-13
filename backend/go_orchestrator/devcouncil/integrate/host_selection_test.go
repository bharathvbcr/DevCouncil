package integrate

import (
	"errors"
	"fmt"
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
	// And no retired name may reappear in `Hosts` later. The loop above only
	// knows the two names retired so far; this one holds for every entry of
	// either table. `checkHost` tests `Hosts` first, so a name in both is
	// accepted and its `retiredHosts` explanation becomes unreachable — the
	// successor it points at would never be printed.
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
// Read out of the Rust source rather than mirrored: a copy of the list here
// agrees with itself no matter what the integrator does, which is how the two
// came apart before.
func TestAdvertisedHostsMatchTheRustIntegrator(t *testing.T) {
	rust, err := rustIntegratorHosts(readRustIntegrator(t))
	if err != nil {
		t.Fatalf("cannot read the hosts the Rust integrator accepts: %v", err)
	}
	got := slices.Clone(Hosts)
	slices.Sort(got)
	slices.Sort(rust)
	if !slices.Equal(got, rust) {
		t.Fatalf("host lists have drifted:\n  go:   %v\n  rust: %v", got, rust)
	}
}

// The drift a literal slice could not see. Injecting an arm into the real
// source stands in for a host added on the Rust side and nowhere else: against
// a hardcoded mirror this changes nothing and the check stays green. Against
// the parse it is a mismatch.
func TestARustOnlyHostAdditionIsCaught(t *testing.T) {
	// Injected at the one anchor the reader already requires to exist, so this
	// cannot rot independently of it.
	const added = "gremlin"
	source := strings.Replace(
		readRustIntegrator(t),
		rustNameMatch,
		rustNameMatch+"\n            Self::Gremlin => \""+added+"\",",
		1,
	)

	rust, err := rustIntegratorHosts(source)
	if err != nil {
		t.Fatalf("cannot read the hosts the Rust integrator accepts: %v", err)
	}
	if !slices.Contains(rust, added) {
		t.Fatalf("a host added to the Rust integrator was not read back: %v", rust)
	}
	got := slices.Clone(Hosts)
	slices.Sort(got)
	slices.Sort(rust)
	if slices.Equal(got, rust) {
		t.Fatalf("a Rust-only host addition still compares equal to Hosts: %v", rust)
	}
}

// Losing the shape this is read from must fail, not quietly compare `Hosts`
// against an empty list — which every name would then look like drift against,
// or, if the comparison were the other way round, none of them would.
func TestAnUnreadableIntegratorFailsClosed(t *testing.T) {
	for name, source := range map[string]string{
		"renamed function": "impl Host { pub fn name(self) -> &'static str { self.label() } }",
		"no match on self": "impl Host { pub fn as_str(self) -> &'static str { self.label() } }",
		"empty source":     "",
		"arm names no literal": `
impl Host {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => CURSOR_NAME,
        }
    }
}
`,
	} {
		t.Run(name, func(t *testing.T) {
			if rust, err := rustIntegratorHosts(source); err == nil {
				t.Fatalf("accepted a source with nothing readable in it: %v", rust)
			}
		})
	}
}

// Every other match in that file maps hosts to something that is not a host
// name — skill directories, config paths. A reader that collected literals past
// the end of `as_str` would report those as hosts, and `Hosts` would look
// wrong for naming only the real ones.
func TestLiteralsPastTheMatchAreNotHosts(t *testing.T) {
	const source = `
impl Host {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Warp => "warp",
        }
    }

    fn skill_destinations(self) -> &'static [&'static str] {
        match self {
            Self::Cursor => &[".cursor/skills"],
            Self::Warp => &[],
        }
    }
}
`
	rust, err := rustIntegratorHosts(source)
	if err != nil {
		t.Fatal(err)
	}
	if want := []string{"cursor", "warp"}; !slices.Equal(rust, want) {
		t.Fatalf("read %v, want %v", rust, want)
	}
}

// readRustIntegrator returns the source of the Rust integrator this command
// spawns. An unreadable source fails: it is committed in this repository, so
// its absence is a broken checkout rather than an environment this test may
// pass in without having checked anything.
//
// The file is outside this module, so `go test`'s result cache does not notice
// when it changes: locally, a Rust-side edit can be answered with a cached pass
// until something else invalidates the entry. Re-run with `-count=1` after
// touching the integrator. CI already does (`go test ./... -count=1`), so drift
// cannot reach a merge on a stale entry.
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

// The match that maps each host to the name it is typed under.
const rustNameMatch = "match self {"

// rustIntegratorHosts returns the host names the Rust integrator accepts, read
// out of `Host::as_str` in devmap-cli/src/integrate.rs rather than restated.
//
// `as_str` is the anchor because it is the only place the names are written as
// literals: the `--help` text, the value parser and `Host::parse`'s refusal are
// all generated from clap's `ValueEnum` derive, which spells them from the
// variants themselves. A Rust test holds `as_str` and clap to the same spelling,
// so these are the names the CLI accepts and not merely ones it prints.
//
// Only the arms of that one match are read: scanning stops where it closes, so
// the file's other matches — skill directories, config paths — cannot be
// mistaken for host names. An arm that names its host indirectly is an error
// rather than a skip, and so is a missing shape, so a rename upstream fails
// this check instead of silently emptying one side of it.
func rustIntegratorHosts(source string) ([]string, error) {
	fn := strings.Index(source, "fn as_str(")
	if fn < 0 {
		return nil, errors.New("no `fn as_str(` in the source")
	}
	rel := strings.Index(source[fn:], rustNameMatch)
	if rel < 0 {
		return nil, fmt.Errorf("`fn as_str(` does not open a `%s`", rustNameMatch)
	}

	var hosts []string
	depth := 1 // already inside the `match`
	for _, line := range strings.Split(source[fn+rel+len(rustNameMatch):], "\n") {
		trimmed := strings.TrimSpace(line)
		_, name, isArm := strings.Cut(trimmed, "=>")
		if isArm && depth == 1 && !strings.HasPrefix(trimmed, "//") {
			literal := rustStringLiteral.FindStringSubmatch(name)
			if literal == nil {
				return nil, fmt.Errorf("arm %q names no literal host", trimmed)
			}
			hosts = append(hosts, literal[1])
		}
		if depth += strings.Count(line, "{") - strings.Count(line, "}"); depth <= 0 {
			break
		}
	}
	if len(hosts) == 0 {
		return nil, errors.New("`Host::as_str` matched no string literals; the shape read here has changed")
	}
	return hosts, nil
}

// Every advertised host reaches an adapter that configures something.
//
// This replaces `TestSupportedHostsAreStillAccepted`, which ranged over
// `Hosts` and asserted `checkHost` accepted each one — but `checkHost` returns
// nil exactly when `slices.Contains(Hosts, host)`, so the assertion reduced to
// `Hosts ⊆ Hosts` and could not fail for any value of `Hosts`. Its name
// claimed the invariant `integrate.go` still enforces at runtime, in the
// `default` arm: `host %q is advertised but has no adapter`. Adding a seventh
// name to `Hosts` would have kept that test green and shipped a host that
// exits non-zero the first time anyone selects it.
//
// Driving `Run` rather than reading the switch's case labels is deliberate:
// reading the thing under test back is what made the old check tautological.
//
// `ModeCheck`, not `ModeDryRun`, and the difference is not cosmetic. The DevMap
// composition block runs `if mode == ModeApply || mode == ModeDryRun`
// (`integrate.go:197`), so a dry run spawns two real `devmap` subprocesses per
// host — twelve across the table — against whichever binary happens to be
// installed, and hands them this `TempDir` as their project root. The empty-tree
// assertion below would then also be asserting that that binary honours
// `--dry-run`, so a stale global `devmap` reddens this test naming a host, for a
// cause with nothing to do with that host's adapter: the false signal this test
// replaced, one layer down. `ModeCheck` reaches the same `switch`, fills the same
// `receipt.Files`, and spawns nothing.
func TestEveryAdvertisedHostHasAnAdapter(t *testing.T) {
	for _, host := range Hosts {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			receipt, err := Run(Options{Root: root, Host: host, Mode: ModeCheck})
			if err != nil {
				t.Fatalf("%s is advertised but has no working adapter: %v", host, err)
			}
			if len(receipt.Files) == 0 {
				t.Fatalf("%s reached an adapter that plans no file: %+v", host, receipt)
			}
			// Asserted rather than assumed: this is what keeps the check's
			// result attributable to the adapter instead of to an installed
			// binary's behaviour.
			if len(receipt.Spawned) != 0 {
				t.Fatalf("%s spawned %v; a check mode must not run anything",
					host, receipt.Spawned)
			}
			// A check that touched the tree would make the check itself the
			// thing that changed the repository.
			entries, readErr := os.ReadDir(root)
			if readErr != nil {
				t.Fatal(readErr)
			}
			if len(entries) != 0 {
				t.Fatalf("check mode wrote %d entr(ies) for %s", len(entries), host)
			}
		})
	}
}

// The converse: a server document for a name `Hosts` does not offer is dead
// configuration. `checkHost` refuses anything outside `Hosts` before the switch
// is reached, so such an entry can never be selected — and reads as support
// for a host that is not actually on offer.
func TestEveryServerDocumentBelongsToAnAdvertisedHost(t *testing.T) {
	for host := range hostMcpDocs {
		if !slices.Contains(Hosts, host) {
			t.Errorf("hostMcpDocs configures %q, which Hosts does not advertise", host)
		}
	}
}
