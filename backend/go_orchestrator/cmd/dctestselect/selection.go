package main

import (
	"fmt"
	"path"
	"sort"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// pkgSel accumulates what one cargo package contributes to the selection.
type pkgSel struct {
	pkg   RustPackage
	whole string // non-empty: every target of the package, and why
	// unfiltered and filtered are keyed by target argument (`--lib`,
	// `--test it`). A target in both runs whole.
	unfiltered map[string]bool
	filtered   map[string]map[string]bool
	files      map[string]bool
}

type goSel struct {
	pkg   GoPackage
	whole string
	files map[string]bool
}

// selection maps blast paths onto the commands the job's full suite also runs.
// A path it cannot map is dropped with its reason, never run: a command outside
// that universe could fail for a reason the full suite would never see.
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
	ps := &pkgSel{pkg: pkg, unfiltered: map[string]bool{}, filtered: map[string]map[string]bool{}, files: map[string]bool{}}
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

// add maps one blast test path onto the commands that run it.
func (s *selection) add(p string) {
	switch {
	case strings.HasSuffix(p, "_test.go"):
		pkg, ok := s.goOwner(p)
		if !ok {
			s.drop(p, "no `go list` package of this module contains it")
			return
		}
		s.goSel(pkg).files[p] = true
	case strings.HasSuffix(p, ".rs"):
		pkg, ok := s.rustOwner(p)
		if !ok {
			s.drop(p, "it is outside every cargo workspace member")
			return
		}
		if s.excluded[pkg.Name] {
			s.drop(p, fmt.Sprintf("package %s is excluded from this job's full suite", pkg.Name))
			return
		}
		s.addRust(s.rustSel(pkg), p)
	default:
		s.drop(p, "it is neither a Rust nor a Go test file")
	}
}

func (s *selection) addRust(ps *pkgSel, file string) {
	pkg := ps.pkg
	rel := strings.TrimPrefix(file, pkg.Dir+"/")
	bySrc := func(src string) *RustTarget {
		for i := range pkg.Targets {
			if pkg.Targets[i].Src == src {
				return &pkg.Targets[i]
			}
		}
		return nil
	}
	switch {
	case strings.HasPrefix(rel, "tests/"):
		if t := bySrc(file); t != nil {
			s.target(ps, *t, nil, file)
			return
		}
		sub := strings.TrimPrefix(rel, "tests/")
		if i := strings.Index(sub, "/"); i >= 0 {
			// tests/<dir>/...: the target whose root is that directory owns it.
			dir := pkg.Dir + "/tests/" + sub[:i]
			for _, t := range pkg.Targets {
				if t.is("test") && path.Dir(t.Src) == dir {
					s.target(ps, t, nil, file)
					return
				}
			}
		} else {
			// tests/<f>.rs that is not a target is a module of a harness
			// target. Confirm it is declared there; the filter `<f>::` is then
			// the module prefix libtest prints.
			name := strings.TrimSuffix(sub, ".rs")
			for _, t := range pkg.Targets {
				if !t.is("test") || path.Dir(t.Src) != pkg.Dir+"/tests" {
					continue
				}
				src, err := s.in.ReadFile(t.Src)
				if err == nil && modDecl(name).Match(src) {
					s.target(ps, t, []string{name + "::"}, file)
					return
				}
			}
		}
		// A support directory, or a module no harness declares: every test
		// target of the crate could include it.
		hit := false
		for _, t := range pkg.Targets {
			if t.is("test") {
				s.target(ps, t, nil, file)
				hit = true
			}
		}
		if !hit {
			s.drop(file, "the crate has no test target")
		}
	case strings.HasPrefix(rel, "src/"):
		if t := bySrc(file); t != nil {
			s.target(ps, *t, nil, file)
			return
		}
		hit := false
		for _, t := range pkg.Targets {
			// A bin whose main.rs sits in its own directory owns that tree.
			if t.is("bin") && path.Base(t.Src) == "main.rs" && path.Dir(t.Src) != pkg.Dir+"/src" && strings.HasPrefix(file, path.Dir(t.Src)+"/") {
				s.target(ps, t, modFilter(strings.TrimPrefix(file, path.Dir(t.Src)+"/")), file)
				hit = true
			}
		}
		if hit {
			return
		}
		filter := modFilter(strings.TrimPrefix(rel, "src/"))
		for _, t := range pkg.Targets {
			// The library owns a module of the crate root; so does a src/main.rs
			// binary, which declares its own modules. A file cannot be told
			// apart from here, so both are asked and a filter that matches
			// nothing costs only a launch.
			if t.isLib() || (t.is("bin") && t.Src == pkg.Dir+"/src/main.rs") {
				s.target(ps, t, filter, file)
				hit = true
			}
		}
		if !hit {
			s.drop(file, "no library or src/main.rs target of the crate can hold it")
		}
	default:
		s.drop(file, "it is not under tests/ or src/ of its crate")
	}
}

// modFilter is the libtest filter for a module file: `a/b.rs` and `a/b/mod.rs`
// are both `a::b::`. The crate root (`lib`, `main`) has no module prefix, so
// the whole target runs.
func modFilter(rel string) []string {
	rel = strings.TrimSuffix(rel, ".rs")
	rel = strings.TrimSuffix(rel, "/mod")
	if rel == "lib" || rel == "main" || rel == "" {
		return nil
	}
	return []string{strings.ReplaceAll(rel, "/", "::") + "::"}
}

func (s *selection) target(ps *pkgSel, t RustTarget, filters []string, file string) {
	var arg string
	switch {
	case t.isLib():
		arg = "--lib"
	case t.is("bin"):
		arg = "--bin " + t.Name
	case t.is("test"):
		arg = "--test " + t.Name
	default:
		s.drop(file, fmt.Sprintf("target %s (%s) is not run by cargo test", t.Name, strings.Join(t.Kinds, ",")))
		return
	}
	if len(t.RequiredFeatures) > 0 {
		s.drop(file, fmt.Sprintf("target %s requires features %v, which this job's full suite does not enable", t.Name, t.RequiredFeatures))
		return
	}
	if !t.Test {
		s.drop(file, fmt.Sprintf("target %s is not run by cargo test", t.Name))
		return
	}
	ps.files[file] = true
	if len(filters) == 0 {
		ps.unfiltered[arg] = true
		return
	}
	if ps.filtered[arg] == nil {
		ps.filtered[arg] = map[string]bool{}
	}
	for _, f := range filters {
		ps.filtered[arg][f] = true
	}
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
		base := []string{"cargo", "test", "--locked", "-p", name}
		if ps.whole != "" {
			out = append(out, Selected{
				Command: logged(newCommand(s.in.RustDir, append(base, "--no-fail-fast")...)),
				Units:   []Unit{{Kind: "rust", Pkg: name}}, Files: files, Seeds: own, Why: ps.whole,
			})
			continue
		}
		if len(ps.unfiltered) > 0 {
			argv := append([]string{}, base...)
			var units []Unit
			for _, arg := range sortedKeys(ps.unfiltered) {
				argv = append(argv, strings.Fields(arg)...)
				units = append(units, Unit{Kind: "rust", Pkg: name, Target: arg})
			}
			out = append(out, Selected{
				Command: logged(newCommand(s.in.RustDir, append(argv, "--no-fail-fast")...)),
				Units:   units, Files: files, Seeds: own, Why: "blast tests",
			})
		}
		for _, arg := range sortedKeys(ps.filtered) {
			if ps.unfiltered[arg] {
				continue
			}
			filters := sortedKeys(ps.filtered[arg])
			argv := append(append([]string{}, base...), strings.Fields(arg)...)
			fallback := append(append([]string{}, argv...), "--no-fail-fast")
			argv = append(append(argv, "--no-fail-fast", "--"), filters...)
			cmd := logged(newCommand(s.in.RustDir, argv...))
			cmd.Fallback = fallback
			out = append(out, Selected{
				Command: cmd,
				Units:   []Unit{{Kind: "rust", Pkg: name, Target: arg, Filters: filters}}, Files: files, Seeds: own, Why: "blast tests",
			})
		}
	}
	// Doctests are a separate cargo target the blast never names. A library
	// whose source is selected has its doc examples selected with it.
	for _, name := range sortedKeys(s.rust) {
		ps := s.rust[name]
		if ps.whole != "" || !(ps.unfiltered["--lib"] || len(ps.filtered["--lib"]) > 0) {
			continue
		}
		for _, t := range ps.pkg.Targets {
			if t.isLib() && t.Doctest && t.Test {
				out = append(out, Selected{
					Command: logged(newCommand(s.in.RustDir, "cargo", "test", "--locked", "-p", name, "--doc", "--no-fail-fast")),
					Units:   []Unit{{Kind: "rust", Pkg: name, Target: "--doc"}},
					Files:   sortedKeys(ps.files), Seeds: seedsUnder(seeds, func(file string) bool { return strings.HasPrefix(file, ps.pkg.Dir+"/") }),
					Why: "doc examples of the selected library",
				})
				break
			}
		}
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
