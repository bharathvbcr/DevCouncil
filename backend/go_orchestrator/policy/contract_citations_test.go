package policy

import (
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"testing"
)

// The contracts name the source that owns each artifact, and a consumer
// resolving a disagreement is told to trust that source. Four of those names
// pointed at files that no longer existed — `manvi/policy/decision.go`,
// `manvi/gate/gate.go`, `crates/dc-store/src/schema.rs`,
// `crates/dc-verify/src/rigor.rs` — after the code moved into DevCouncil, so the
// ownership table sent readers to the wrong repository for the authority on
// three of the five shared artifacts.
//
// This holds every source path a contract file cites to a file that exists in
// this repository. A path prefixed with another repository's name
// (`GitPulse/…`, `Manvi/…`) belongs to that repository and is checked there,
// not here; it is named explicitly so it cannot be mistaken for a local one.

var citedSource = regexp.MustCompile(`[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)+\.(?:go|rs|py|ts|mjs|sql)\b`)

// foreignRepos are the repositories a contract may cite by prefix.
var foreignRepos = []string{"GitPulse/", "Manvi/"}

// prose is the text of a contract file that can cite a source: all of a
// Markdown file, and only the description and $comment strings of a JSON one.
// A JSON contract also carries data — verdict.cases.json is full of fixture
// targets like src/lib/example.ts — and a path in data is not a citation.
func prose(name string, raw []byte) (string, error) {
	if !strings.HasSuffix(name, ".json") {
		return string(raw), nil
	}
	var doc any
	if err := json.Unmarshal(raw, &doc); err != nil {
		return "", err
	}
	var out strings.Builder
	var walk func(any)
	walk = func(v any) {
		switch v := v.(type) {
		case map[string]any:
			for key, child := range v {
				if text, ok := child.(string); ok && (key == "description" || key == "$comment") {
					out.WriteString(text)
					out.WriteByte('\n')
					continue
				}
				walk(child)
			}
		case []any:
			for _, child := range v {
				walk(child)
			}
		}
	}
	walk(doc)
	return out.String(), nil
}

// citedSourcePaths returns every DevCouncil-relative source path the contract
// files in dir cite, keyed by path with the files that cite it.
func citedSourcePaths(dir string) (map[string][]string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	cited := map[string][]string{}
	for _, entry := range entries {
		name := entry.Name()
		if !entry.Type().IsRegular() || name == "CHECKSUMS" || strings.HasPrefix(name, ".") {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			return nil, err
		}
		text, err := prose(name, raw)
		if err != nil {
			return nil, err
		}
	match:
		for _, path := range citedSource.FindAllString(text, -1) {
			for _, foreign := range foreignRepos {
				if strings.HasPrefix(path, foreign) {
					continue match
				}
			}
			if files := cited[path]; len(files) == 0 || files[len(files)-1] != name {
				cited[path] = append(files, name)
			}
		}
	}
	return cited, nil
}

func missingCitations(repoRoot string, cited map[string][]string) []string {
	var missing []string
	for path, files := range cited {
		if _, err := os.Stat(filepath.Join(repoRoot, filepath.FromSlash(path))); err != nil {
			missing = append(missing, path+" (cited by "+strings.Join(files, ", ")+")")
		}
	}
	sort.Strings(missing)
	return missing
}

func TestEveryCitedOwnerSourceExists(t *testing.T) {
	dir := contractsDir(t)
	repoRoot := filepath.Join(dir, "..", "..")
	cited, err := citedSourcePaths(dir)
	if err != nil {
		t.Fatal(err)
	}
	// The table names at least the verdict, store and redaction owners; a scan
	// that found none is a scan that did not run, not a clean result.
	if len(cited) < 3 {
		t.Fatalf("found only %d cited source paths in %s; the scan is broken: %v", len(cited), dir, cited)
	}
	if missing := missingCitations(repoRoot, cited); len(missing) > 0 {
		t.Fatalf("contracts cite source files that do not exist in this repository:\n  %s\n\n"+
			"Point each at its current owner, or prefix it with the repository that owns it "+
			"(%s).", strings.Join(missing, "\n  "), strings.Join(foreignRepos, ", "))
	}
}

// The check above passes on a correct tree; this shows it fails on a stale one.
func TestAStaleCitationIsReported(t *testing.T) {
	root := t.TempDir()
	contracts := filepath.Join(root, "backend", "contracts")
	if err := os.MkdirAll(filepath.Join(root, "rust", "dc-store", "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(contracts, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "rust", "dc-store", "src", "schema.rs"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	doc := "Owner: `rust/dc-store/src/schema.rs`. Old owner: `manvi/gate/gate.go`.\n" +
		"Foreign: `GitPulse/src-tauri/src/ledger/mod.rs`.\n"
	if err := os.WriteFile(filepath.Join(contracts, "README.md"), []byte(doc), 0o644); err != nil {
		t.Fatal(err)
	}
	cited, err := citedSourcePaths(contracts)
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := cited["GitPulse/src-tauri/src/ledger/mod.rs"]; ok {
		t.Fatal("a path qualified with another repository must not be checked here")
	}
	missing := missingCitations(root, cited)
	if len(missing) != 1 || !strings.HasPrefix(missing[0], "manvi/gate/gate.go") {
		t.Fatalf("want exactly the stale manvi/gate/gate.go reported, got %v", missing)
	}
}
