package devmap

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

// subcommandLog is a stand-in devmap that logs every non-probe invocation's
// subcommand to a file, so a test can state how many processes a call started.
func subcommandLog(t *testing.T, statusReply string) (*Client, func() []string) {
	t.Helper()
	dir := t.TempDir()
	log := filepath.Join(dir, "calls.log")
	script := "#!/bin/sh\n" +
		"case \" $* \" in *\" --help \"*) printf 'usage\\n      --graph-output <PATH>\\n'; exit 0;; esac\n" +
		"echo \"$2\" >> " + log + "\n" +
		"case \"$2\" in\n" +
		"  status) cat <<'JSON'\n" + statusReply + "\nJSON\n;;\n" +
		"  *) echo '{\"items\":[{\"source_file\":\"a.go\",\"target_file\":\"b.go\"}],\"hidden\":0}';;\n" +
		"esac\n"
	path := filepath.Join(dir, "devmap")
	if err := os.WriteFile(path, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	calls := func() []string {
		raw, err := os.ReadFile(log)
		if err != nil {
			return nil
		}
		return strings.Fields(string(raw))
	}
	return New(path, dir), calls
}

const scopeHealthyStatus = `{"generation_id":3,"node_count":10,"is_fresh":true,"degraded_reason":null}`

func countSub(calls []string, sub string) int {
	n := 0
	for _, c := range calls {
		if c == sub {
			n++
		}
	}
	return n
}

// Without a call scope every Status is a process, as it always was: nothing
// about the existing behaviour changes for a caller that does not opt in.
func TestStatusWithoutACallScopeSpawnsEveryTime(t *testing.T) {
	c, calls := subcommandLog(t, scopeHealthyStatus)
	ctx := context.Background()
	for i := 0; i < 3; i++ {
		if _, err := c.Status(ctx); err != nil {
			t.Fatal(err)
		}
	}
	if got := countSub(calls(), "status"); got != 3 {
		t.Fatalf("status ran %d times, want 3", got)
	}
}

// Available, then the query's own freshness read, are two questions about one
// instant. Inside one call scope they are one process.
func TestAvailableThenQueryShareOneStatusInACallScope(t *testing.T) {
	c, calls := subcommandLog(t, scopeHealthyStatus)
	ctx := WithCall(context.Background())
	if _, err := c.Available(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Deps(ctx, "a.go"); err != nil {
		t.Fatal(err)
	}
	got := calls()
	if countSub(got, "status") != 1 || countSub(got, "deps") != 1 {
		t.Fatalf("spawns = %v, want exactly one status and one deps", got)
	}
}

// The memo is a call's, not the client's. A second call may be minutes after
// an edit, and its freshness verdict has to be about then.
func TestCallScopeDoesNotOutliveItsCall(t *testing.T) {
	c, calls := subcommandLog(t, scopeHealthyStatus)
	for i := 0; i < 3; i++ {
		ctx := WithCall(context.Background())
		if _, err := c.Available(ctx); err != nil {
			t.Fatal(err)
		}
	}
	if got := countSub(calls(), "status"); got != 3 {
		t.Fatalf("status ran %d times across three calls, want 3: a verdict was served across calls", got)
	}
}

// Parallel queries inside one call share the single status read.
func TestParallelQueriesInACallShareOneStatus(t *testing.T) {
	c, calls := subcommandLog(t, scopeHealthyStatus)
	ctx := WithCall(context.Background())
	if _, err := c.Available(ctx); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	for i := 0; i < 12; i++ {
		wg.Go(func() {
			if _, err := c.Deps(ctx, "a.go"); err != nil {
				t.Error(err)
			}
		})
	}
	wg.Wait()
	got := calls()
	if countSub(got, "status") != 1 || countSub(got, "deps") != 12 {
		t.Fatalf("spawns = %v, want one status and twelve deps", got)
	}
}

// An unhealthy index is refused whatever the scope: the memo carries the
// verdict, it does not soften it.
func TestCallScopeStillRefusesAnUnbuiltIndex(t *testing.T) {
	c, _ := subcommandLog(t, `{"generation_id":0,"node_count":0,"is_fresh":false,"degraded_reason":null}`)
	ctx := WithCall(context.Background())
	for i := 0; i < 2; i++ {
		if _, err := c.Available(ctx); err == nil {
			t.Fatalf("attempt %d: an unbuilt index was reported available", i)
		}
	}
}

// A failed status read is not remembered: the next caller in the call tries
// again rather than inheriting one transient failure.
func TestFailedStatusIsNotMemoised(t *testing.T) {
	dir := t.TempDir()
	flag := filepath.Join(dir, "ok")
	script := "#!/bin/sh\n" +
		"case \" $* \" in *\" --help \"*) printf '      --graph-output <PATH>\\n'; exit 0;; esac\n" +
		"if [ -f " + flag + " ]; then echo '" + scopeHealthyStatus + "'; else echo boom >&2; exit 3; fi\n"
	path := filepath.Join(dir, "devmap")
	if err := os.WriteFile(path, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	c := New(path, dir)
	ctx := WithCall(context.Background())
	if _, err := c.Status(ctx); err == nil {
		t.Fatal("the failing devmap reported a status")
	}
	if err := os.WriteFile(flag, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Status(ctx); err != nil {
		t.Fatalf("the failure was remembered: %v", err)
	}
}

// A scope belongs to the client that filled it.
func TestCallScopeIsPerClient(t *testing.T) {
	a, aCalls := subcommandLog(t, scopeHealthyStatus)
	b, bCalls := subcommandLog(t, scopeHealthyStatus)
	ctx := WithCall(context.Background())
	if _, err := a.Status(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := b.Status(ctx); err != nil {
		t.Fatal(err)
	}
	if countSub(aCalls(), "status") != 1 || countSub(bCalls(), "status") != 1 {
		t.Fatalf("a=%v b=%v: one client's verdict answered for the other", aCalls(), bCalls())
	}
}

// A caller waiting on the shared read gives up when its own context does.
func TestWaiterHonoursItsOwnCancellation(t *testing.T) {
	c, _ := subcommandLog(t, scopeHealthyStatus)
	scope := WithCall(context.Background())
	m := scope.Value(callKey{}).(*callMemo)
	m.slot <- struct{}{} // another caller is mid-read
	ctx, cancel := context.WithCancel(scope)
	cancel()
	if _, err := c.Status(ctx); err == nil {
		t.Fatal("a cancelled waiter was served")
	}
	<-m.slot
}
