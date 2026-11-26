package devmap

import (
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// contractPath is protocols/devmap/stdio.v1.json, the document that says what
// devmap's stdout promises this package.
var contractPath = filepath.Join("..", "..", "..", "..", "protocols", "devmap", "stdio.v1.json")

type contractDoc struct {
	Required map[string]string            `json:"required"`
	Nested   map[string]map[string]string `json:"nested"`
	Absent   []string                     `json:"absent"`
}

func loadContract(t *testing.T) map[string]contractDoc {
	t.Helper()
	raw, err := os.ReadFile(contractPath)
	if err != nil {
		t.Fatal(err)
	}
	var contract struct {
		Documents map[string]contractDoc `json:"documents"`
	}
	if err := json.Unmarshal(raw, &contract); err != nil {
		t.Fatal(err)
	}
	if len(contract.Documents) == 0 {
		t.Fatal("the contract names no documents")
	}
	return contract.Documents
}

func jsonType(v any) string {
	switch v.(type) {
	case string:
		return "string"
	case float64:
		return "number"
	case bool:
		return "boolean"
	case []any:
		return "array"
	case map[string]any:
		return "object"
	case nil:
		return "null"
	}
	return "unknown"
}

// conforms reports every way `doc` departs from `want`.
func conforms(doc map[string]any, want contractDoc) []string {
	var problems []string
	for field, typ := range want.Required {
		got, ok := doc[field]
		switch {
		case !ok:
			problems = append(problems, "missing "+field)
		case jsonType(got) != typ:
			problems = append(problems, field+" is "+jsonType(got)+", want "+typ)
		}
	}
	for field, inner := range want.Nested {
		obj, _ := doc[field].(map[string]any)
		for name, typ := range inner {
			if got, ok := obj[name]; !ok || jsonType(got) != typ {
				problems = append(problems, field+"."+name+" is not "+typ)
			}
		}
	}
	for _, field := range want.Absent {
		if _, ok := doc[field]; ok {
			problems = append(problems, "must not carry "+field)
		}
	}
	return problems
}

// devmapStdout runs the live binary under a scratch HOME and returns the first
// JSON document on stdout. A non-zero exit is not an error here: a failing
// --check and a refusal are both answers the contract describes.
func devmapStdout(t *testing.T, bin, home string, stdin []byte, args ...string) map[string]any {
	t.Helper()
	cmd := exec.Command(bin, args...)
	cmd.Env = testsupport.HostConfigChildEnv(home)
	cmd.Stdin = bytes.NewReader(stdin)
	var out bytes.Buffer
	cmd.Stdout = &out
	_ = cmd.Run()
	var doc map[string]any
	if err := json.NewDecoder(&out).Decode(&doc); err != nil {
		t.Fatalf("devmap %v wrote no JSON document on stdout: %v", args, err)
	}
	return doc
}

// A seat-pinned session exports CLAUDE_CONFIG_DIR, and devmap writes the Claude
// host's global config there. devmapStdout must not hand that variable to the
// child: the planted seat comes out byte for byte as it went in, and every file
// the apply reports lies under the scratch roots.
func TestStdioHarnessNeverReachesSeatConfig(t *testing.T) {
	bin := devmapBinary(t)
	seat := t.TempDir()
	planted := filepath.Join(seat, ".claude.json")
	const body = `{"projects":{},"sentinel":"seat-config-do-not-touch"}` + "\n"
	if err := os.WriteFile(planted, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("CLAUDE_CONFIG_DIR", seat)
	home := t.TempDir()
	root := fixtureRepo(t)
	servers := []byte(`{"servers":[{"name":"contract-probe","command":"` +
		testsupport.AbsoluteFixturePath("/opt/probe/bin/probe") + `","args":["mcp"]}]}`)

	doc := devmapStdout(t, bin, home, servers,
		"--json", "integrate", "claude", "--project-root", root, "--servers-stdin")
	if msg, failed := doc["error"]; failed {
		t.Fatalf("the apply refused, so nothing below was exercised: %v", msg)
	}
	if ok, present := doc["check_ok"].(bool); !present {
		t.Fatalf("the apply wrote no integrate report (check_ok missing, got %v): %v", ok, doc)
	}
	// IntegrateReport names the documents it touched under "path" in each of
	// these arrays (protocols/devmap/stdio.v1.json, integrate_report). DevMap
	// reports them canonical, which on Windows is the verbatim `\\?\C:\...`
	// spelling, so they are read through PlainPath as every reader must.
	var paths []string
	for _, key := range []string{"global_mcp", "project_mcp", "hooks", "servers", "guides"} {
		entries, _ := doc[key].([]any)
		for _, entry := range entries {
			if obj, ok := entry.(map[string]any); ok {
				if path, ok := obj["path"].(string); ok && path != "" {
					paths = append(paths, PlainPath(path))
				}
			}
		}
	}
	if len(paths) == 0 {
		t.Fatalf("the integrate report names no path, so the containment check would pass vacuously: %v", doc)
	}
	testsupport.AssertWithin(t, []string{root, home}, paths...)
	underHome := false
	for _, path := range paths {
		if rel, err := filepath.Rel(resolvedPath(home), resolvedPath(path)); err == nil && !strings.HasPrefix(rel, "..") {
			underHome = true
		}
	}
	if !underHome {
		t.Fatalf("no reported path lies under the scratch HOME %s, so the user-level config was not exercised: %v", home, paths)
	}
	after, err := os.ReadFile(planted)
	if err != nil || string(after) != body {
		t.Fatalf("the seat-pinned %s changed (err %v):\n%s", planted, err, after)
	}
	entries, err := os.ReadDir(seat)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 {
		t.Fatalf("the seat-pinned directory gained files: %v", entries)
	}
}

// resolvedPath follows symlinks in the longest existing prefix (macOS /var is
// /private/var) so two spellings of one directory compare equal.
func resolvedPath(p string) string {
	p = filepath.Clean(p)
	for q, rest := p, ""; ; q = filepath.Dir(q) {
		if r, err := filepath.EvalSymlinks(q); err == nil {
			return filepath.Join(r, rest)
		}
		if filepath.Dir(q) == q {
			return p
		}
		rest = filepath.Join(filepath.Base(q), rest)
	}
}

// TestTheLiveStdioContractReplay replays protocols/devmap/stdio.v1.json against
// the real binary: every document the contract names is produced by a real
// command and checked against it, and the error envelope is shown to be
// distinguishable from a report.
func TestTheLiveStdioContractReplay(t *testing.T) {
	bin := devmapBinary(t)
	contract := loadContract(t)
	home := t.TempDir()
	root := fixtureRepo(t)
	servers := []byte(`{"servers":[{"name":"contract-probe","command":"` +
		testsupport.AbsoluteFixturePath("/opt/probe/bin/probe") + `","args":["mcp"]}]}`)

	check := func(name string, doc map[string]any) {
		t.Helper()
		want, ok := contract[name]
		if !ok {
			t.Fatalf("the contract has no %q document", name)
		}
		if problems := conforms(doc, want); len(problems) > 0 {
			t.Errorf("%s departs from protocols/devmap/stdio.v1.json: %v\n%v", name, problems, doc)
		}
	}

	check("integrate_report", devmapStdout(t, bin, home, servers,
		"--json", "integrate", "claude", "--project-root", root, "--servers-stdin", "--check"))
	applied := devmapStdout(t, bin, home, servers,
		"--json", "integrate", "claude", "--project-root", root, "--servers-stdin")
	check("integrate_report", applied)
	check("remove_report", devmapStdout(t, bin, home, servers,
		"--json", "integrate", "claude", "--project-root", root, "--servers-stdin",
		"--remove", "--check", "--only-path", filepath.Join(root, ".mcp.json")))
	check("build_result", devmapStdout(t, bin, home, nil, "--json", "build", filepath.Join(root)))

	// A refusal writes the envelope, and the envelope is not a report.
	jsonc := filepath.Join(home, ".cursor", "mcp.json")
	if err := os.MkdirAll(filepath.Dir(jsonc), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(jsonc, []byte("// comment\n{\"mcpServers\":{}}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	refusal := devmapStdout(t, bin, home, nil, "--json", "integrate", "cursor", "--project-root", root, "--check")
	check("error_envelope", refusal)
}
