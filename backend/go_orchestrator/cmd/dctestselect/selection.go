package main

import (
	"fmt"
	"path"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// pkgSel accumulates what one cargo package contributes to the selection. A
// package is selected whole (every target `cargo test -p` runs: lib, every bin
// including auto-discovered ones, every test, the doctests) or by declared test
// targets. Nothing narrower exists: a module path libtest prints is not a path
// the source tree spells (`#[path]`, `include!`), so a filter on one can match
// nothing, or match only part of what it was meant to, and pass.
type pkgSel struct {
	pkg     RustPackage
	whole   string          // non-empty: every target of the package, and why
	targets map[string]bool // declared test targets, as `--test name`
	files   map[string]bool
}

type goSel struct {
	pkg   GoPackage
	whole string
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
	ps := &pkgSel{pkg: pkg, targets: map[string]bool{}, files: map[string]bool{}}
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
		s.rustSel(pkg).whole = "every target of " + pkg.Name + ": " + p + " changed outside any symbol (" + reason + ")"
		return "crate " + pkg.Name, true
	}
	if pkg, ok := s.goOwner(p); ok {
		s.goSel(pkg).whole = "go test " + pkg.ImportPath + ": " + p + " changed outside any symbol (" + reason + ")"
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
				s.goSel(pkg).whole = "go test " + pkg.ImportPath + ": it imports " + strings.TrimPrefix(imp, ws.Module+"/") + ", which drives a Rust binary"
				break
			}
		}
	}
}

// place maps one changed or reached .rs or .go path onto the commands that run
// it, or says why it cannot. The mapping is exactly:
//
//   - a main-workspace member's tests/ file that is a declared test target
//     selects that target;
//   - any other file under a member's tests/ or src/ selects every target of
//     that member;
//   - a Go file selects its package.
//
// Every other path (examples/, benches/, fixtures outside tests/, a crate of
// another cargo workspace, a Go file outside every package of the module) is
// not placed.
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
		for _, t := range pkg.Targets {
			if t.Src == p && t.is("test") {
				if len(t.RequiredFeatures) > 0 {
					s.drop(p, fmt.Sprintf("target %s requires features %v, which this job's full suite does not enable", t.Name, t.RequiredFeatures))
				} else {
					ps.targets["--test "+t.Name] = true
				}
				return "", true
			}
		}
		if ps.whole == "" {
			ps.whole = "every target of " + pkg.Name + ": " + p + " is a module or fixture of it"
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

// commands turns the accumulated selection into commands, in a stable order.
func (s *selection) commands(seeds []devmap.BlastSeed) []Selected {
	out := []Selected{}
	for _, name := range sortedKeys(s.rust) {
		ps := s.rust[name]
		own := seedsUnder(seeds, func(file string) bool { return strings.HasPrefix(file, ps.pkg.Dir+"/") })
		files := sortedKeys(ps.files)
		argv := []string{"cargo", "test", "--locked", "-p", name}
		if ps.whole != "" {
			out = append(out, Selected{
				Command: logged(newCommand(s.in.RustDir, append(argv, "--no-fail-fast")...)),
				Units:   []Unit{{Kind: "rust", Pkg: name}}, Files: files, Seeds: own, Why: ps.whole,
			})
			continue
		}
		var units []Unit
		for _, arg := range sortedKeys(ps.targets) {
			argv = append(argv, strings.Fields(arg)...)
			units = append(units, Unit{Kind: "rust", Pkg: name, Target: arg})
		}
		if len(units) == 0 {
			continue
		}
		out = append(out, Selected{
			Command: logged(newCommand(s.in.RustDir, append(argv, "--no-fail-fast")...)),
			Units:   units, Files: files, Seeds: own, Why: "blast tests",
		})
	}
	for _, ip := range sortedKeys(s.gos) {
		gs := s.gos[ip]
		own := seedsUnder(seeds, func(file string) bool { return path.Dir(file) == gs.pkg.Dir })
		why := "blast tests"
		if gs.whole != "" {
			why = gs.whole
		}
		out = append(out, Selected{
			Command: logged(newCommand(s.in.GoDir, "go", "test", "-count=1", "-json", ip)),
			Units:   []Unit{{Kind: "go", Pkg: ip}}, Files: sortedKeys(gs.files), Seeds: own, Why: why,
		})
	}
	return out
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
