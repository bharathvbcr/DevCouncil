package devmap

import (
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
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
	cmd.Env = append(os.Environ(), "HOME="+home)
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

// TestTheLiveStdioContractReplay replays protocols/devmap/stdio.v1.json against
// the real binary: every document the contract names is produced by a real
// command and checked against it, and the error envelope is shown to be
// distinguishable from a report.
func TestTheLiveStdioContractReplay(t *testing.T) {
	bin := devmapBinary(t)
	contract := loadContract(t)
	home := t.TempDir()
	root := fixtureRepo(t)
	servers := []byte(`{"servers":[{"name":"contract-probe","command":"/opt/probe/bin/probe","args":["mcp"]}]}`)

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
