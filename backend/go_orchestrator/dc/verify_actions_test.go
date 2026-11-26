package dc

import "testing"

func TestGapTypeForRigorGate(t *testing.T) {
	cases := []struct {
		gate string
		want string
	}{
		{"secret_scan", "security_risk"},
		{"stub_detection", "stub_detected"},
		{"stub_allowed", "stub_declared"},
		{"assert_free_test", "assert_free_test"},
		{"skipped_test", "skipped_test"},
		{"runner_hijack", "runner_hijack"},
		{"lean_tcb", "lean_tcb"},
		{"unknown_gate", "rigor_check_unavailable"},
	}
	for _, tc := range cases {
		if got := GapTypeForRigorGate(tc.gate); got != tc.want {
			t.Errorf("GapTypeForRigorGate(%q) = %q, want %q", tc.gate, got, tc.want)
		}
	}
}

func TestCategoryForGapType(t *testing.T) {
	cases := []struct {
		gapType string
		want    string
	}{
		{"security_risk", "security"},
		{"stub_detected", "fix_code"},
		{"stub_declared", "review"},
		{"assert_free_test", "add_test"},
		{"skipped_test", "add_test"},
		{"runner_hijack", "security"},
		{"lean_tcb", "security"},
		{GateConfigInvalid, "fix_config"},
		{VerifyEnvConfigInvalid, "fix_config"},
		{"unrecognized_gap_type", "review"},
	}
	for _, tc := range cases {
		if got := CategoryForGapType(tc.gapType); got != tc.want {
			t.Errorf("CategoryForGapType(%q) = %q, want %q", tc.gapType, got, tc.want)
		}
	}
	if got := KnownGapTypes(); len(got) == 0 {
		t.Fatal("KnownGapTypes() returned empty slice")
	}
}

func TestRigorActionText(t *testing.T) {
	cases := []struct {
		gate     string
		path     string
		line     int
		fallback string
		want     string
	}{
		{
			"secret_scan", "src/auth.go", 12, "fb",
			"Remove the detected secret/finding from the diff and rotate any exposed credential.",
		},
		{
			"stub_detection", "src/handler.rs", 38, "fb",
			"Replace the stub/placeholder at src/handler.rs:38 with a real implementation, then re-verify. Do not mark work complete while placeholders remain.",
		},
		{
			"stub_detection", "", 0, "fb",
			"Replace the stub/placeholder at the affected file with a real implementation, then re-verify. Do not mark work complete while placeholders remain.",
		},
		{
			"stub_allowed", "app.go", 4, "fb",
			"Review the intentional stub declared at app.go:4; replace it before marking done.",
		},
		{
			"assert_free_test", "a_test.go", 9, "fb",
			"Make the added test assert on the behaviour it is named for, or call a helper that does.",
		},
		{
			"skipped_test", "a_test.go", 15, "fb",
			"Remove the unconditional skip, or make the skip conditional on what the test needs.",
		},
		{
			"runner_hijack", "conftest.py", 2, "fb",
			"Remove the test-runner hijack in conftest.py so the real test harness executes and reports failures honestly.",
		},
		{
			"lean_tcb", "", 1, "fb",
			"Replace the Lean 4 TCB soundness hole, metaprogramming escape, or non-standard axiom/extern hook in the affected file with a kernel- and lean4checker-verified proof, then re-verify.",
		},
		{
			"unknown", "x.go", 1, "custom fallback",
			"custom fallback",
		},
	}
	for _, tc := range cases {
		if got := RigorActionText(tc.gate, tc.path, tc.line, tc.fallback); got != tc.want {
			t.Errorf("RigorActionText(%q, %q, %d, %q) = %q, want %q",
				tc.gate, tc.path, tc.line, tc.fallback, got, tc.want)
		}
	}
}
