package dcverify

// The AST stub checks, through the real dcverify binary.
//
// The Rust suites prove the detector; these prove the boundary: that the
// binary reads the post-change file from --root, that the three new gate names
// survive validate(), and that an allow-stub reason reaches this side.

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func findingsByGate(t *testing.T, c *Client, diff string) map[string][]Finding {
	t.Helper()
	result, err := c.Check(context.Background(), Request{Diff: diff})
	if err != nil {
		t.Fatalf("check: %v", err)
	}
	by := map[string][]Finding{}
	for _, f := range result.Findings {
		by[f.Gate] = append(by[f.Gate], f)
	}
	return by
}

func TestTestRigorGatesReachTheHost(t *testing.T) {
	diff := `diff --git a/src/t.rs b/src/t.rs
new file mode 100644
--- /dev/null
+++ b/src/t.rs
@@ -0,0 +1,6 @@
+#[test]
+#[ignore]
+fn skipped() { assert!(true); }
+
+#[test]
+fn hollow() { let _x = 1; }
`
	by := findingsByGate(t, client(t), diff)
	skipped, hollow := by[GateSkippedTest], by[GateAssertFreeTest]
	if len(skipped) != 1 || skipped[0].Line != 2 || skipped[0].Strength != StrengthProven {
		t.Errorf("skipped_test = %+v, want one proven finding on line 2", skipped)
	}
	if len(hollow) != 1 || hollow[0].Line != 6 || hollow[0].Strength != StrengthDerived {
		t.Errorf("assert_free_test = %+v, want one derived finding on line 6", hollow)
	}
	for _, f := range append(skipped, hollow...) {
		if f.Blocking() {
			t.Errorf("test-rigor findings are advisory: %+v", f)
		}
	}
}

// A modified file is parsed from the working tree under --root, which is what
// lets the gate tell a placeholder from the same bytes in a string literal.
func TestTheBinaryParsesTheWorkingTreeAndHonoursAllowStub(t *testing.T) {
	c := client(t)
	post := "pub fn a() -> u8 { 1 }\n" +
		"pub fn b() -> &'static str { \"todo!()\" }\n" +
		"// allow-stub: blocked on the v2 storage API\n" +
		"pub fn c() { todo!() }\n"
	if err := os.MkdirAll(filepath.Join(c.Root, "src"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(c.Root, "src", "lib.rs"), []byte(post), 0o600); err != nil {
		t.Fatal(err)
	}
	diff := `diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,4 @@
 pub fn a() -> u8 { 1 }
+pub fn b() -> &'static str { "todo!()" }
+// allow-stub: blocked on the v2 storage API
+pub fn c() { todo!() }
`
	by := findingsByGate(t, c, diff)
	if stubs := by[GateStubDetection]; len(stubs) != 0 {
		t.Errorf("a string literal and an allowed stub still produced stub findings: %+v", stubs)
	}
	allowed := by[GateStubAllowed]
	if len(allowed) != 1 || allowed[0].Line != 4 || allowed[0].Blocking() ||
		!strings.Contains(allowed[0].Message, "blocked on the v2 storage API") {
		t.Fatalf("stub_allowed = %+v, want one advisory finding on line 4 carrying the reason", allowed)
	}

	// The same diff against a working tree that does not match it: the
	// binary must not trust the file, and falls back to the substring check,
	// which cannot tell the literal from the macro and says it only derived it.
	if err := os.WriteFile(filepath.Join(c.Root, "src", "lib.rs"), []byte("// moved on\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	by = findingsByGate(t, c, diff)
	derived := 0
	for _, f := range by[GateStubDetection] {
		if f.Strength == StrengthDerived && f.Blocking() {
			derived++
		}
	}
	if derived == 0 {
		t.Errorf("a mismatched working tree was trusted, or the fallback did not run: %+v", by)
	}
}
