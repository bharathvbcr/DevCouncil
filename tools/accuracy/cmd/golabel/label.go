package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"go/ast"
	"go/token"
	"go/types"
	"io/fs"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/tools/go/packages"
)

// language is the only language this oracle labels.
const language = "go"

// RecallLabel is one statically bound call site, or one function or method
// used as a value: the scorer counts it present when DevMap has a use edge
// from Caller to any id in Truth (`a|b`).
type RecallLabel struct {
	Language  string `json:"language"`
	Construct string `json:"construct"`
	Caller    string `json:"caller"`
	Name      string `json:"name"`
	Truth     string `json:"truth"`
	// iface is the callee's interface key, so its implementations join Truth
	// once every module is loaded (a later module can add one).
	iface string
}

// Use is one (caller, callee) pair go/types bound. Impls lists the in-repo
// implementations of Callee when it is an interface method.
type Use struct {
	Caller string   `json:"caller"`
	Callee string   `json:"callee"`
	Impls  []string `json:"impls"`
}

// Oracle is what go/types bound, for every caller it read.
type Oracle struct {
	Language       string   `json:"language"`
	CoveredCallers []string `json:"covered_callers"`
	Uses           []Use    `json:"uses"`
}

// DeadLabel is a hand-labelled dead row, merged verbatim.
type DeadLabel struct {
	Language  string `json:"language"`
	ID        string `json:"id"`
	TrulyDead bool   `json:"truly_dead"`
}

// Labels is the labels file the corpus runner reads.
type Labels struct {
	Recall []RecallLabel `json:"recall"`
	Oracle []Oracle      `json:"oracle"`
	Dead   []DeadLabel   `json:"dead"`
}

// Encode is the labels as indented JSON with a trailing newline, the same
// bytes for the same corpus.
func (l *Labels) Encode() ([]byte, error) {
	if l.Dead == nil {
		l.Dead = []DeadLabel{}
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetIndent("", "  ")
	enc.SetEscapeHTML(false)
	if err := enc.Encode(l); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// maxReportedErrors bounds how many package errors a refusal lists.
const maxReportedErrors = 20

type useKey struct{ caller, callee string }

type labeller struct {
	root string
	fset *token.FileSet
	// seen holds every corpus file already walked: a test variant of a
	// package repeats its production files, and a file is labelled once.
	seen    map[string]bool
	covered map[string]bool
	// uses maps each bound pair to the interface key of its callee ("" for a
	// concrete function or method), resolved into impls at the end.
	uses   map[useKey]string
	impls  map[string]map[string]bool
	recall []RecallLabel
}

// Label loads every Go module under root with its tests and labels it.
func Label(root string) (*Labels, error) {
	abs, err := filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	if abs, err = filepath.EvalSymlinks(abs); err != nil {
		return nil, err
	}
	modules, err := moduleDirs(abs)
	if err != nil {
		return nil, err
	}
	if len(modules) == 0 {
		return nil, fmt.Errorf("%s holds no go.mod outside vendor/ and testdata/", abs)
	}
	l := &labeller{
		root:    abs,
		seen:    map[string]bool{},
		covered: map[string]bool{},
		uses:    map[useKey]string{},
		impls:   map[string]map[string]bool{},
	}
	for _, dir := range modules {
		if err := l.loadModule(dir); err != nil {
			return nil, err
		}
	}
	return l.labels(), nil
}

// moduleDirs is every directory under root holding a go.mod, skipping the
// trees `go` itself skips (vendor, testdata, dot and underscore directories).
func moduleDirs(root string) ([]string, error) {
	var dirs []string
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			name := d.Name()
			if path != root && (name == "vendor" || name == "testdata" || name == "node_modules" ||
				strings.HasPrefix(name, ".") || strings.HasPrefix(name, "_")) {
				return filepath.SkipDir
			}
			return nil
		}
		if d.Name() == "go.mod" {
			dirs = append(dirs, filepath.Dir(path))
		}
		return nil
	})
	sort.Strings(dirs)
	return dirs, err
}

func (l *labeller) loadModule(dir string) error {
	cfg := &packages.Config{
		Mode: packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles |
			packages.NeedSyntax | packages.NeedTypes | packages.NeedTypesInfo |
			packages.NeedImports | packages.NeedDeps | packages.NeedModule,
		Dir:   dir,
		Tests: true,
	}
	roots, err := packages.Load(cfg, "./...")
	if err != nil {
		return fmt.Errorf("%s: %w", dir, err)
	}
	var all []*packages.Package
	var errs []string
	packages.Visit(roots, nil, func(p *packages.Package) {
		all = append(all, p)
		for _, e := range p.Errors {
			errs = append(errs, e.Error())
		}
	})
	if len(errs) > 0 {
		shown := errs
		if len(shown) > maxReportedErrors {
			shown = shown[:maxReportedErrors]
		}
		return fmt.Errorf("%s: %d package error(s); a corpus that does not type-check is not labelled (first %d):\n  %s",
			dir, len(errs), len(shown), strings.Join(shown, "\n  "))
	}
	if len(roots) == 0 {
		return fmt.Errorf("%s: ./... matched no package", dir)
	}
	l.fset = roots[0].Fset
	l.indexImpls(all)

	// Production packages first, so a production file is walked with the
	// production package's type information, not a test variant's copy.
	sort.SliceStable(roots, func(i, j int) bool {
		vi, vj := isVariant(roots[i]), isVariant(roots[j])
		if vi != vj {
			return !vi
		}
		return roots[i].ID < roots[j].ID
	})
	for _, p := range roots {
		for _, f := range p.Syntax {
			rel, ok := l.rel(l.fset.Position(f.Package).Filename)
			if !ok || l.seen[rel] {
				continue
			}
			l.seen[rel] = true
			l.walkFile(p.TypesInfo, f, rel)
		}
	}
	return nil
}

// isVariant reports a test variant ("p [p.test]") or a generated test main.
func isVariant(p *packages.Package) bool {
	return strings.Contains(p.ID, " [") || strings.HasSuffix(p.ID, ".test")
}

// rel is filename relative to the corpus root in DevMap's form, or false for
// a file outside it or under vendor/.
func (l *labeller) rel(filename string) (string, bool) {
	if filename == "" {
		return "", false
	}
	rel, err := filepath.Rel(l.root, filename)
	if err != nil || filepath.IsAbs(rel) || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", false
	}
	rel = filepath.ToSlash(rel)
	for _, seg := range strings.Split(rel, "/") {
		if seg == "vendor" {
			return "", false
		}
	}
	return rel, true
}

// funcID is DevMap's symbol id for fn (`<file>::<Recv.>Name`, a generic
// receiver named without its type arguments), and its interface key when fn
// is an interface method. False for a function declared outside the corpus or
// a method of an unnamed type.
func (l *labeller) funcID(fn *types.Func) (id, iface string, ok bool) {
	fn = fn.Origin()
	rel, ok := l.rel(l.fset.Position(fn.Pos()).Filename)
	if !ok {
		return "", "", false
	}
	name := fn.Name()
	sig, isSig := fn.Type().(*types.Signature)
	if !isSig {
		return "", "", false
	}
	if recv := sig.Recv(); recv != nil {
		t := types.Unalias(recv.Type())
		if ptr, isPtr := t.(*types.Pointer); isPtr {
			t = types.Unalias(ptr.Elem())
		}
		named, isNamed := t.(*types.Named)
		if !isNamed {
			return "", "", false
		}
		named = named.Origin()
		name = named.Obj().Name() + "." + name
		if types.IsInterface(named) {
			iface = ifaceKey(named, fn.Name())
		}
	}
	return rel + "::" + name, iface, true
}

func ifaceKey(named *types.Named, method string) string {
	pkg := ""
	if named.Obj().Pkg() != nil {
		pkg = named.Obj().Pkg().Path()
	}
	return pkg + "." + named.Obj().Name() + "." + method
}

// indexImpls records, for every corpus interface method, the corpus methods
// that implement it: every non-generic corpus type whose value or pointer
// method set satisfies a non-generic corpus interface. Every package of the
// load takes part, test variants included (test doubles implement
// interfaces too); a pair whose objects come from different variants fails
// types.Implements and is found again through the production copies.
func (l *labeller) indexImpls(all []*packages.Package) {
	var ifaces, concretes []*types.Named
	seen := map[*types.Named]bool{}
	for _, p := range all {
		if p.Types == nil {
			continue
		}
		scope := p.Types.Scope()
		for _, name := range scope.Names() {
			tn, ok := scope.Lookup(name).(*types.TypeName)
			if !ok || tn.IsAlias() {
				continue
			}
			named, ok := tn.Type().(*types.Named)
			if !ok || seen[named] || named.TypeParams().Len() > 0 {
				continue
			}
			if _, in := l.rel(l.fset.Position(tn.Pos()).Filename); !in {
				continue
			}
			seen[named] = true
			if it, isIface := named.Underlying().(*types.Interface); isIface {
				if it.NumMethods() > 0 {
					ifaces = append(ifaces, named)
				}
			} else {
				concretes = append(concretes, named)
			}
		}
	}
	for _, iface := range ifaces {
		it := iface.Underlying().(*types.Interface)
		for _, c := range concretes {
			ptr := types.NewPointer(c)
			if !types.Implements(c, it) && !types.Implements(ptr, it) {
				continue
			}
			for i := 0; i < it.NumMethods(); i++ {
				m := it.Method(i)
				_, key, ok := l.funcID(m)
				if !ok || key == "" {
					continue
				}
				obj, _, _ := types.LookupFieldOrMethod(ptr, false, m.Pkg(), m.Name())
				impl, isFunc := obj.(*types.Func)
				if !isFunc {
					continue
				}
				id, implIface, ok := l.funcID(impl)
				if !ok || implIface != "" {
					continue
				}
				if l.impls[key] == nil {
					l.impls[key] = map[string]bool{}
				}
				l.impls[key][id] = true
			}
		}
	}
}

func (l *labeller) walkFile(info *types.Info, f *ast.File, rel string) {
	l.covered[rel] = true
	for _, decl := range f.Decls {
		switch d := decl.(type) {
		case *ast.FuncDecl:
			fn, ok := info.Defs[d.Name].(*types.Func)
			if !ok {
				continue
			}
			caller, _, ok := l.funcID(fn)
			if !ok {
				continue
			}
			l.covered[caller] = true
			var recv *types.Var
			if d.Recv != nil && len(d.Recv.List) > 0 && len(d.Recv.List[0].Names) > 0 {
				recv, _ = info.Defs[d.Recv.List[0].Names[0]].(*types.Var)
			}
			l.walk(info, d, caller, recv)
		case *ast.GenDecl:
			// Package-level initializers belong to the file.
			l.walk(info, d, rel, nil)
		}
	}
}

// scope is what a walk knows about the variables of one top-level
// declaration: how each local was bound, and which are parameters.
type scope struct {
	info   *types.Info
	recv   *types.Var
	params map[*types.Var]bool
	bound  map[types.Object]string
}

func (l *labeller) walk(info *types.Info, root ast.Node, caller string, recv *types.Var) {
	s := &scope{info: info, recv: recv, params: map[*types.Var]bool{}, bound: map[types.Object]string{}}
	called := map[ast.Expr]bool{}
	selectorNames := map[*ast.Ident]bool{}
	ast.Inspect(root, func(n ast.Node) bool {
		switch n := n.(type) {
		case *ast.FuncType:
			if n.Params != nil {
				for _, field := range n.Params.List {
					for _, name := range field.Names {
						if v, ok := info.Defs[name].(*types.Var); ok {
							s.params[v] = true
						}
					}
				}
			}
		case *ast.AssignStmt:
			if n.Tok == token.DEFINE {
				for i, lhs := range n.Lhs {
					if id, ok := lhs.(*ast.Ident); ok && info.Defs[id] != nil {
						s.bound[info.Defs[id]] = s.bindingKind(n.Rhs, len(n.Lhs), i)
					}
				}
			}
		case *ast.RangeStmt:
			if n.Tok == token.DEFINE {
				for _, e := range []ast.Expr{n.Key, n.Value} {
					if id, ok := e.(*ast.Ident); ok && info.Defs[id] != nil {
						s.bound[info.Defs[id]] = "range var"
					}
				}
			}
		case *ast.ValueSpec:
			for i, id := range n.Names {
				obj := info.Defs[id]
				if obj == nil {
					continue
				}
				if len(n.Values) == 0 {
					s.bound[obj] = declaredKind(n.Type)
				} else {
					s.bound[obj] = s.bindingKind(n.Values, len(n.Names), i)
				}
			}
		case *ast.TypeSwitchStmt:
			for _, stmt := range n.Body.List {
				if obj := info.Implicits[stmt]; obj != nil {
					s.bound[obj] = "type switch"
				}
			}
		case *ast.CallExpr:
			called[calleeExpr(info, n.Fun)] = true
		case *ast.SelectorExpr:
			selectorNames[n.Sel] = true
		}
		return true
	})

	ast.Inspect(root, func(n ast.Node) bool {
		switch n := n.(type) {
		case *ast.Ident:
			if fn, ok := info.Uses[n].(*types.Func); ok {
				l.use(caller, fn)
				if !selectorNames[n] && !called[n] {
					l.site(caller, "func value", n.Name, fn)
				}
			}
		case *ast.SelectorExpr:
			if called[n] {
				return true
			}
			if sel := info.Selections[n]; sel != nil {
				fn, ok := sel.Obj().(*types.Func)
				if !ok {
					return true
				}
				switch sel.Kind() {
				case types.MethodVal:
					l.site(caller, "method value", n.Sel.Name, fn)
				case types.MethodExpr:
					l.site(caller, "method expression", n.Sel.Name, fn)
				}
			} else if fn, ok := info.Uses[n.Sel].(*types.Func); ok {
				l.site(caller, "func value", n.Sel.Name, fn)
			}
		case *ast.CallExpr:
			switch fe := calleeExpr(info, n.Fun).(type) {
			case *ast.Ident:
				if fn, ok := info.Uses[fe].(*types.Func); ok {
					l.site(caller, "bare same-package", fe.Name, fn)
				}
			case *ast.SelectorExpr:
				if sel := info.Selections[fe]; sel != nil {
					fn, ok := sel.Obj().(*types.Func)
					if !ok {
						return true
					}
					switch sel.Kind() {
					case types.MethodVal:
						l.site(caller, s.receiverKind(sel, fe.X), fe.Sel.Name, fn)
					case types.MethodExpr:
						l.site(caller, "method expression", fe.Sel.Name, fn)
					}
				} else if fn, ok := info.Uses[fe.Sel].(*types.Func); ok {
					l.site(caller, "package-qualified", fe.Sel.Name, fn)
				}
			}
		}
		return true
	})
}

// use records that caller binds fn, when fn is a corpus function or method.
func (l *labeller) use(caller string, fn *types.Func) {
	if id, iface, ok := l.funcID(fn); ok {
		l.uses[useKey{caller, id}] = iface
	}
}

// site records one recall row, when fn is a corpus function or method. An
// interface method's row is satisfied by an edge to the method or to any of
// its implementations, and is reported under "interface dispatch".
func (l *labeller) site(caller, construct, name string, fn *types.Func) {
	id, iface, ok := l.funcID(fn)
	if !ok {
		return
	}
	if iface != "" && construct != "method expression" && construct != "promoted" {
		construct = "interface dispatch"
	}
	l.recall = append(l.recall, RecallLabel{
		Language:  language,
		Construct: construct,
		Caller:    caller,
		Name:      name,
		Truth:     id,
		iface:     iface,
	})
}

// calleeExpr is the expression a call names: parentheses and the explicit
// instantiation of a generic function (`F[int]`) removed.
func calleeExpr(info *types.Info, fun ast.Expr) ast.Expr {
	for {
		fun = ast.Unparen(fun)
		var x ast.Expr
		switch e := fun.(type) {
		case *ast.IndexExpr:
			x = e.X
		case *ast.IndexListExpr:
			x = e.X
		default:
			return fun
		}
		if !namesFunc(info, x) {
			return fun
		}
		fun = x
	}
}

func namesFunc(info *types.Info, e ast.Expr) bool {
	switch e := ast.Unparen(e).(type) {
	case *ast.Ident:
		_, ok := info.Uses[e].(*types.Func)
		return ok
	case *ast.SelectorExpr:
		if sel := info.Selections[e]; sel != nil {
			_, ok := sel.Obj().(*types.Func)
			return ok
		}
		_, ok := info.Uses[e.Sel].(*types.Func)
		return ok
	}
	return false
}

// receiverKind is the audit's construct for a method call on x.
func (s *scope) receiverKind(sel *types.Selection, x ast.Expr) string {
	if len(sel.Index()) > 1 {
		return "promoted"
	}
	recv := sel.Recv()
	if types.IsInterface(recv) {
		return "interface dispatch"
	}
	t := types.Unalias(recv)
	if ptr, ok := t.(*types.Pointer); ok {
		t = types.Unalias(ptr.Elem())
	}
	if named, ok := t.(*types.Named); ok && named.TypeArgs().Len() > 0 {
		return "generic type"
	}
	switch x := ast.Unparen(x).(type) {
	case *ast.Ident:
		v, ok := s.info.Uses[x].(*types.Var)
		if !ok {
			return "other receiver"
		}
		if v == s.recv {
			return "own receiver"
		}
		if kind, ok := s.bound[v]; ok {
			return kind
		}
		if v.Pkg() != nil && v.Parent() == v.Pkg().Scope() {
			return "package var"
		}
		if s.params[v] {
			return "param receiver"
		}
		return "other receiver"
	case *ast.SelectorExpr:
		if fs := s.info.Selections[x]; fs != nil && fs.Kind() == types.FieldVal {
			return "field"
		}
		return "package var"
	case *ast.CallExpr:
		return "chain"
	case *ast.IndexExpr, *ast.IndexListExpr:
		return "element"
	case *ast.CompositeLit:
		return "composite literal"
	case *ast.UnaryExpr:
		if _, ok := ast.Unparen(x.X).(*ast.CompositeLit); ok {
			return "composite literal"
		}
	case *ast.TypeAssertExpr:
		return "type assertion"
	case *ast.StarExpr:
		return "dereference"
	}
	return "other receiver"
}

// bindingKind is how the i-th of n names was bound by rhs.
func (s *scope) bindingKind(rhs []ast.Expr, n, i int) string {
	if len(rhs) == 1 && n > 1 {
		return "multi-value binding"
	}
	if len(rhs) != n {
		return "other binding"
	}
	switch e := ast.Unparen(rhs[i]).(type) {
	case *ast.CompositeLit:
		return "composite binding"
	case *ast.UnaryExpr:
		if _, ok := ast.Unparen(e.X).(*ast.CompositeLit); ok {
			return "composite binding"
		}
	case *ast.CallExpr:
		if sel, ok := calleeExpr(s.info, e.Fun).(*ast.SelectorExpr); ok {
			if ms := s.info.Selections[sel]; ms != nil && ms.Kind() == types.MethodVal {
				return "method-result binding"
			}
		}
		return "call-result binding"
	case *ast.Ident, *ast.SelectorExpr:
		return "copy binding"
	case *ast.TypeAssertExpr:
		return "type-assertion binding"
	case *ast.IndexExpr:
		return "element binding"
	}
	return "other binding"
}

// declaredKind is the construct for `var x T` with no value: "var x pkg.T"
// when the type is qualified by another package, "var x T" otherwise.
func declaredKind(t ast.Expr) string {
	for {
		switch e := t.(type) {
		case *ast.StarExpr:
			t = e.X
			continue
		case *ast.IndexExpr:
			t = e.X
			continue
		case *ast.IndexListExpr:
			t = e.X
			continue
		case *ast.SelectorExpr:
			return "var x pkg.T"
		}
		return "var x T"
	}
}

func sortedKeys(set map[string]bool) []string {
	out := make([]string, 0, len(set))
	for k := range set {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func (l *labeller) labels() *Labels {
	for i, r := range l.recall {
		if r.iface != "" {
			l.recall[i].Truth = strings.Join(append([]string{r.Truth}, sortedKeys(l.impls[r.iface])...), "|")
		}
	}
	sort.SliceStable(l.recall, func(i, j int) bool {
		a, b := l.recall[i], l.recall[j]
		if a.Caller != b.Caller {
			return a.Caller < b.Caller
		}
		if a.Truth != b.Truth {
			return a.Truth < b.Truth
		}
		if a.Name != b.Name {
			return a.Name < b.Name
		}
		return a.Construct < b.Construct
	})
	uses := make([]Use, 0, len(l.uses))
	for k, iface := range l.uses {
		impls := []string{}
		if iface != "" {
			impls = sortedKeys(l.impls[iface])
		}
		uses = append(uses, Use{Caller: k.caller, Callee: k.callee, Impls: impls})
	}
	sort.Slice(uses, func(i, j int) bool {
		if uses[i].Caller != uses[j].Caller {
			return uses[i].Caller < uses[j].Caller
		}
		return uses[i].Callee < uses[j].Callee
	})
	recall := l.recall
	if recall == nil {
		recall = []RecallLabel{}
	}
	return &Labels{
		Recall: recall,
		Oracle: []Oracle{{Language: language, CoveredCallers: sortedKeys(l.covered), Uses: uses}},
	}
}
