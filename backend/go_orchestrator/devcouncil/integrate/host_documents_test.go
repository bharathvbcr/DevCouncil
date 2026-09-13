package integrate

import (
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"testing"
)

func readJSON(t *testing.T, path string) map[string]any {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var out map[string]any
	if err := json.Unmarshal(b, &out); err != nil {
		t.Fatalf("%s is not JSON: %v\n%s", path, err, b)
	}
	return out
}

// servers returns the host's server map, whatever it is nested under.
func servers(t *testing.T, doc hostMcpDoc, root string) map[string]any {
	t.Helper()
	file := readJSON(t, filepath.Join(root, doc.rel))
	if doc.container == "" {
		return file
	}
	inner, ok := file[doc.container].(map[string]any)
	if !ok {
		t.Fatalf("%s: %q is not an object", doc.rel, doc.container)
	}
	return inner
}

func apply(t *testing.T, root, host string) *Receipt {
	t.Helper()
	receipt, err := Run(Options{Root: root, Host: host, Mode: ModeApply})
	if err != nil {
		t.Fatalf("integrate %s: %v", host, err)
	}
	return receipt
}

// The server list a host already had is not ours to edit. Only our own entry
// is added or replaced.
func TestEachHostRegistersWithoutDisturbingNeighbours(t *testing.T) {
	for host, doc := range hostMcpDocs {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, doc.rel)
			if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
				t.Fatal(err)
			}
			existing := map[string]any{"theirs": map[string]any{"command": "other"}}
			if doc.container != "" {
				existing = map[string]any{doc.container: existing, "unrelated": "keep me"}
			}
			b, _ := json.Marshal(existing)
			if err := os.WriteFile(path, b, 0o644); err != nil {
				t.Fatal(err)
			}

			apply(t, root, host)
			got := servers(t, doc, root)
			if _, ours := got[mcpServerName]; !ours {
				t.Fatalf("%s: our server was not registered: %v", host, got)
			}
			if _, theirs := got["theirs"]; !theirs {
				t.Fatalf("%s: dropped an unrelated server: %v", host, got)
			}
			if doc.container != "" {
				if readJSON(t, path)["unrelated"] != "keep me" {
					t.Fatalf("%s: dropped an unrelated top-level key", host)
				}
			}
		})
	}
}

// OpenCode's entry names the program as one argv array; the other two use a
// command plus a separate args list. Getting this wrong yields a config the
// host silently ignores.
func TestEntryShapeMatchesTheHost(t *testing.T) {
	for host, doc := range hostMcpDocs {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			apply(t, root, host)
			entry, ok := servers(t, doc, root)[mcpServerName].(map[string]any)
			if !ok {
				t.Fatalf("%s: entry is not an object", host)
			}
			_, argv := entry["command"].([]any)
			if argv != doc.argvForm {
				t.Fatalf("%s: argv form is %v, want %v (entry %v)", host, argv, doc.argvForm, entry)
			}
			if doc.argvForm {
				if entry["type"] != "local" {
					t.Fatalf("%s: opencode entries declare a local type: %v", host, entry)
				}
			} else if _, hasArgs := entry["args"].([]any); !hasArgs {
				t.Fatalf("%s: expected a separate args list: %v", host, entry)
			}
		})
	}
}

// A preamble key establishes a default; it does not overrule a user who pinned
// something else on purpose.
func TestPreambleIsEstablishedNotImposed(t *testing.T) {
	doc := hostMcpDocs["opencode"]
	root := t.TempDir()
	path := filepath.Join(root, doc.rel)
	if err := os.WriteFile(path, []byte(`{"$schema":"https://opencode.ai/config-v2.json"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	apply(t, root, "opencode")
	if got := readJSON(t, path)["$schema"]; got != "https://opencode.ai/config-v2.json" {
		t.Fatalf("overwrote a pinned schema: %v", got)
	}

	fresh := t.TempDir()
	apply(t, fresh, "opencode")
	if got := readJSON(t, filepath.Join(fresh, doc.rel))["$schema"]; got != "https://opencode.ai/config.json" {
		t.Fatalf("a new file did not get the default schema: %v", got)
	}
}

// Applying twice must leave the file alone the second time, and `--check` must
// then agree it is current. Before `planWrite` compared against the *merged*
// result, a file holding our entry beside someone else's always read as drift.
func TestASecondApplyIsCleanAndCheckAgrees(t *testing.T) {
	for host, doc := range hostMcpDocs {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			apply(t, root, host)
			path := filepath.Join(root, doc.rel)
			first, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			apply(t, root, host)
			second, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if string(first) != string(second) {
				t.Fatalf("%s: a second apply changed bytes", host)
			}
			receipt, err := Run(Options{Root: root, Host: host, Mode: ModeCheck})
			if err != nil {
				t.Fatal(err)
			}
			if got := receipt.Files[doc.rel]; got != "unchanged" {
				t.Fatalf("%s: check reports %q for an up-to-date file", host, got)
			}
		})
	}
}

// A config this command cannot read is kept, not replaced.
func TestUnreadableConfigIsRefusedNotOverwritten(t *testing.T) {
	cases := map[string]string{
		"not-json":        `{ this is not json`,
		"wrong-container": `{"mcp": "not an object"}`,
		"has-comments":    "{\n  // a comment encoding/json cannot round-trip\n  \"mcp\": {}\n}",
	}
	doc := hostMcpDocs["opencode"]
	for label, body := range cases {
		t.Run(label, func(t *testing.T) {
			root := t.TempDir()
			path := filepath.Join(root, doc.rel)
			if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
				t.Fatal(err)
			}
			if _, err := Run(Options{Root: root, Host: "opencode", Mode: ModeApply}); err == nil {
				t.Fatalf("%s was accepted", label)
			}
			after, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			if string(after) != body {
				t.Fatalf("%s was rewritten despite being refused:\n%s", label, after)
			}
		})
	}
}

func TestDryRunWritesNothing(t *testing.T) {
	for host, doc := range hostMcpDocs {
		t.Run(host, func(t *testing.T) {
			root := t.TempDir()
			if _, err := Run(Options{Root: root, Host: host, Mode: ModeDryRun}); err != nil {
				t.Fatal(err)
			}
			if _, err := os.Stat(filepath.Join(root, doc.rel)); !os.IsNotExist(err) {
				t.Fatalf("%s: dry run created %s", host, doc.rel)
			}
		})
	}
}

// The Rust integrator writes the `devmap` entry into these same files. Two
// tables that disagree about where a host keeps its servers would have the two
// binaries writing to different places, or to different keys in one place.
//
// This check used to restate the Rust table as a Go literal, which left it
// blind to the only drift it exists to catch: moving a path or renaming a
// container key on the Rust side alone moved nothing here, so the comparison
// stayed green while the two binaries wrote to different files. The Rust side
// is therefore read out of the Rust source. See
// `TestARustOnlyHostDocumentChangeIsCaught` for the cases that used to pass.
func TestHostDocumentsMatchTheRustIntegrator(t *testing.T) {
	rust, err := rustIntegratorHostDocuments(readRustIntegrator(t))
	if err != nil {
		t.Fatalf("cannot read the documents `Host::mcp_document` writes: %v", err)
	}
	if drift := hostDocumentDrift(hostMcpDocs, rust); drift != "" {
		t.Fatalf("host document tables have drifted:\n%s", drift)
	}
}

// The drift the literal table could not see. Each case edits the real Rust
// source the way a Rust-only change would, then runs the *same* comparison the
// check above runs and requires it to report the difference. Against a
// hardcoded mirror every one of these stays green, which is how a table like
// this comes apart.
func TestARustOnlyHostDocumentChangeIsCaught(t *testing.T) {
	// Each edit is anchored on text the reader itself must parse, so a case
	// cannot rot into a no-op independently of the reader it exercises.
	for name, edit := range map[string]func(*testing.T, string) string{
		"moved path": func(t *testing.T, source string) string {
			return replaceOnce(t, source,
				`rel: ".agents/mcp_config.json"`, `rel: ".agents/moved.json"`)
		},
		"renamed container": func(t *testing.T, source string) string {
			return replaceOnce(t, source,
				`container: Some("mcp")`, `container: Some("mcpServers")`)
		},
		"dropped container": func(t *testing.T, source string) string {
			return replaceOnce(t, source, `container: Some("mcp")`, `container: None`)
		},
		"flipped entry shape": func(t *testing.T, source string) string {
			return replaceOnce(t, source, `argv_form: true`, `argv_form: false`)
		},
		"changed preamble": func(t *testing.T, source string) string {
			return replaceOnce(t, source,
				`preamble: &[("$schema", "https://opencode.ai/config.json")]`,
				`preamble: &[("$schema", "https://opencode.ai/config-v9.json")]`)
		},
		// A host that gains a document on the Rust side needs a name as well as
		// an arm, because the name is what joins the two tables.
		"added host": func(t *testing.T, source string) string {
			source = replaceOnce(t, source,
				`Self::Cursor => "cursor",`,
				"Self::Cursor => \"cursor\",\n            Self::Gremlin => \"gremlin\",")
			return replaceOnce(t, source,
				`Self::Cursor | Self::Claude | Self::Codex => None,`,
				"Self::Cursor | Self::Claude | Self::Codex => None,\n"+
					"            Self::Gremlin => Some(HostMcpDoc {\n"+
					"                rel: \"gremlin.json\",\n"+
					"                container: None,\n"+
					"                preamble: &[],\n"+
					"                argv_form: false,\n"+
					"            }),")
		},
		// The mirror image: a host this package still writes for, which the
		// Rust side has stopped writing. Matched as a whole arm rather than by
		// spelling out its indentation, so reformatting the source does not
		// quietly turn this case into a no-op.
		"dropped host": func(t *testing.T, source string) string {
			arm := regexp.MustCompile(`(?s)\n[ \t]*Self::Warp => Some\(HostMcpDoc \{.*?\n[ \t]*\}\),`)
			if n := len(arm.FindAllString(source, -1)); n != 1 {
				t.Fatalf("the Warp arm matched %d times, want exactly 1", n)
			}
			return arm.ReplaceAllString(source, "\n            Self::Warp => None,")
		},
	} {
		t.Run(name, func(t *testing.T) {
			rust, err := rustIntegratorHostDocuments(edit(t, readRustIntegrator(t)))
			if err != nil {
				t.Fatalf("cannot read the edited source: %v", err)
			}
			if drift := hostDocumentDrift(hostMcpDocs, rust); drift == "" {
				t.Fatalf("a Rust-only change still compares equal:\n  rust: %v", rust)
			}
		})
	}
}

// Losing the shape this is read from must fail, not quietly compare the Go
// table against an empty map — which would report every host as drift, or, with
// the comparison the other way round, none of them.
func TestAnUnreadableHostDocumentParseFailsClosed(t *testing.T) {
	// The base every case below is derived from, so each one isolates the single
	// defect it is named for. It is asserted to parse first: cases that edit a
	// fixture the reader already rejects would all pass while testing nothing.
	const readable = `
impl Host {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warp => "warp",
        }
    }
    fn mcp_document(self) -> Option<HostMcpDoc> {
        match self {
            Self::Warp => Some(HostMcpDoc {
                rel: "warp.json",
                container: None,
                preamble: &[],
                argv_form: false,
            }),
        }
    }
}
`
	base, err := rustIntegratorHostDocuments(readable)
	if err != nil {
		t.Fatalf("the fixture every case below edits does not parse: %v", err)
	}
	if want := map[string]hostMcpDoc{"warp": {rel: "warp.json"}}; !reflect.DeepEqual(base, want) {
		t.Fatalf("the fixture parses to %v, want %v", base, want)
	}

	// Derived with the same one-occurrence guard the injection cases use, so a
	// fixture edit that silently misses cannot leave a case passing vacuously.
	edit := func(from, to string) string { return replaceOnce(t, readable, from, to) }

	for name, source := range map[string]string{
		"renamed function": edit("fn mcp_document(", "fn server_document("),
		"no match on self": edit("match self {"+"\n"+`            Self::Warp => Some`, "{ Self::lookup("),
		"no as_str":        edit("fn as_str(", "fn name("),
		"empty source":     "",
		// A `HostMcpDoc` that has grown a field is the dangerous one: a reader
		// that skipped the unknown key would go on reporting agreement about a
		// document whose shape it can no longer describe.
		"grown struct":      edit("argv_form: false,", "argv_form: false,"+"\n"+"                indent: 4,"),
		"missing argv_form": edit("                argv_form: false,"+"\n", ""),
		"unreadable arm":    edit("Some(HostMcpDoc {", "Self::lookup("),
		"truncated rel":     edit(`rel: "warp.json",`, "rel: warp_path(),"),
		// Every arm returning None finds the shape and reads nothing out of it.
		// An empty map here is not "the two tables agree".
		"every arm is None": edit("Some(HostMcpDoc {", "None, // "),
		// A document with no name in `as_str` cannot be keyed against the Go
		// table at all, so it must not be dropped from the comparison.
		"document unnamed": edit(`Self::Warp => "warp",`, `Self::Cursor => "cursor",`),
	} {
		t.Run(name, func(t *testing.T) {
			rust, err := rustIntegratorHostDocuments(source)
			if err == nil {
				t.Fatalf("accepted a source the reader cannot describe: %v", rust)
			}
			// Logged so `-v` shows *why* each case was refused: a case that
			// fails for an unintended reason still passes an err != nil check.
			t.Logf("refused with: %v", err)
		})
	}
}

// replaceOnce edits the single occurrence of from. An anchor that is absent or
// duplicated fails: the value of an injection test is that the edit lands on
// the text the reader parses, and a silent no-op would pass for the wrong
// reason.
func replaceOnce(t *testing.T, source, from, to string) string {
	t.Helper()
	if n := strings.Count(source, from); n != 1 {
		t.Fatalf("anchor %q occurs %d times in the Rust integrator, want exactly 1", from, n)
	}
	return strings.Replace(source, from, to, 1)
}

// hostDocumentDrift describes how the two tables disagree, or "" when they
// match.
//
// Documents are compared whole rather than field by field. That is what keeps
// the comparison honest as `hostMcpDoc` grows: a field added here and populated
// in `hostMcpDocs` differs from the zero value the reader leaves behind, so the
// check goes red until the reader learns to read it — and a field added to the
// *Rust* struct is refused outright by `rustHostDocFields`.
func hostDocumentDrift(goSide, rustSide map[string]hostMcpDoc) string {
	var lines []string
	for _, host := range slices.Sorted(maps.Keys(goSide)) {
		want, written := rustSide[host]
		if !written {
			lines = append(lines, fmt.Sprintf("  %s: written here, absent from `Host::mcp_document`", host))
			continue
		}
		if got := goSide[host]; !reflect.DeepEqual(got, want) {
			lines = append(lines, fmt.Sprintf("  %s:\n    go:   %s\n    rust: %s",
				host, describeHostDoc(got), describeHostDoc(want)))
		}
	}
	for _, host := range slices.Sorted(maps.Keys(rustSide)) {
		if _, written := goSide[host]; !written {
			lines = append(lines, fmt.Sprintf("  %s: written by `Host::mcp_document`, absent here", host))
		}
	}
	return strings.Join(lines, "\n")
}

func describeHostDoc(doc hostMcpDoc) string {
	return fmt.Sprintf("rel=%q container=%q preamble=%v argvForm=%v",
		doc.rel, doc.container, doc.preamble, doc.argvForm)
}

// rustMatchArm is one arm of a `match self { … }`: the `Self::` variants on its
// pattern side, whatever follows `=>` on the same line, and the raw lines of
// its body. Body lines are kept raw because the values are read out of them;
// only the structural scan strips their strings and comments.
type rustMatchArm struct {
	variants []string
	head     string
	body     []string
}

var (
	rustSelfVariant  = regexp.MustCompile(`Self::(\w+)`)
	rustFieldName    = regexp.MustCompile(`^([a-z_][a-z0-9_]*)\s*:`)
	rustDocRel       = regexp.MustCompile(`^rel:\s*"((?:[^"\\]|\\.)*)"\s*,?$`)
	rustDocContainer = regexp.MustCompile(`^container:\s*(?:None|Some\("((?:[^"\\]|\\.)*)"\))\s*,?$`)
	rustDocPreamble  = regexp.MustCompile(`^preamble:\s*&\[(.*)\]\s*,?$`)
	rustDocArgvForm  = regexp.MustCompile(`^argv_form:\s*(true|false)\s*,?$`)
	rustPreamblePair = regexp.MustCompile(`\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*"((?:[^"\\]|\\.)*)"\s*\)`)
)

// rustStructuralLine leaves only the punctuation that shapes a line, so braces
// inside strings and comments cannot move the nesting depth.
//
// The order is load-bearing. `"https://opencode.ai/config.json"` contains a
// `//`, so stripping comments before strings would cut the line in half at a
// URL and lose the bracket that closes the preamble.
func rustStructuralLine(line string) string {
	stripped := rustStringLiteral.ReplaceAllString(line, `""`)
	if at := strings.Index(stripped, "//"); at >= 0 {
		stripped = stripped[:at]
	}
	return stripped
}

// rustSelfMatchArms returns the arms of the `match self { … }` opened inside the
// named function.
func rustSelfMatchArms(source, fn string) ([]rustMatchArm, error) {
	at := strings.Index(source, fn)
	if at < 0 {
		return nil, fmt.Errorf("no `%s` in the source", fn)
	}
	const opener = "match self {"
	rel := strings.Index(source[at:], opener)
	if rel < 0 {
		return nil, fmt.Errorf("`%s` does not open a `%s`", fn, opener)
	}

	var arms []rustMatchArm
	depth := 1 // already inside the `match`
	for _, line := range strings.Split(source[at+rel+len(opener):], "\n") {
		structural := rustStructuralLine(line)
		delta := strings.Count(structural, "{") - strings.Count(structural, "}")
		trimmed := strings.TrimSpace(structural)

		// Whether a line is an arm is decided on the structural form, so a `=>`
		// inside a string or a comment cannot pass for one. The arm is then cut
		// out of the *raw* line, because the head carries the values and
		// rustStructuralLine has already blanked them. A `Self::` pattern holds
		// no string literals, so the raw line's first `=>` is the one the
		// structural form just confirmed.
		if _, _, isArm := strings.Cut(trimmed, "=>"); isArm && strings.HasPrefix(trimmed, "Self::") {
			pattern, head, _ := strings.Cut(strings.TrimSpace(line), "=>")
			arms = append(arms, rustMatchArm{
				variants: variantsIn(pattern),
				head:     strings.TrimSpace(head),
			})
		} else if len(arms) > 0 {
			arms[len(arms)-1].body = append(arms[len(arms)-1].body, line)
		}

		// Counted after the line is classified, not before: an arm and the
		// brace closing the `match` share a line in a single-line match, and
		// breaking first would discard that arm.
		if depth += delta; depth <= 0 {
			if len(arms) == 0 {
				return nil, fmt.Errorf("the `%s` in `%s` has no arms", opener, fn)
			}
			return arms, nil
		}
	}
	return nil, fmt.Errorf("the `%s` in `%s` is never closed", opener, fn)
}

func variantsIn(pattern string) []string {
	var variants []string
	for _, m := range rustSelfVariant.FindAllStringSubmatch(pattern, -1) {
		variants = append(variants, m[1])
	}
	return variants
}

// rustHostNames maps each `Host` variant to the name it answers to, read from
// `Host::as_str`.
//
// That function, not `mcp_document`, is what supplies the key the two tables
// are joined on. Lowercasing the variant here instead would agree with the Rust
// source only by coincidence — nothing makes `OpenCode` spell itself
// `opencode`, and a variant renamed without touching `as_str` would then look
// like drift that is not there.
func rustHostNames(source string) (map[string]string, error) {
	arms, err := rustSelfMatchArms(source, "fn as_str(")
	if err != nil {
		return nil, err
	}
	names := map[string]string{}
	for _, arm := range arms {
		name := rustStringLiteral.FindStringSubmatch(arm.head)
		if name == nil {
			return nil, fmt.Errorf("`as_str` arm for %v yields no name: %q", arm.variants, arm.head)
		}
		for _, variant := range arm.variants {
			names[variant] = name[1]
		}
	}
	if len(names) == 0 {
		return nil, errors.New("`Host::as_str` named no variants; the shape read here has changed")
	}
	return names, nil
}

// rustIntegratorHostDocuments returns the server documents `Host::mcp_document`
// writes, keyed by host name and read out of the Rust source rather than
// restated.
//
// Arms are walked rather than literals collected, because most of this file's
// quoted strings are paths that are not these: the same `mcp_document` match
// answers `None` for three hosts, and the module around it holds a `mod tests`
// that writes `opencode.json` a dozen times over.
//
// Returns an error rather than a short map whenever the shape it reads is gone
// — a renamed function, an arm it cannot classify, a field it does not know, or
// a document it cannot name — so a change upstream fails this check instead of
// silently emptying one side of it.
func rustIntegratorHostDocuments(source string) (map[string]hostMcpDoc, error) {
	names, err := rustHostNames(source)
	if err != nil {
		return nil, err
	}
	arms, err := rustSelfMatchArms(source, "fn mcp_document(")
	if err != nil {
		return nil, err
	}

	docs := map[string]hostMcpDoc{}
	for _, arm := range arms {
		if strings.HasPrefix(arm.head, "None") {
			continue // a host this module writes no project document for
		}
		if !strings.HasPrefix(arm.head, "Some(HostMcpDoc {") {
			return nil, fmt.Errorf("arm for %v returns neither `None` nor `Some(HostMcpDoc {`: %q",
				arm.variants, arm.head)
		}
		doc, err := rustHostDocFields(arm.body)
		if err != nil {
			return nil, fmt.Errorf("arm for %v: %w", arm.variants, err)
		}
		for _, variant := range arm.variants {
			name, named := names[variant]
			if !named {
				return nil, fmt.Errorf("`Host::%s` writes a document but `as_str` gives it no name", variant)
			}
			docs[name] = doc
		}
	}
	if len(docs) == 0 {
		return nil, errors.New("`Host::mcp_document` yielded no documents; the shape read here has changed")
	}
	return docs, nil
}

// rustHostDocFields reads one `HostMcpDoc` literal into the Go shape it is
// compared against.
//
// Every field the Rust struct declares must be read, and a field it does not
// recognise is an error rather than something to skip: a reader that ignored an
// unknown key would keep reporting that the two tables agree about a document
// whose shape it can no longer describe.
func rustHostDocFields(body []string) (hostMcpDoc, error) {
	var doc hostMcpDoc
	seen := map[string]bool{}
	for _, line := range body {
		// Whether this is a field line is decided on the structural form, so a
		// comment mentioning a field name is not read as one.
		match := rustFieldName.FindStringSubmatch(strings.TrimSpace(rustStructuralLine(line)))
		if match == nil {
			continue
		}
		field, raw := match[1], strings.TrimSpace(line)
		switch field {
		case "rel":
			value := rustDocRel.FindStringSubmatch(raw)
			if value == nil {
				return doc, fmt.Errorf("cannot read `rel` from %q", raw)
			}
			doc.rel = value[1]
		case "container":
			value := rustDocContainer.FindStringSubmatch(raw)
			if value == nil {
				return doc, fmt.Errorf("cannot read `container` from %q", raw)
			}
			// Empty for `None`, which is how the Go table spells "the document
			// root is the server map".
			doc.container = value[1]
		case "preamble":
			value := rustDocPreamble.FindStringSubmatch(raw)
			if value == nil {
				return doc, fmt.Errorf("cannot read `preamble` from %q", raw)
			}
			pairs := rustPreamblePair.FindAllStringSubmatch(value[1], -1)
			if inner := strings.TrimSpace(value[1]); inner != "" && len(pairs) == 0 {
				return doc, fmt.Errorf("cannot read the pairs in `preamble` from %q", raw)
			}
			for _, pair := range pairs {
				if doc.preamble == nil {
					doc.preamble = map[string]any{}
				}
				doc.preamble[pair[1]] = pair[2]
			}
		case "argv_form":
			value := rustDocArgvForm.FindStringSubmatch(raw)
			if value == nil {
				return doc, fmt.Errorf("cannot read `argv_form` from %q", raw)
			}
			doc.argvForm = value[1] == "true"
		default:
			return doc, fmt.Errorf(
				"unrecognized field %q; `HostMcpDoc` has grown a field this reader does not compare", field)
		}
		seen[field] = true
	}
	for _, field := range []string{"rel", "container", "preamble", "argv_form"} {
		if !seen[field] {
			return doc, fmt.Errorf("no `%s` in the arm", field)
		}
	}
	return doc, nil
}
