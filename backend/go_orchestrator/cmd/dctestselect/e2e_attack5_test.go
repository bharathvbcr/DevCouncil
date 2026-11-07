//go:build e2e

package main

import (
	"context"
	"encoding/json"
	"sort"
	"strings"
	"testing"
	"time"
)

// B1 closure on the real tree (cargo metadata only, nothing built). For every
// excluded member X, change X/src/lib.rs; an independent reverse closure over
// the packages' DECLARED dependencies (not the resolve graph the selector uses)
// names every member that can reach X, and each non-excluded one must be in
// the one cargo command. No excluded crate may become a Unit.
func TestE2EAttack5RealTreeEveryExcludedCrateSelectsEveryNonExcludedTransitiveDependent(t *testing.T) {
	root := mustRoot(t)
	ctx := context.Background()
	raw, err := execRunner{timeout: 3 * time.Minute}.Output(ctx, root+"/rust", "cargo", "metadata", "--format-version", "1", "--locked")
	if err != nil {
		t.Fatal(err)
	}
	var w struct {
		Packages []struct {
			Name         string `json:"name"`
			ID           string `json:"id"`
			Dependencies []struct {
				Name string `json:"name"`
			} `json:"dependencies"`
		} `json:"packages"`
		Members []string `json:"workspace_members"`
	}
	if err := json.Unmarshal(raw, &w); err != nil {
		t.Fatal(err)
	}
	member := map[string]bool{}
	isMember := map[string]bool{}
	for _, id := range w.Members {
		isMember[id] = true
	}
	rev := map[string][]string{}
	for _, p := range w.Packages {
		if isMember[p.ID] {
			member[p.Name] = true
		}
		for _, d := range p.Dependencies {
			rev[d.Name] = append(rev[d.Name], p.Name)
		}
	}
	ws, err := loadRust(ctx, execRunner{timeout: 3 * time.Minute}, root, "rust")
	if err != nil {
		t.Fatal(err)
	}
	for _, x := range DefaultRustExcludes {
		want := []string{}
		for n := range reach([]string{x}, rev) {
			if n != x && member[n] && !contains(DefaultRustExcludes, n) {
				want = append(want, n)
			}
		}
		sort.Strings(want)
		file := "rust/" + x + "/src/lib.rs"
		in := inputs(withChanged(file), withTests(file))
		in.Rust, in.RustDir = ws, "rust"
		p := BuildPlan(in)
		t.Logf("%s: declared-closure dependents=%v everything=%v selected=%v", x, want, p.Everything, cmds(p))
		if p.Everything {
			continue // fail-closed is acceptable (dc-glob: gusset)
		}
		got := map[string]bool{}
		for _, s := range p.Selected {
			for _, u := range s.Units {
				if u.Kind == "rust" {
					got[u.Pkg] = true
					if contains(DefaultRustExcludes, u.Pkg) {
						t.Errorf("%s: excluded crate %s became a Unit", x, u.Pkg)
					}
				}
			}
		}
		for _, n := range want {
			if !got[n] {
				t.Errorf("%s changed: non-excluded transitive dependent %s (declared deps) not selected", x, n)
			}
		}
		if len(want) == 0 {
			for _, c := range cmds(p) {
				if strings.HasPrefix(c, "cargo ") {
					t.Errorf("%s has no non-excluded dependents but selected %q", x, c)
				}
			}
		}
	}
}
