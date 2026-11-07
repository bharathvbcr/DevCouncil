package main

import (
	"fmt"
	"path"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// pkgSel is what one cargo package contributes to the selection: the crate,
// whole (every target `cargo test -p` runs: lib, every bin including
// auto-discovered ones, every test, the doctests). Nothing narrower exists: a
// module path libtest prints is not a path the source tree spells (`#[path]`,
// `include!`), and a per-target choice misses a file another target includes.
type pkgSel struct {
	pkg   RustPackage
	why   string
	files map[string]bool
}

type goSel struct {
	pkg   GoPackage
	why   string
	files map[string]bool
}

// selection maps changed and reached paths onto the commands the job's full
// suite also runs. A path inside the mapping is placed; a path it cannot place
// is refused with a reason, and the caller runs everything.
type selection struct {
	in       Inputs
	excluded map[string]bool
	rust     map[string]*pkgSel
	gos      map[string]*goSel
	dropped  []Dropped
}

func newSelection(in Inputs) *selection {
	s := &selection{in: in, excluded: map[string]bool{}, rust: map[string]*pkgSel{}, gos: map[string]*goSel{}}
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

func (s *selection) rustSel(pkg RustPackage) *pkgSel {
	if ps, ok := s.rust[pkg.Name]; ok {
		return ps
	}
	ps := &pkgSel{pkg: pkg, files: map[string]bool{}}
	s.rust[pkg.Name] = ps
	return ps
}

func (s *selection) goSel(pkg GoPackage) *goSel {
	if gs, ok := s.gos[pkg.ImportPath]; ok {
		return gs
	}
	gs := &goSel{pkg: pkg, files: map[string]bool{}}
	s.gos[pkg.ImportPath] = gs
	return gs
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
		s.rustSel(pkg).why = "every target of " + pkg.Name + ": " + p + " changed outside any symbol (" + reason + ")"
		return "crate " + pkg.Name, true
	}
	if pkg, ok := s.goOwner(p); ok {
		s.goSel(pkg).why = "go test " + pkg.ImportPath + ": " + p + " changed outside any symbol (" + reason + ")"
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
				s.goSel(pkg).why = "go test " + pkg.ImportPath + ": it imports " + strings.TrimPrefix(imp, ws.Module+"/") + ", which drives a Rust binary"
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
		s.goSel(pkg).files[p] = true
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
		ps := s.rustSel(pkg)
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
	if c, ok := s.rustCommand(seeds); ok {
		out = append(out, c)
	}
	if c, ok := s.goCommand(seeds); ok {
		out = append(out, c)
	}
	return out
}

// rustCommand selects the changed crates and every workspace member that
// depends on them. A dependent the full suite excludes is dropped and logged.
func (s *selection) rustCommand(seeds []devmap.BlastSeed) (Selected, bool) {
	if len(s.rust) == 0 {
		return Selected{}, false
	}
	byName := map[string]RustPackage{}
	for _, pkg := range s.in.Rust.Packages {
		byName[pkg.Name] = pkg
	}
	direct := sortedKeys(s.rust)
	chosen := map[string]bool{}
	var whys, files []string
	fileSet := map[string]bool{}
	for _, name := range direct {
		ps := s.rust[name]
		chosen[name] = true
		whys = append(whys, ps.why)
		for f := range ps.files {
			fileSet[f] = true
		}
		for _, dep := range s.in.Rust.Dependents[name] {
			if s.excluded[dep] {
				s.drop(byName[dep].Dir, fmt.Sprintf("package %s depends on %s and is excluded from this job's full suite", dep, name))
				continue
			}
			chosen[dep] = true
		}
	}
	files = sortedKeys(fileSet)
	var extra []string
	argv := []string{"cargo", "test", "--locked"}
	var units []Unit
	for _, name := range sortedKeys(chosen) {
		argv = append(argv, "-p", name)
		units = append(units, Unit{Kind: "rust", Pkg: name})
		if s.rust[name] == nil {
			extra = append(extra, name)
		}
	}
	why := strings.Join(whys, "; ")
	if len(extra) > 0 {
		why += "; with dependents " + strings.Join(extra, ", ")
	}
	dirs := map[string]bool{}
	for _, name := range direct {
		dirs[s.rust[name].pkg.Dir+"/"] = true
	}
	own := seedsUnder(seeds, func(file string) bool {
		for d := range dirs {
			if strings.HasPrefix(file, d) {
				return true
			}
		}
		return false
	})
	return Selected{
		Command: logged(newCommand(s.in.RustDir, append(argv, "--no-fail-fast")...)),
		Units:   units, Files: files, Seeds: own, Why: why,
	}, true
}

// goCommand selects the changed packages and every package of the module that
// imports one of them, transitively, keeping those with test files: a package
// whose dependency changed can fail without a line of its own changing.
func (s *selection) goCommand(seeds []devmap.BlastSeed) (Selected, bool) {
	if len(s.gos) == 0 {
		return Selected{}, false
	}
	importers := map[string][]string{}
	byPath := map[string]GoPackage{}
	for _, pkg := range s.in.Go.Packages {
		byPath[pkg.ImportPath] = pkg
		for _, imp := range pkg.Imports {
			importers[imp] = append(importers[imp], pkg.ImportPath)
		}
	}
	chosen := map[string]bool{}
	queue := sortedKeys(s.gos)
	for _, ip := range queue {
		chosen[ip] = true
	}
	for len(queue) > 0 {
		ip := queue[0]
		queue = queue[1:]
		for _, up := range importers[ip] {
			if !chosen[up] {
				chosen[up] = true
				queue = append(queue, up)
			}
		}
	}
	var whys, pkgs []string
	var units []Unit
	fileSet := map[string]bool{}
	dirs := map[string]bool{}
	for _, ip := range sortedKeys(s.gos) {
		gs := s.gos[ip]
		why := "blast tests"
		if gs.why != "" {
			why = gs.why
		}
		whys = append(whys, why)
		dirs[gs.pkg.Dir] = true
		for f := range gs.files {
			fileSet[f] = true
		}
	}
	var extra []string
	for _, ip := range sortedKeys(chosen) {
		if len(byPath[ip].TestFiles) == 0 {
			continue
		}
		pkgs = append(pkgs, ip)
		units = append(units, Unit{Kind: "go", Pkg: ip})
		if s.gos[ip] == nil {
			extra = append(extra, ip)
		}
	}
	if len(pkgs) == 0 {
		return Selected{}, false
	}
	why := strings.Join(whys, "; ")
	if len(extra) > 0 {
		why += "; with importers " + strings.Join(extra, ", ")
	}
	own := seedsUnder(seeds, func(file string) bool { return dirs[path.Dir(file)] })
	argv := append([]string{"go", "test", "-count=1", "-json"}, pkgs...)
	return Selected{
		Command: logged(newCommand(s.in.GoDir, argv...)),
		Units:   units, Files: sortedKeys(fileSet), Seeds: own, Why: why,
	}, true
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
