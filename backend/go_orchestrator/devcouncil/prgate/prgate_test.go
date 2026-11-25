package prgate

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/dc/redact"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/verify"
)

const (
	testBase = "1111111111111111111111111111111111111111"
	testHead = "2222222222222222222222222222222222222222"
)

func cleanBlast() *devmap.Blast {
	return &devmap.Blast{
		Change:       testBase + ".." + testHead,
		Complete:     true,
		ChangedFiles: []devmap.BlastChangedFile{{Path: "pkg/a.go", Status: "modified"}},
		Seeds:        []devmap.BlastSeed{{FilePath: "pkg/a.go", QualifiedName: "SECRET_SYMBOL_NAME_DO_NOT_LEAK", StartLine: 1, EndLine: 5}},
		Impacted:     []devmap.BlastImpact{{Distance: 1, FilePath: "pkg/b.go", QualifiedName: "IMPACTED_SYMBOL_DO_NOT_LEAK"}},
		Tests:        []devmap.BlastTest{{Distance: 1, Path: "pkg/a_test.go"}},
		Unattributed: []devmap.BlastUnattributed{},
		Unavailable:  []devmap.BlastUnavailable{},
	}
}

func cleanRange() verify.RangeResult {
	return verify.RangeResult{
		Status:       "verified",
		Passed:       true,
		GateMode:     "enforce",
		BaseSHA:      testBase,
		HeadSHA:      testHead,
		ChangedFiles: []string{"pkg/a.go"},
		RigorApplied: []string{"secret_scan", "stub_detection", "substance"},
	}
}

func TestQualifyBlastContractTable(t *testing.T) {
	depth, reached := 3, 250

	// 1. Notices only (NOT degraded): impact_incomplete, affected_tests_incomplete, owners_unavailable, outside_every_symbol.
	b := cleanBlast()
	b.Complete = false
	b.Unavailable = []devmap.BlastUnavailable{
		{Kind: "impact_incomplete", Depth: &depth, Reached: &reached},
		{Kind: "affected_tests_incomplete"},
		{Kind: "owners_unavailable"},
		{Kind: "change_unattributed"},
	}
	b.Unattributed = []devmap.BlastUnattributed{
		{Path: "pkg/const.go", Reason: "outside_every_symbol", NearestSymbol: "PRIVATE_CONST_SYMBOL"},
	}
	q := QualifyBlast(b, nil, testHead)
	if q.Error || len(q.Degraded) != 0 || len(q.Notices) != 4 {
		t.Fatalf("notice-only blast: err=%t degraded=%+v notices=%+v", q.Error, q.Degraded, q.Notices)
	}
	// NearestSymbol must never appear in notices.
	for _, n := range q.Notices {
		if strings.Contains(n.Message, "PRIVATE_CONST_SYMBOL") {
			t.Fatalf("leaked NearestSymbol in notice: %+v", n)
		}
	}

	// 2. Degraded unattributed reasons: file_not_indexed, basis_mismatch, content_unavailable, beyond_end_of_file, unknown.
	for _, reason := range []string{"file_not_indexed", "basis_mismatch", "content_unavailable", "beyond_end_of_file", "future_unknown_reason"} {
		t.Run("unattributed_"+reason, func(t *testing.T) {
			b := cleanBlast()
			b.Unavailable = []devmap.BlastUnavailable{{Kind: "change_unattributed"}}
			b.Unattributed = []devmap.BlastUnattributed{{Path: "pkg/x.go", Reason: reason}}
			q := QualifyBlast(b, nil, testHead)
			if q.Error || len(q.Degraded) != 1 {
				t.Fatalf("reason %s: err=%t degraded=%+v", reason, q.Error, q.Degraded)
			}
		})
	}

	// 3. Unknown unavailable kind -> degraded.
	bUnknown := cleanBlast()
	bUnknown.Unavailable = []devmap.BlastUnavailable{{Kind: "new_future_limit"}}
	qUnknown := QualifyBlast(bUnknown, nil, testHead)
	if qUnknown.Error || len(qUnknown.Degraded) != 1 || qUnknown.Degraded[0].Code != BlastDegUnknownUnavailable {
		t.Fatalf("unknown unavailable: err=%t degraded=%+v", qUnknown.Error, qUnknown.Degraded)
	}

	// 4. Error cases: blastErr, head mismatch, unattributed disagreement, invalid path.
	if q := QualifyBlast(nil, errors.New("decode failed"), testHead); !q.Error || q.ErrorCode != BlastErrUnavailable {
		t.Fatalf("blastErr: %+v", q)
	}
	bWrongHead := cleanBlast()
	bWrongHead.Change = testBase + ".." + strings.Repeat("9", 40)
	if q := QualifyBlast(bWrongHead, nil, testHead); !q.Error || q.ErrorCode != BlastErrHeadMismatch {
		t.Fatalf("head mismatch: %+v", q)
	}
	bDisagree := cleanBlast()
	bDisagree.Unavailable = []devmap.BlastUnavailable{{Kind: "change_unattributed"}}
	bDisagree.Unattributed = nil
	if q := QualifyBlast(bDisagree, nil, testHead); !q.Error || q.ErrorCode != BlastErrUnattributedMismatch {
		t.Fatalf("unattributed disagree: %+v", q)
	}
	bTraversal := cleanBlast()
	bTraversal.Tests = []devmap.BlastTest{{Distance: 1, Path: "../escape_test.go"}}
	if q := QualifyBlast(bTraversal, nil, testHead); !q.Error || q.ErrorCode != BlastErrInvalidPath {
		t.Fatalf("traversal path: %+v", q)
	}
}

func TestEvaluateExitCodesAndFailClosedContract(t *testing.T) {
	// Pass -> 0, status success.
	rep := Evaluate(EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: cleanRange(), Blast: cleanBlast(),
	})
	if rep.Outcome != OutcomeSuccess || rep.ExitCode != ExitPass || rep.StatusState != "success" || !rep.ShouldPost {
		t.Fatalf("pass report = %+v", rep)
	}
	if rep.Coverage.Status != "not measured" {
		t.Fatalf("coverage status = %q, want not measured", rep.Coverage.Status)
	}

	// Blocking finding -> 1, status failure.
	f := "pkg/a.go"
	ln := 10
	rngFail := cleanRange()
	rngFail.Status = "blocked"
	rngFail.Passed = false
	rngFail.BlockingGapCount = 1
	rngFail.Gaps = []verify.Gap{{
		ID: "GAP-1", Severity: "critical", GapType: "security_risk",
		Description: "RAW_DESC_SECRET_VALUE", Evidence: []string{"token=[REDACTED]"},
		RecommendedFix: "RAW_FIX_SECRET", Blocking: true, File: &f, Line: &ln,
	}}
	repFail := Evaluate(EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: rngFail, Blast: cleanBlast(),
	})
	if repFail.Outcome != OutcomeFailure || repFail.ExitCode != ExitFail || repFail.StatusState != "failure" {
		t.Fatalf("fail report = %+v", repFail)
	}

	// Degraded -> 2, status error (NEVER neutral!).
	rngDeg := cleanRange()
	rngDeg.Status = "degraded"
	rngDeg.Passed = false
	rngDeg.Degraded = []verify.RangeDegraded{{Code: verify.DegradedSensitivePathChanged, Path: ".gitattributes"}}
	repDeg := Evaluate(EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: rngDeg, Blast: cleanBlast(),
	})
	if repDeg.Outcome != OutcomeError || repDeg.ExitCode != ExitError || repDeg.StatusState != "error" {
		t.Fatalf("degraded report = %+v", repDeg)
	}

	// Mode off -> skipped, ShouldPost=false, StatusState="", exit 0.
	repOff := Evaluate(EvalInput{Mode: "off", BaseSHA: testBase, HeadSHA: testHead})
	if repOff.Outcome != OutcomeSkipped || repOff.ExitCode != ExitPass || repOff.ShouldPost || repOff.StatusState != "" {
		t.Fatalf("off report = %+v", repOff)
	}

	// Checkout head mismatch -> error, exit 2.
	repMismatch := Evaluate(EvalInput{Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, CheckoutMismatch: true})
	if repMismatch.Outcome != OutcomeError || repMismatch.ExitCode != ExitError || repMismatch.ErrorCode != "checkout_head_mismatch" {
		t.Fatalf("mismatch report = %+v", repMismatch)
	}

	// Defence in depth (#5): Range.Status != "verified" or !Range.Passed must never yield success.
	rngNotVerified := cleanRange()
	rngNotVerified.Status = "degraded"
	rngNotVerified.Passed = false
	repNotVerified := Evaluate(EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: rngNotVerified, Blast: cleanBlast(),
	})
	if repNotVerified.Outcome != OutcomeError || repNotVerified.ExitCode != ExitError || repNotVerified.ErrorCode != "range_not_verified" {
		t.Fatalf("non-verified range yielded %+v", repNotVerified)
	}

	// Cross-check (#6): blast.ChangedFiles disagrees with range.ChangedFiles -> error, exit 2.
	bMismatchFiles := cleanBlast()
	bMismatchFiles.ChangedFiles = []devmap.BlastChangedFile{{Path: "pkg/other.go", Status: "modified"}}
	repFilesMismatch := Evaluate(EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: cleanRange(), Blast: bMismatchFiles,
	})
	if repFilesMismatch.Outcome != OutcomeError || repFilesMismatch.ExitCode != ExitError || repFilesMismatch.ErrorCode != "blast_changed_files_mismatch" {
		t.Fatalf("blast changed files mismatch yielded %+v", repFilesMismatch)
	}
}

func TestPrivacyNoSecretOrSymbolOrStderrLeaks(t *testing.T) {
	path := "pkg/a.go"
	secretPath := ".env.production"
	line := 7
	rng := cleanRange()
	rng.Status = "blocked"
	rng.Passed = false
	rng.RigorErrorDetail = "STDERR_LEAK_SENTINEL"
	rng.Gaps = []verify.Gap{
		{
			ID: "GAP-STUB", Severity: "high", GapType: "stub_detected",
			Description: "STUB_DESC_SENTINEL", Evidence: []string{"PR_SOURCE_LINE_SENTINEL"},
			RecommendedFix: "FIX_SENTINEL", Blocking: true, File: &path, Line: &line,
		},
		{
			ID: "GAP-SEC", Severity: "critical", GapType: "security_risk",
			Description: "SEC_DESC_SENTINEL", Evidence: []string{"SECRET_EVIDENCE_SENTINEL"},
			RecommendedFix: "SEC_FIX_SENTINEL", Blocking: true, File: &secretPath, Line: &line,
		},
	}

	b := cleanBlast()
	b.Unavailable = []devmap.BlastUnavailable{{Kind: "change_unattributed"}}
	b.Unattributed = []devmap.BlastUnattributed{{
		Path: "pkg/a.go", Reason: "outside_every_symbol", NearestSymbol: "NEAREST_SYMBOL_SENTINEL",
	}}

	rep := Evaluate(EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: rng, Blast: b,
	})

	md := RenderMarkdown(rep)
	sarifBytes, err := RenderSARIF(rep)
	if err != nil {
		t.Fatal(err)
	}
	sarif := string(sarifBytes)
	ann := strings.Join(RenderAnnotations(rep), "\n")
	jsonBytes, err := RenderJSON(rep)
	if err != nil {
		t.Fatal(err)
	}
	js := string(jsonBytes)

	leaks := []string{
		"SECRET_SYMBOL_NAME_DO_NOT_LEAK",
		"IMPACTED_SYMBOL_DO_NOT_LEAK",
		"NEAREST_SYMBOL_SENTINEL",
		"STUB_DESC_SENTINEL",
		"PR_SOURCE_LINE_SENTINEL",
		"FIX_SENTINEL",
		"SEC_DESC_SENTINEL",
		"SEC_FIX_SENTINEL",
		"STDERR_LEAK_SENTINEL",
		"SECRET_EVIDENCE_SENTINEL",
		".env.production",
	}
	for _, sentinel := range leaks {
		if strings.Contains(md, sentinel) {
			t.Errorf("Markdown leaked %q:\n%s", sentinel, md)
		}
		if strings.Contains(sarif, sentinel) {
			t.Errorf("SARIF leaked %q:\n%s", sentinel, sarif)
		}
		if strings.Contains(ann, sentinel) {
			t.Errorf("Annotations leaked %q:\n%s", sentinel, ann)
		}
		if strings.Contains(js, sentinel) {
			t.Errorf("JSON leaked %q:\n%s", sentinel, js)
		}
	}
	if !strings.HasPrefix(md, StickyMarker+"\n") {
		t.Fatalf("Markdown missing sticky marker prefix")
	}
}

func TestHostileFileNamesEscapedAcrossAllRenderers(t *testing.T) {
	hostilePaths := []string{
		"dir/nl\n::error::pwn.go",
		"dir/cr\r::add-mask::secret.go",
		"dir/pct%0A%25file.go",
		"dir/backtick`inject`.go",
		"dir/mention@victim.go",
		"dir/entity&#64;team.go",
		"www.evil.tld/x.go",
		"dir/html<details><script>alert(1)</script>.go",
		"dir/table|break|[link](evil.test).go",
		"../etc/passwd",
		"/etc/shadow",
	}

	rng := cleanRange()
	rng.Status = "blocked"
	rng.Passed = false
	for i, hp := range hostilePaths {
		p := hp
		ln := i + 1
		rng.Gaps = append(rng.Gaps, verify.Gap{
			ID: fmt.Sprintf("GAP-%d", i), Severity: "high", GapType: "stub_detected",
			Blocking: true, File: &p, Line: &ln,
		})
	}

	rep := Evaluate(EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: rng, Blast: cleanBlast(),
	})

	md := RenderMarkdown(rep)
	for _, forbidden := range []string{"<script>", "<details>", "::error::pwn", "::add-mask::", "../etc/passwd", "/etc/shadow"} {
		if strings.Contains(md, forbidden) {
			t.Errorf("Markdown contains unescaped hostile sequence %q:\n%s", forbidden, md)
		}
	}
	// Every path must be rendered inside a backtick code span so @mentions,
	// HTML entities, autolinks (www.evil.tld/x), and emphasis never fire.
	for _, wantCodeSpan := range []string{
		"`dir/mention@victim.go:5`",
		"`dir/entity&#64;team.go:6`",
		"`www.evil.tld/x.go:7`",
		"`dir/backtick\\x60inject\\x60.go:4`",
		"`dir/table\\|break\\|[link](evil.test).go:9`",
	} {
		if !strings.Contains(md, wantCodeSpan) {
			t.Errorf("Markdown missing code-span escaped location %q:\n%s", wantCodeSpan, md)
		}
	}

	anns := RenderAnnotations(rep)
	for _, a := range anns {
		if strings.ContainsAny(a, "\r\n") {
			t.Errorf("annotation contains raw line break: %q", a)
		}
		if strings.Contains(a, "../etc/passwd") || strings.Contains(a, "/etc/shadow") {
			t.Errorf("annotation emitted traversal/absolute path: %q", a)
		}
	}

	sarifBytes, err := RenderSARIF(rep)
	if err != nil {
		t.Fatal(err)
	}
	var parsed map[string]any
	if err := json.Unmarshal(sarifBytes, &parsed); err != nil {
		t.Fatalf("SARIF invalid JSON: %v", err)
	}
	if parsed["version"] != "2.1.0" {
		t.Fatalf("SARIF version = %v, want 2.1.0", parsed["version"])
	}
	if strings.Contains(string(sarifBytes), "../etc/passwd") || strings.Contains(string(sarifBytes), "/etc/shadow") {
		t.Fatalf("SARIF emitted traversal/absolute URI:\n%s", sarifBytes)
	}
}

func TestDeterminismAndCaps(t *testing.T) {
	rng := cleanRange()
	rng.Status = "blocked"
	rng.Passed = false
	for i := 0; i < 120; i++ {
		p := fmt.Sprintf("pkg/very_long_directory_name_for_cap_testing_%03d/file_%03d.go", i, i)
		ln := i + 1
		rng.Gaps = append(rng.Gaps, verify.Gap{
			ID: fmt.Sprintf("GAP-%03d", i), Severity: "high", GapType: "stub_detected",
			Blocking: true, File: &p, Line: &ln,
		})
	}
	in := EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: rng, Blast: cleanBlast(),
	}
	r1 := Evaluate(in)
	r2 := Evaluate(in)

	j1, _ := RenderJSON(r1)
	j2, _ := RenderJSON(r2)
	if !bytes.Equal(j1, j2) {
		t.Fatal("RenderJSON is not deterministic")
	}
	m1, m2 := RenderMarkdown(r1), RenderMarkdown(r2)
	if m1 != m2 {
		t.Fatal("RenderMarkdown is not deterministic")
	}
	if len(m1) >= 65536 {
		t.Fatalf("Markdown length %d >= 65536", len(m1))
	}
	s1, _ := RenderSARIF(r1)
	s2, _ := RenderSARIF(r2)
	if !bytes.Equal(s1, s2) {
		t.Fatal("RenderSARIF is not deterministic")
	}
	anns := RenderAnnotations(r1)
	if len(anns) > MaxAnnotations {
		t.Fatalf("annotations count %d > %d", len(anns), MaxAnnotations)
	}
}

func TestRenderScrubbedCatchesPlantedTokensAndFailsClosed(t *testing.T) {
	ctx := context.Background()
	awsKey := "AKIA" + "IOSFODNN7EXAMPLE"
	ghpTok := "ghp_" + strings.Repeat("A1b2C3d4", 4) + "abcd"
	pemHdr := "-----BEGIN " + "RSA PRIVATE KEY-----"

	// Plant secret-like tokens directly in a finding path and security evidence
	// to verify defense-in-depth egress scrubbing across every output surface.
	plantedPath := "fixtures/" + awsKey + "/" + ghpTok + ".go"
	line := 4
	rng := cleanRange()
	rng.Status = "blocked"
	rng.Passed = false
	rng.ChangedFiles = []string{plantedPath}
	rng.Gaps = []verify.Gap{{
		ID:       "GAP-PLANTED",
		Severity: "critical",
		GapType:  "security_risk",
		Evidence: []string{"raw " + awsKey + " and " + pemHdr},
		Blocking: true,
		File:     &plantedPath,
		Line:     &line,
	}}
	b := cleanBlast()
	b.ChangedFiles = []devmap.BlastChangedFile{{Path: plantedPath, Status: "modified"}}
	rep := Evaluate(EvalInput{
		Mode: "enforce", BaseSHA: testBase, HeadSHA: testHead, MergeBaseSHA: testBase,
		Range: rng, Blast: b,
	})

	// 1. Pure-Go fallback scrubber (nil Scrubber) must strip every planted token
	// across Markdown, SARIF, JSON, and annotations while preserving validity.
	out, err := RenderScrubbed(ctx, rep, nil)
	if err != nil {
		t.Fatalf("RenderScrubbed(fallback) error: %v", err)
	}
	for _, secret := range []string{awsKey, ghpTok, pemHdr} {
		if strings.Contains(out.Markdown, secret) {
			t.Errorf("scrubbed Markdown leaked %q:\n%s", secret, out.Markdown)
		}
		if strings.Contains(string(out.SARIF), secret) {
			t.Errorf("scrubbed SARIF leaked %q:\n%s", secret, out.SARIF)
		}
		if strings.Contains(string(out.JSON), secret) {
			t.Errorf("scrubbed JSON leaked %q:\n%s", secret, out.JSON)
		}
		if strings.Contains(strings.Join(out.Annotations, "\n"), secret) {
			t.Errorf("scrubbed Annotations leaked %q:\n%v", secret, out.Annotations)
		}
	}
	if !strings.HasPrefix(out.Markdown, StickyMarker+"\n") {
		t.Fatalf("scrubbed Markdown lost sticky marker:\n%s", out.Markdown)
	}
	if !json.Valid(out.SARIF) {
		t.Fatalf("scrubbed SARIF is not valid JSON:\n%s", out.SARIF)
	}
	if !json.Valid(out.JSON) {
		t.Fatalf("scrubbed JSON is not valid JSON:\n%s", out.JSON)
	}

	// 2. Directly scrubbing widened Markdown/SARIF/annotations catches PEM headers too.
	widenedMD := StickyMarker + "\nleaked " + pemHdr + " and " + awsKey + "\n"
	scrubMD, err := ScrubMarkdown(ctx, widenedMD, nil)
	if err != nil {
		t.Fatalf("ScrubMarkdown error: %v", err)
	}
	if strings.Contains(scrubMD, pemHdr) || strings.Contains(scrubMD, awsKey) {
		t.Fatalf("ScrubMarkdown leaked planted token: %q", scrubMD)
	}

	// 3. Failing/crashing dcredact binary must fail closed with error.
	if runtime.GOOS != "windows" {
		badBin := filepath.Join(t.TempDir(), "dcredact")
		if err := os.WriteFile(badBin, []byte("#!/bin/sh\necho crash >&2\nexit 1\n"), 0o755); err != nil {
			t.Fatal(err)
		}
		failScrubber := &redact.Scrubber{Client: redact.New(badBin), RequireBinary: true}
		if _, err := RenderScrubbed(ctx, rep, failScrubber); err == nil {
			t.Fatal("expected RenderScrubbed to fail closed when dcredact crashes")
		}
	}
}
