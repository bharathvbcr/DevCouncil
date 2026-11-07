package main

import (
	"fmt"
	"path"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// unitSel is what one cargo package or Go package contributes to the selection.
// A crate is selected whole (every target `cargo test -p` runs: lib, every bin
// including auto-discovered ones, every test, the doctests). Nothing narrower
// exists: a module path libtest prints is not a path the source tree spells
// (`#[path]`, `include!`), and a per-target choice misses a file another target
// includes.
type unitSel struct {
	dir, why string
	files    map[string]bool
}

// selection maps changed and reached paths onto the commands the job's full
// suite also runs. A path inside the mapping is placed; a path it cannot place
// is refused with a reason, and the caller runs everything.
type selection struct {
	in       Inputs
	excluded map[string]bool
	rust     map[string]*unitSel
	gos      map[string]*unitSel
	dropped  []Dropped
}

func newSelection(in Inputs) *selection {
	s := &selection{in: in, excluded: map[string]bool{}, rust: map[string]*unitSel{}, gos: map[string]*unitSel{}}
	for _, ex := range in.Excludes {
		s.excluded[ex] = true
	}
	return s
}

func (s *selection) drop(p, reason string) {
	s.dropped = append(s.dropped, Dropped{Path: p, Reason: reason})
}

func (s *selection) rustOwner(p string) (RustPackage, bool) {
	var best RustPackage
	found := false
	for _, pkg := range s.in.Rust.Packages {
		if strings.HasPrefix(p, pkg.Dir+"/") && (!found || len(pkg.Dir) > len(best.Dir)) {
			best, found = pkg, true
		}
	}
	return best, found
}

func (s *selection) goOwner(p string) (GoPackage, bool) {
	if !strings.HasSuffix(p, ".go") {
		return GoPackage{}, false
	}
	for _, pkg := range s.in.Go.Packages {
		if pkg.Dir == path.Dir(p) {
			return pkg, true
		}
	}
	return GoPackage{}, false
}

func unit(m map[string]*unitSel, name, dir string) *unitSel {
	if u, ok := m[name]; ok {
		return u
	}
	u := &unitSel{dir: dir, files: map[string]bool{}}
	m[name] = u
	return u
}

// widenOwner selects every test of the unit that owns a changed file, for a
// change the graph could not place in a symbol. It returns the scope it
// widened to ("" when the owner is outside the job's universe, and the file is
// logged as dropped) and false when no unit owns the file at all.
func (s *selection) widenOwner(p, reason string) (string, bool) {
	if pkg, ok := s.rustOwner(p); ok {
		if s.excluded[pkg.Name] {
			s.drop(p, fmt.Sprintf("package %s is excluded from this job's full suite", pkg.Name))
			return "", true
		}
		unit(s.rust, pkg.Name, pkg.Dir).why = "every target of " + pkg.Name + ": " + p + " changed outside any symbol (" + reason + ")"
		return "crate " + pkg.Name, true
	}
	if pkg, ok := s.goOwner(p); ok {
		unit(s.gos, pkg.ImportPath, pkg.Dir).why = "go test " + pkg.ImportPath + ": " + p + " changed outside any symbol (" + reason + ")"
		return "go package " + pkg.ImportPath, true
	}
	return "", false
}

// widenGoImporters selects the Go packages that drive Rust binaries: those
// importing testsupport (which builds dcstore, dcverify and dcgrep) or
// dc/devmap (which execs devmap).
func (s *selection) widenGoImporters(ws *GoWorkspace) {
	drivers := map[string]bool{ws.Module + "/testsupport": true, ws.Module + "/dc/devmap": true}
	for _, pkg := range ws.Packages {
		if len(pkg.TestFiles) == 0 {
			continue
		}
		for _, imp := range pkg.Imports {
			if drivers[imp] {
				unit(s.gos, pkg.ImportPath, pkg.Dir).why = "go test " + pkg.ImportPath + ": it imports " + strings.TrimPrefix(imp, ws.Module+"/") + ", which drives a Rust binary"
				break
			}
		}
	}
}

// place maps one changed or reached .rs or .go path onto the unit that runs
// it, or says why it cannot. The mapping is exactly:
//
//   - any file under a main-workspace member's tests/ or src/ selects that
//     crate;
//   - a Go file selects its package.
//
// Every other path (examples/, benches/, fixtures outside tests/, a crate of
// another cargo workspace, a Go file outside every package of the module) is
// not placed. commands widens each unit to the units that depend on it.
func (s *selection) place(p string) (reason string, ok bool) {
	switch {
	case strings.HasSuffix(p, ".go"):
		pkg, found := s.goOwner(p)
		if !found {
			return p + " is a Go file in no package of the module", false
		}
		gs := unit(s.gos, pkg.ImportPath, pkg.Dir)
		gs.files[p] = true
		if gs.why == "" {
			gs.why = "blast tests"
		}
		return "", true
	case strings.HasSuffix(p, ".rs"):
		pkg, found := s.rustOwner(p)
		if !found {
			return p + " is Rust outside every main-workspace member", false
		}
		rel := strings.TrimPrefix(p, pkg.Dir+"/")
		if !strings.HasPrefix(rel, "src/") && !strings.HasPrefix(rel, "tests/") {
			return p + " is Rust outside src/ and tests/ of " + pkg.Name, false
		}
		if s.excluded[pkg.Name] {
			s.drop(p, fmt.Sprintf("package %s is excluded from this job's full suite", pkg.Name))
			return "", true
		}
		ps := unit(s.rust, pkg.Name, pkg.Dir)
		ps.files[p] = true
		if ps.why == "" {
			ps.why = "every target of " + pkg.Name + ": " + p
		}
		return "", true
	}
	return p + " is neither a Rust nor a Go file", false
}

func sortedKeys[V any](m map[string]V) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// commands turns the accumulated selection into at most one cargo command and
// one go command. One cargo command, so cargo unifies the features of the
// selected crates as the full suite's --workspace does; a separate command per
// crate would build each with fewer features than the full suite gives it.
func (s *selection) commands(seeds []devmap.BlastSeed) []Selected {
	out := []Selected{}
	if len(s.rust) > 0 {
		byName := map[string]RustPackage{}
		for _, pkg := range s.in.Rust.Packages {
			byName[pkg.Name] = pkg
		}
		chosen := map[string]bool{}
		for name := range s.rust {
			chosen[name] = true
			for _, dep := range s.in.Rust.Dependents[name] {
				if s.excluded[dep] {
					s.drop(byName[dep].Dir, fmt.Sprintf("package %s depends on %s and is excluded from this job's full suite", dep, name))
				} else {
					chosen[dep] = true
				}
			}
		}
		out = append(out, assemble("rust", "dependents", s.rust, chosen, seeds, func(file, dir string) bool { return strings.HasPrefix(file, dir+"/") },
			func(names []string) Command {
				argv := []string{"cargo", "test", "--locked"}
				for _, n := range names {
					argv = append(argv, "-p", n)
				}
				return newCommand(s.in.RustDir, append(argv, "--no-fail-fast")...)
			}))
	}
	if len(s.gos) > 0 {
		importers := map[string][]string{}
		tested := map[string]bool{}
		for _, pkg := range s.in.Go.Packages {
			tested[pkg.ImportPath] = len(pkg.TestFiles) > 0
			for _, imp := range pkg.Imports {
				importers[imp] = append(importers[imp], pkg.ImportPath)
			}
		}
		chosen := reach(sortedKeys(s.gos), importers)
		for ip := range chosen {
			if !tested[ip] {
				delete(chosen, ip)
			}
		}
		if len(chosen) > 0 {
			out = append(out, assemble("go", "importers", s.gos, chosen, seeds, func(file, dir string) bool { return path.Dir(file) == dir },
				func(names []string) Command {
					return newCommand(s.in.GoDir, append([]string{"go", "test", "-count=1", "-json"}, names...)...)
				}))
		}
	}
	return out
}

// assemble builds the one command of a kind over the chosen units: the changed
// ones (direct) and those that depend on them.
func assemble(kind, label string, direct map[string]*unitSel, chosen map[string]bool, seeds []devmap.BlastSeed,
	under func(file, dir string) bool, command func(names []string) Command) Selected {
	var units []Unit
	var whys, extra []string
	files := map[string]bool{}
	for _, name := range sortedKeys(direct) {
		whys = append(whys, direct[name].why)
		for f := range direct[name].files {
			files[f] = true
		}
	}
	names := sortedKeys(chosen)
	for _, name := range names {
		units = append(units, Unit{Kind: kind, Pkg: name})
		if direct[name] == nil {
			extra = append(extra, name)
		}
	}
	why := strings.Join(whys, "; ")
	if len(extra) > 0 {
		why += "; with " + label + " " + strings.Join(extra, ", ")
	}
	own := seedsUnder(seeds, func(file string) bool {
		for _, u := range direct {
			if under(file, u.dir) {
				return true
			}
		}
		return false
	})
	return Selected{Command: logged(command(names)), Units: units, Files: sortedKeys(files), Seeds: own, Why: why}
}

func seedsUnder(seeds []devmap.BlastSeed, in func(file string) bool) []string {
	names := []string{}
	total := 0
	for _, seed := range seeds {
		if !in(seed.FilePath) {
			continue
		}
		total++
		if len(names) < maxSeedsPerCmd {
			names = append(names, seed.QualifiedName)
		}
	}
	if total > len(names) {
		names = append(names, fmt.Sprintf("... and %d more", total-len(names)))
	}
	return names
}

// logged routes a selected command's output into the fast phase's logs, so the
// audit can tell a failure only the selected phase saw from one the full suite
// also had. Go commands run with -json for the same reason.
func logged(c Command) Command {
	if len(c.Argv) > 0 && c.Argv[0] == "go" {
		c.GoJSON, c.Log = true, "selected-go.jsonl"
	} else {
		c.Log = "selected-rust.log"
	}
	return c
}
