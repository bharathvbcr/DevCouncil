package testsupport

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// captureTB records Fatalf so the contract helper's refusal paths can be
// asserted without failing this test. Helper() is a no-op because the
// helper under test already marked itself.
type captureTB struct {
	testing.TB
	fatal string
}

func (c *captureTB) Helper() {}

func (c *captureTB) Fatalf(format string, args ...any) {
	c.fatal = fmt.Sprintf(format, args...)
	panic("fatal")
}

func (c *captureTB) Fatal(args ...any) {
	c.fatal = fmt.Sprint(args...)
	panic("fatal")
}

func (c *captureTB) FailNow() {
	panic("fatal")
}

func fatalMessage(t *testing.T, fn func(testing.TB)) string {
	t.Helper()
	c := &captureTB{TB: t}
	panicked := true
	func() {
		defer func() { recover() }()
		fn(c)
		panicked = false
	}()
	if !panicked || c.fatal == "" {
		t.Fatalf("expected Fatalf, panicked=%v message=%q", panicked, c.fatal)
	}
	return c.fatal
}

func TestAssertOneDiagnosedObjectAcceptsASingleSuccessfulReply(t *testing.T) {
	AssertOneDiagnosedObject(t, "devmap", []string{"status"}, []byte(`{"ok":true,"n":1}`), 0)
}

func TestAssertOneDiagnosedObjectAcceptsADiagnosedFailure(t *testing.T) {
	AssertOneDiagnosedObject(t, "dcstore", []string{"health"}, []byte(`{"ok":false,"error":"store is locked"}`), 1)
}

func TestAssertOneDiagnosedObjectRefusesNonJSON(t *testing.T) {
	msg := fatalMessage(t, func(tb testing.TB) {
		AssertOneDiagnosedObject(tb, "dcstore", []string{"health"}, []byte("not json"), 0)
	})
	if !strings.Contains(msg, "did not print one JSON object") {
		t.Fatalf("refusal should name the missing object, got %q", msg)
	}
}

func TestAssertOneDiagnosedObjectRefusesAJSONNull(t *testing.T) {
	msg := fatalMessage(t, func(tb testing.TB) {
		AssertOneDiagnosedObject(tb, "dcstore", []string{"health"}, []byte("null"), 0)
	})
	if !strings.Contains(msg, "JSON null") {
		t.Fatalf("refusal should name the null, got %q", msg)
	}
}

func TestAssertOneDiagnosedObjectRefusesASecondValueOnStdout(t *testing.T) {
	msg := fatalMessage(t, func(tb testing.TB) {
		AssertOneDiagnosedObject(tb, "dcstore", []string{"health"}, []byte(`{"ok":true}{"ok":true}`), 0)
	})
	if !strings.Contains(msg, "more than one JSON value") {
		t.Fatalf("refusal should name the extra value, got %q", msg)
	}
}

func TestAssertOneDiagnosedObjectRefusesAReplyWithNoOkField(t *testing.T) {
	msg := fatalMessage(t, func(tb testing.TB) {
		AssertOneDiagnosedObject(tb, "dcstore", []string{"health"}, []byte(`{"error":"nope"}`), 0)
	})
	if !strings.Contains(msg, "no \"ok\" field") {
		t.Fatalf("refusal should name the missing field, got %q", msg)
	}
}

func TestAssertOneDiagnosedObjectRefusesANonZeroExitThatClaimsSuccess(t *testing.T) {
	msg := fatalMessage(t, func(tb testing.TB) {
		AssertOneDiagnosedObject(tb, "dcstore", []string{"health"}, []byte(`{"ok":true}`), 2)
	})
	if !strings.Contains(msg, "ok:true") {
		t.Fatalf("refusal should name the contradictory ok:true, got %q", msg)
	}
}

func TestAssertOneDiagnosedObjectRefusesANonZeroExitWithNoError(t *testing.T) {
	msg := fatalMessage(t, func(tb testing.TB) {
		AssertOneDiagnosedObject(tb, "dcstore", []string{"health"}, []byte(`{"ok":false,"error":"  "}`), 2)
	})
	if !strings.Contains(msg, "no error to report") {
		t.Fatalf("refusal should name the missing diagnosis, got %q", msg)
	}
}

func TestRunChildReturnsStdoutAndAZeroExit(t *testing.T) {
	script := filepath.Join(t.TempDir(), "ok.sh")
	if err := os.WriteFile(script, []byte("#!/bin/sh\ncat\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	out, code, err := RunChild(t, script, nil, `{"ok":true}`)
	if err != nil {
		t.Fatal(err)
	}
	if code != 0 {
		t.Fatalf("exit = %d, want 0", code)
	}
	if strings.TrimSpace(string(out)) != `{"ok":true}` {
		t.Fatalf("stdout = %q", out)
	}
}

func TestRunChildReturnsANonZeroExitAsADiagnosedOutcome(t *testing.T) {
	script := filepath.Join(t.TempDir(), "fail.sh")
	if err := os.WriteFile(script, []byte("#!/bin/sh\necho '{\"ok\":false,\"error\":\"denied\"}'\nexit 2\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	out, code, err := RunChild(t, script, nil, "")
	if err != nil {
		t.Fatalf("a non-zero exit is a diagnosed outcome, not a harness error: %v", err)
	}
	if code != 2 {
		t.Fatalf("exit = %d, want 2", code)
	}
	AssertOneDiagnosedObject(t, "fail.sh", nil, out, code)
}

func TestRunChildSurfacesAMissingBinary(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "no-such-binary")
	_, _, err := RunChild(t, missing, nil, "")
	if err == nil {
		t.Fatal("a missing binary must be an error, not a zero exit")
	}
}
