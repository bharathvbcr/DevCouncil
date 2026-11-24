package proc

import (
	"context"
	"errors"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestRunBoundedReturnsTheDeadlineWhenTheCallNeverDoes(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()

	blocked := make(chan struct{})
	t.Cleanup(func() { close(blocked) })
	start := time.Now()
	err, timedOut := RunBounded(ctx, func() error {
		<-blocked
		return nil
	})

	if !timedOut || err != nil {
		t.Fatalf("a call that never returns reported err=%v timedOut=%v", err, timedOut)
	}
	if elapsed := time.Since(start); elapsed > 5*time.Second {
		t.Fatalf("took %s to honour a 50ms deadline", elapsed)
	}
}

func TestRunBoundedPassesTheCallsOwnErrorThrough(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	want := errors.New("exit status 2")
	err, timedOut := RunBounded(ctx, func() error { return want })
	if timedOut || !errors.Is(err, want) {
		t.Fatalf("got err=%v timedOut=%v, want %v and false", err, timedOut, want)
	}
}

func TestRunBoundedDoesNotReportSuccessAsATimeout(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err, timedOut := RunBounded(ctx, func() error { return nil })
	if timedOut || err != nil {
		t.Fatalf("a clean call reported err=%v timedOut=%v", err, timedOut)
	}
}

func TestRunBoundedPrefersTheAnswerItAlreadyHas(t *testing.T) {
	for i := 0; i < 200; i++ {
		ctx, cancel := context.WithTimeout(context.Background(), time.Millisecond)
		done := make(chan error, 1)
		done <- nil
		err, timedOut := RunBounded(ctx, func() error { return <-done })
		cancel()
		if err != nil {
			t.Fatalf("iteration %d: %v", i, err)
		}
		_ = timedOut
	}
}

// This source check makes the shared bound a repository invariant. A direct
// cmd.Run call only starts CommandContext's bound after fork succeeds, so a
// wedged Start can outlive the timeout printed by its caller.
func TestEverySubprocessBoundaryUsesTheBound(t *testing.T) {
	root, err := filepath.Abs("../..")
	if err != nil {
		t.Fatal(err)
	}
	var offenders []string
	err = filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".go") || strings.HasSuffix(path, "_test.go") {
			return nil
		}
		body, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(root, path)
		found, err := unboundedCmdRuns(rel, body)
		if err != nil {
			return err
		}
		offenders = append(offenders, found...)
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(offenders) > 0 {
		t.Fatalf("these calls bypass the process-wide wait bound:\n  %s", strings.Join(offenders, "\n  "))
	}
}

// unboundedCmdRuns returns one "path:text (line N)" entry for every cmd.Run()
// call in src that is not lexically inside a func literal passed directly to
// RunBounded or RunBoundedWithCleanup (bare or as proc.X). Parsing instead of
// matching text means comments and strings never count, and a closure that is
// merely assigned to a variable does not count as bounded.
func unboundedCmdRuns(filename string, src []byte) ([]string, error) {
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, filename, src, parser.SkipObjectResolution)
	if err != nil {
		return nil, err
	}
	lines := strings.Split(string(src), "\n")
	isBound := func(fn ast.Expr) bool {
		switch f := fn.(type) {
		case *ast.Ident:
			return f.Name == "RunBounded" || f.Name == "RunBoundedWithCleanup"
		case *ast.SelectorExpr:
			pkg, ok := f.X.(*ast.Ident)
			return ok && pkg.Name == "proc" && (f.Sel.Name == "RunBounded" || f.Sel.Name == "RunBoundedWithCleanup")
		}
		return false
	}
	var offenders []string
	// stack holds the ancestors of the node being visited; a nil visit pops.
	var stack []ast.Node
	ast.Inspect(file, func(n ast.Node) bool {
		if n == nil {
			stack = stack[:len(stack)-1]
			return true
		}
		if call, ok := n.(*ast.CallExpr); ok && len(call.Args) == 0 {
			if sel, ok := call.Fun.(*ast.SelectorExpr); ok && sel.Sel.Name == "Run" {
				if recv, ok := sel.X.(*ast.Ident); ok && recv.Name == "cmd" && !boundedByClosure(stack, isBound) {
					line := fset.Position(call.Pos()).Line
					offenders = append(offenders, filename+":"+strings.TrimSpace(lines[line-1])+" (line "+itoa(line)+")")
				}
			}
		}
		stack = append(stack, n)
		return true
	})
	return offenders, nil
}

// boundedByClosure reports whether the innermost func literal around the
// visited node is itself a direct argument of a bounding call.
func boundedByClosure(stack []ast.Node, isBound func(ast.Expr) bool) bool {
	for i := len(stack) - 1; i >= 0; i-- {
		lit, ok := stack[i].(*ast.FuncLit)
		if !ok {
			continue
		}
		if i == 0 {
			return false
		}
		call, ok := stack[i-1].(*ast.CallExpr)
		if !ok || !isBound(call.Fun) {
			return false
		}
		for _, arg := range call.Args {
			if arg == ast.Expr(lit) {
				return true
			}
		}
		return false
	}
	return false
}

func TestUnboundedCmdRunsScanner(t *testing.T) {
	const head = "package p\nfunc f() error {\n"
	cases := []struct {
		name string
		body string
		want int
	}{
		{"bare call is flagged", "return cmd.Run()\n", 1},
		{"closure passed to RunBoundedWithCleanup is accepted", "err, _ := proc.RunBoundedWithCleanup(ctx, func() error {\nreturn cmd.Run()\n})\nreturn err\n", 0},
		{"closure passed to RunBounded is accepted", "err, _ := RunBounded(ctx, func() error { return cmd.Run() })\nreturn err\n", 0},
		{"assigned closure is still flagged", "g := func() error { return cmd.Run() }\nreturn g()\n", 1},
		{"closure passed to another call is flagged", "return other(ctx, func() error { return cmd.Run() })\n", 1},
		{"goroutine inside a bounded closure is flagged", "RunBounded(ctx, func() error {\ngo func() { _ = cmd.Run() }()\nreturn nil\n})\nreturn nil\n", 1},
		{"named function passed to RunBounded is flagged", "NAMED", 1},
		{"line comment is ignored", "// return cmd.Run()\nreturn nil\n", 0},
		{"block comment and string are ignored", "/* cmd.Run() */\ns := \"cmd.Run()\"\n_ = s\nreturn nil\n", 0},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			src := head + tc.body + "}\n"
			if tc.body == "NAMED" {
				src = "package p\nfunc run() error { return cmd.Run() }\nfunc f() error {\nerr, _ := proc.RunBounded(ctx, run)\nreturn err\n}\n"
			}
			got, err := unboundedCmdRuns("x.go", []byte(src))
			if err != nil {
				t.Fatal(err)
			}
			if len(got) != tc.want {
				t.Fatalf("want %d offenders, got %d: %v", tc.want, len(got), got)
			}
		})
	}
}

// TestEverySubprocessBoundaryIsGroupIsolated is the companion invariant to the
// one above, and it exists because the bound alone was not enough.
//
// CommandContext kills the direct child and nothing else. A child that started
// its own children leaves them holding the inherited stdout pipe, os/exec's
// copy goroutine blocks on an EOF that never arrives, and Wait does not return
// — so the call outlives the deadline its caller printed. Group isolation is
// what closes that, and it had been applied at only two of the twelve places
// this repository execs something: the shell tool and the MCP client. The
// store, the verifier, the repo map, the searcher, the incumbent CLI, both git
// paths and the quick actions each ran a child the deadline could not fully
// reach.
//
// A lesson applied at some call sites is a lesson the next call site is
// written without, so it is asserted over all of them here.
func TestEverySubprocessBoundaryIsGroupIsolated(t *testing.T) {
	root, err := filepath.Abs("../..")
	if err != nil {
		t.Fatal(err)
	}
	// Counted per file rather than by proximity. The configuration and the
	// construction are not always adjacent — in dc/store a fourteen-line
	// comment about WaitDelay sits between them, and it belongs there — so a
	// proximity window either rejects correct code or has to be widened until
	// it would accept a call belonging to some other command. What actually
	// matters is that no file execs more children than it isolates.
	//
	// Two shared helpers, and one package that still has its own. ConfigureGroup
	// is for a child bounded by one caller's deadline and arms cmd.Cancel;
	// ConfigureOwnGroup is for a child that outlives any single call — the MCP
	// servers and the store's serve sessions — and so is ended explicitly
	// rather than by a context. Both set Setpgid, which is what this test is
	// actually asserting. The shell tool in devcouncil keeps setOwnProcessGroup
	// because it merges into a SysProcAttr it also uses for other fields.
	//
	// A helper missing from this list reads exactly like a call site that never
	// isolated anything, so adding one here is part of adding one at all.
	configures := []string{
		"proc.ConfigureGroup(cmd)", "ConfigureGroup(cmd)",
		"proc.ConfigureOwnGroup(cmd)", "ConfigureOwnGroup(cmd)",
		"proc.EnforceSubprocessInvariants(cmd)", "EnforceSubprocessInvariants(cmd)",
		"setOwnProcessGroup(cmd)",
	}

	var offenders []string
	err = filepath.WalkDir(root, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".go") || strings.HasSuffix(path, "_test.go") {
			return nil
		}
		body, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		spawns, isolates := 0, 0
		for _, line := range strings.Split(string(body), "\n") {
			trimmed := strings.TrimSpace(line)
			if strings.HasPrefix(trimmed, "//") {
				continue
			}
			if strings.Contains(trimmed, "exec.CommandContext(") {
				spawns++
			}
			for _, marker := range configures {
				if strings.Contains(trimmed, marker) {
					isolates++
					break
				}
			}
		}
		if spawns > isolates {
			rel, _ := filepath.Rel(root, path)
			offenders = append(offenders, rel+": "+itoa(spawns)+" child(ren) started, "+
				itoa(isolates)+" isolated")
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(offenders) > 0 {
		t.Fatalf("these children can outlive their deadline, because killing them "+
			"does not reach what they started:\n  %s", strings.Join(offenders, "\n  "))
	}
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var digits []byte
	for n > 0 {
		digits = append([]byte{byte('0' + n%10)}, digits...)
		n /= 10
	}
	return string(digits)
}

func TestCleanupGraceWaitsForReapingButRemainsBounded(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	entered, reaped := make(chan struct{}), make(chan struct{})
	start := time.Now()
	_, timeout := RunBoundedWithCleanup(ctx, func() error {
		close(entered)
		cancel()
		<-ctx.Done()
		time.Sleep(20 * time.Millisecond)
		close(reaped)
		return ctx.Err()
	})
	if !timeout {
		t.Fatal("cancellation was reported as completion")
	}
	select {
	case <-reaped:
	default:
		t.Fatal("returned before the child cleanup completed")
	}
	if time.Since(start) > time.Second {
		t.Fatal("cleanup exceeded its bound")
	}
	blocked := make(chan struct{})
	defer close(blocked)
	start = time.Now()
	_, timeout = RunBoundedWithCleanup(ctx, func() error { <-blocked; return nil })
	if !timeout || time.Since(start) > time.Second {
		t.Fatal("stuck cleanup blocked cancellation")
	}
}
