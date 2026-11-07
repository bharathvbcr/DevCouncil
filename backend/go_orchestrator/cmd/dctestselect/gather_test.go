package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// fakeRunner answers git by the exact command line, and fails any other.
type fakeRunner map[string]string

func (f fakeRunner) Output(_ context.Context, _ string, argv ...string) ([]byte, error) {
	key := strings.Join(argv, " ")
	v, ok := f[key]
	if !ok {
		return nil, errors.New("unexpected command: " + key)
	}
	return []byte(v + "\n"), nil
}

func sha(c string) string { return strings.Repeat(c, 40) }

func TestAMergeBaseEqualToHeadMeasuresTheLastCommitNotAnEmptyChange(t *testing.T) {
	h, p := sha("a"), sha("b")
	run := fakeRunner{
		"git rev-parse --verify --quiet HEAD^{commit}":        h,
		"git rev-parse --verify --quiet origin/main^{commit}": h,
		"git merge-base origin/main " + h:                     h,
		"git rev-parse --verify --quiet " + h + "~1^{commit}": p,
	}
	base, head, err := resolveBase(context.Background(), run, Options{Repo: ".", Event: "workflow_dispatch", RefName: "main", BaseRef: "main"})
	if err != nil || base != p || head != h {
		t.Fatalf("base=%s head=%s err=%v; want the parent %s", base, head, err, p)
	}
	// A root commit has no parent: an error, which the plan widens on.
	delete(run, "git rev-parse --verify --quiet "+h+"~1^{commit}")
	if _, _, err := resolveBase(context.Background(), run, Options{Repo: ".", Event: "workflow_dispatch", RefName: "main", BaseRef: "main"}); err == nil {
		t.Fatal("a HEAD with no parent resolved a base")
	}
}

// metadataFixture is cargo metadata's shape, over real files so the paths
// resolve: devmap-cli (bin) depends on devmap-lib, which depends on
// devmap-leaf and, for tests only, on devmap-testonly; unrelated depends on
// nothing; dc-tool (a dc-* bin) depends on dc-lib.
func metadataFixture(t *testing.T, withResolve bool) (string, []byte) {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	type pkg struct{ name, kind string }
	pkgs := []pkg{{"devmap-cli", "bin"}, {"devmap-lib", "lib"}, {"devmap-leaf", "lib"}, {"devmap-testonly", "lib"}, {"unrelated", "lib"}, {"dc-tool", "bin"}, {"dc-lib", "lib"}}
	var packages, members []string
	for _, p := range pkgs {
		dir := filepath.Join(root, "rust", p.name)
		if err := os.MkdirAll(filepath.Join(dir, "src"), 0o755); err != nil {
			t.Fatal(err)
		}
		src := filepath.Join(dir, "src", "lib.rs")
		if p.kind == "bin" {
			src = filepath.Join(dir, "src", "main.rs")
		}
		for _, f := range []string{filepath.Join(dir, "Cargo.toml"), src} {
			if err := os.WriteFile(f, nil, 0o644); err != nil {
				t.Fatal(err)
			}
		}
		id := "path+file://" + dir + "#0.1.0"
		members = append(members, fmt.Sprintf("%q", id))
		packages = append(packages, fmt.Sprintf(`{"name":%q,"id":%q,"manifest_path":%q,"targets":[{"name":%q,"kind":[%q],"src_path":%q,"test":true,"doctest":%t}]}`,
			p.name, id, filepath.Join(dir, "Cargo.toml"), p.name, p.kind, src, p.kind == "lib"))
	}
	id := func(name string) string { return "path+file://" + filepath.Join(root, "rust", name) + "#0.1.0" }
	dep := func(name, kind string) string {
		k := "null"
		if kind != "" {
			k = fmt.Sprintf("%q", kind)
		}
		return fmt.Sprintf(`{"pkg":%q,"dep_kinds":[{"kind":%s}]}`, id(name), k)
	}
	resolve := `,"resolve":{"nodes":[` + strings.Join([]string{
		fmt.Sprintf(`{"id":%q,"deps":[%s]}`, id("devmap-cli"), dep("devmap-lib", "")),
		fmt.Sprintf(`{"id":%q,"deps":[%s,%s]}`, id("devmap-lib"), dep("devmap-leaf", "build"), dep("devmap-testonly", "dev")),
		fmt.Sprintf(`{"id":%q,"deps":[]}`, id("devmap-leaf")),
		fmt.Sprintf(`{"id":%q,"deps":[]}`, id("devmap-testonly")),
		fmt.Sprintf(`{"id":%q,"deps":[]}`, id("unrelated")),
		fmt.Sprintf(`{"id":%q,"deps":[%s]}`, id("dc-tool"), dep("dc-lib", "")),
		fmt.Sprintf(`{"id":%q,"deps":[]}`, id("dc-lib")),
	}, ",") + `]}`
	if !withResolve {
		resolve = ""
	}
	return root, []byte(`{"packages":[` + strings.Join(packages, ",") + `],"workspace_members":[` + strings.Join(members, ",") + `]` + resolve + `}`)
}

func TestTheDriverClosureIsWhatTheBinariesCompileNotTheTestOnlyDependencies(t *testing.T) {
	root, doc := metadataFixture(t, true)
	ws, err := decodeMetadata(doc, root)
	if err != nil {
		t.Fatal(err)
	}
	for _, in := range []string{"devmap-cli", "devmap-lib", "devmap-leaf", "dc-tool", "dc-lib"} {
		if !ws.DriverClosure[in] {
			t.Errorf("%s is compiled into a binary Go tests run and is not in the closure %v", in, ws.DriverClosure)
		}
	}
	for _, out := range []string{"devmap-testonly", "unrelated"} {
		if ws.DriverClosure[out] {
			t.Errorf("%s is not compiled into any such binary but is in the closure", out)
		}
	}
	if lib := ws.Packages[1]; lib.Name != "devmap-lib" || !lib.Targets[0].Doctest {
		t.Errorf("doctest was not decoded: %+v", lib)
	}
}

func TestMetadataWithoutAResolveGraphIsRefused(t *testing.T) {
	root, doc := metadataFixture(t, false)
	if _, err := decodeMetadata(doc, root); err == nil || !strings.Contains(err.Error(), "resolve graph") {
		t.Fatalf("err = %v; a closure that cannot be computed must not be an empty one", err)
	}
}
