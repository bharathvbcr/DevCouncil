package dcverify

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// This boundary is the store's and the searcher's third sibling: one fork/exec
// per check, the answer as a single JSON document on stdout. What makes it
// worth its own adversarial pass is the shape of its success case, which is
// worse than the searcher's. An empty match list at least *means* something —
// the repository does not contain this. An empty finding list means "the secret
// scanner and the stub detector ran over this diff and it is clean", and a
// verifier that records that for gates which never ran has written "approved"
// where it should have written "unexamined".
//
// So every test below drives a binary that misbehaves in one specific way, and
// asserts the same thing: the client returned an error. Not a Result with no
// findings — an error.

// fake writes an executable standing in for dcverify.
func fake(t *testing.T, body string) string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("the fake binaries here are shell scripts")
	}
	path := filepath.Join(t.TempDir(), "fake.sh")
	// #nosec G306 -- this writes a shell script the test then execs, so the
	// owner execute bit is the point; no mode at or below 0600 would work.
	if err := os.WriteFile(path, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}

// reply builds a fake that prints one line and exits zero.
func reply(t *testing.T, json string) string {
	t.Helper()
	return fake(t, "#!/bin/sh\ncat >/dev/null\necho '"+json+"'\n")
}

// lcov writes a minimal coverage profile and returns its path. The fakes never
// read it — what matters is only that a path was supplied, which is what moves
// the client from "nothing was measured" to "this is a measurement".
func lcov(t *testing.T) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "lcov.info")
	if err := os.WriteFile(path, []byte("SF:src/a.go\nDA:1,1\nend_of_record\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	return path
}

// checkWith runs one check against the given fake.
func checkWith(t *testing.T, binary string, req Request) (*Result, error) {
	t.Helper()
	client := New(binary, t.TempDir())
	client.Timeout = 5 * time.Second
	return client.Check(context.Background(), req)
}

// A diff shaped the way every reply below claims to describe: one file, so a
// reply that classifies one file is internally consistent and the test is about
// the property under examination rather than about the file count.
const oneFileDiff = `diff --git a/src/a.go b/src/a.go
--- a/src/a.go
+++ b/src/a.go
@@ -1,0 +1,1 @@
+const answer = 42
`

// TestEveryWayTheVerifierCanMisbehaveIsAnErrorNotACleanReport is the whole
// contract in one table. Each binary below is a different way for the boundary
// to fail, and not one of them may produce a Result the caller can read as
// "the rigor gates ran and this diff is clean".
func TestEveryWayTheVerifierCanMisbehaveIsAnErrorNotACleanReport(t *testing.T) {
	cases := []struct {
		name string
		body string
	}{
		{"output that is not JSON", "#!/bin/sh\ncat >/dev/null\necho 'this is not json'\n"},
		{"nothing at all on stdout", "#!/bin/sh\ncat >/dev/null\nexit 0\n"},
		{"a non-zero exit with no explanation", "#!/bin/sh\ncat >/dev/null\nexit 1\n"},
		{"the verifier's own error reply", `#!/bin/sh
cat >/dev/null
echo '{"ok":false,"error":"the diff could not be parsed at line 3"}'
exit 2
`},
		{"an error reply with no reason given", "#!/bin/sh\ncat >/dev/null\necho '{\"ok\":false}'\nexit 2\n"},
		// The nastiest of the set: valid JSON, exit 0, and a finding list that
		// is empty because the field is absent rather than because the gates
		// found nothing. It is refused for the OK flag alone, which is why that
		// flag is not merely decorative.
		{"a well-formed reply that never says ok", `#!/bin/sh
cat >/dev/null
echo '{"files":1,"in_scope":["src/a.go"],"orphans":[],"findings":[]}'
`},
		{"a JSON array where an object belongs", "#!/bin/sh\ncat >/dev/null\necho '[]'\n"},
		{"a JSON null", "#!/bin/sh\ncat >/dev/null\necho 'null'\n"},
		{"a truncated document", "#!/bin/sh\ncat >/dev/null\nprintf '{\"ok\":true,\"files\":1,\"find'\n"},
		{"two documents where one belongs", `#!/bin/sh
cat >/dev/null
echo '{"ok":true,"files":0,"in_scope":[],"orphans":[],"findings":[]}{"ok":true,"files":99}'
`},
		{"a flood that never ends", "#!/bin/sh\ncat >/dev/null\nexec yes '{\"ok\":true,\"files\":0}'\n"},
		{"a binary that crashes on a signal", "#!/bin/sh\ncat >/dev/null\nkill -9 $$\n"},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			result, err := checkWith(t, fake(t, tc.body), Request{Diff: oneFileDiff})
			if err == nil {
				t.Fatalf("must be an error, got a report the caller would record as rigor applied: %+v", result)
			}
			if result != nil {
				t.Fatalf("an error must not also carry a result: %+v", result)
			}
		})
	}
}

// TestAReplyThatSaysOKButCannotBeBelievedIsRefused covers the half that is
// harder to see: the binary exits zero, says ok, and sends a report that is
// internally impossible. Each case here is a way for a finding to be lost or a
// measurement to be invented between the two processes.
func TestAReplyThatSaysOKButCannotBeBelievedIsRefused(t *testing.T) {
	cases := []struct {
		name string
		json string
		// want is a fragment of the refusal, so a case cannot pass by being
		// refused for some unrelated reason.
		want string
		// profile supplies a coverage file. The arithmetic checks are only
		// reachable with one: without it the reply is refused earlier, for
		// reporting gaps nobody measured, and the case would pass while
		// covering a different rule than the one it names.
		profile bool
	}{
		{
			name: "a finding from a gate this client cannot map",
			json: `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[{"gate":"licence_scan","severity":"blocking","path":"src/a.go","line":4,` +
				`"evidence":"x","message":"y"}],"coverage_unmeasured":[],"coverage_gaps":[],` +
				`"coverage_skipped_by_type":[]}`,
			want: "cannot map to a gap",
		},
		{
			name: "a severity outside the two that exist",
			json: `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[{"gate":"secret_scan","severity":"informational","path":"src/a.go","line":4,` +
				`"evidence":"x","message":"y"}],"coverage_unmeasured":[],"coverage_gaps":[],` +
				`"coverage_skipped_by_type":[]}`,
			want: "severity",
		},
		{
			name: "a finding on line zero, which names no line",
			json: `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[{"gate":"stub_detection","severity":"advisory","path":"src/a.go","line":0,` +
				`"evidence":"x","message":"y"}],"coverage_unmeasured":[],"coverage_gaps":[],` +
				`"coverage_skipped_by_type":[]}`,
			want: "names no line",
		},
		{
			name: "a finding against an absolute path",
			json: `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[{"gate":"secret_scan","severity":"blocking","path":"/etc/passwd","line":1,` +
				`"evidence":"x","message":"y"}],"coverage_unmeasured":[],"coverage_gaps":[],` +
				`"coverage_skipped_by_type":[]}`,
			want: "absolute",
		},
		{
			name: "a scope classification that climbs out of the repository",
			json: `{"ok":true,"files":1,"in_scope":["../elsewhere/a.go"],"orphans":[],` +
				`"untouched_planned":[],"findings":[],"coverage_unmeasured":[],"coverage_gaps":[],` +
				`"coverage_skipped_by_type":[]}`,
			want: "climbs out",
		},
		{
			name: "more files counted than classified",
			json: `{"ok":true,"files":9,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[],"coverage_unmeasured":[],"coverage_gaps":[],"coverage_skipped_by_type":[]}`,
			want: "classified",
		},
		{
			// The one that matters most. Nothing was measured, so nothing could
			// be found uncovered; a reply that reports gaps anyway is either a
			// different binary or a measurement nobody made.
			name: "coverage gaps with no coverage profile supplied",
			json: `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[],"coverage_unmeasured":[],"coverage_gaps":[{"path":"src/a.go",` +
				`"added_lines":3,"uncovered_lines":[1,2]}],"coverage_skipped_by_type":[]}`,
			want: "without a coverage profile",
		},
		{
			name: "a coverage gap naming no uncovered line",
			json: `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[],"coverage_unmeasured":[],"coverage_gaps":[{"path":"src/a.go",` +
				`"added_lines":3,"uncovered_lines":[]}],"coverage_skipped_by_type":[]}`,
			want:    "not a gap",
			profile: true,
		},
		{
			name: "more lines uncovered than the diff added",
			json: `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[],"coverage_unmeasured":[],"coverage_gaps":[{"path":"src/a.go",` +
				`"added_lines":1,"uncovered_lines":[1,2,3]}],"coverage_skipped_by_type":[]}`,
			want:    "above one is not a measurement",
			profile: true,
		},
		{
			name: "a coverage gap reporting no added lines to cover",
			json: `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],"untouched_planned":[],` +
				`"findings":[],"coverage_unmeasured":[],"coverage_gaps":[{"path":"src/a.go",` +
				`"added_lines":0,"uncovered_lines":[1]}],"coverage_skipped_by_type":[]}`,
			want:    "nothing to cover",
			profile: true,
		},
		{
			name: "a negative file count",
			json: `{"ok":true,"files":-1,"in_scope":[],"orphans":[],"untouched_planned":[],` +
				`"findings":[],"coverage_unmeasured":[],"coverage_gaps":[],"coverage_skipped_by_type":[]}`,
			want: "not a count",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			req := Request{Diff: oneFileDiff}
			if tc.profile {
				req.CoveragePath = lcov(t)
			}
			result, err := checkWith(t, reply(t, tc.json), req)
			if err == nil {
				t.Fatalf("must be refused, got: %+v", result)
			}
			if result != nil {
				t.Fatalf("a refusal must not also carry a result: %+v", result)
			}
			if !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("refused for the wrong reason: got %q, wanted it to mention %q", err, tc.want)
			}
		})
	}
}

// cleanReply is a well-formed report over a one-file diff with nothing found.
// It is the reply the honesty tests below build on, because the whole question
// they ask is what a *clean* report is allowed to claim.
const cleanReply = `{"ok":true,"files":1,"in_scope":["src/a.go"],"orphans":[],` +
	`"untouched_planned":[],"findings":[],"coverage_unmeasured":["src/a.go"],` +
	`"coverage_gaps":[],"coverage_skipped_by_type":[]}`

// TestACleanReportWithNoProfileNeverClaimsCoverageRan is the honesty property
// the whole package exists for.
//
// Read the wire alone and these two invocations are indistinguishable: both
// report zero coverage gaps. One of them measured nothing. If GatesRun named
// the coverage gate in both, a verifier would record "diff coverage applied,
// no gaps" for a run where no profile was ever opened.
func TestACleanReportWithNoProfileNeverClaimsCoverageRan(t *testing.T) {
	result, err := checkWith(t, reply(t, cleanReply), Request{Diff: oneFileDiff})
	if err != nil {
		t.Fatalf("a well-formed clean report must be accepted: %v", err)
	}

	coverage := result.Coverage()
	if coverage.Measured {
		t.Error("no coverage profile was supplied, so nothing was measured")
	}
	if len(coverage.Gaps) != 0 {
		t.Errorf("gaps=%v; an unmeasured run cannot have found any", coverage.Gaps)
	}
	// The unmeasured list is the honest reading, and it must survive: it is the
	// only thing in the reply that says the file went unexamined.
	if len(coverage.Unmeasured) != 1 || coverage.Unmeasured[0] != "src/a.go" {
		t.Errorf("unmeasured=%v, want the changed file", coverage.Unmeasured)
	}

	gates := result.GatesRun()
	for _, gate := range gates {
		if gate == GateDiffCoverage {
			t.Fatalf("GatesRun()=%v names the coverage gate for a run with no profile; "+
				"a caller records that list as the rigor it applied", gates)
		}
	}
	// The two findings gates are unconditional, so a clean report does attest
	// to them having run. Losing that would be the opposite error — refusing to
	// take credit for work that was done.
	if len(gates) != 2 {
		t.Fatalf("GatesRun()=%v, want exactly the two unconditional findings gates", gates)
	}
}

// TestAProfileMakesTheCoverageGateCountAsRun is the other half: when a profile
// is supplied the gate really did run, and the report must say so.
func TestAProfileMakesTheCoverageGateCountAsRun(t *testing.T) {
	profile := filepath.Join(t.TempDir(), "lcov.info")
	if err := os.WriteFile(profile, []byte("SF:src/a.go\nDA:1,1\nend_of_record\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	result, err := checkWith(t, reply(t, cleanReply), Request{Diff: oneFileDiff, CoveragePath: profile})
	if err != nil {
		t.Fatalf("check: %v", err)
	}
	if !result.Coverage().Measured {
		t.Error("a profile was supplied, so the coverage answer is a measurement")
	}
	found := false
	for _, gate := range result.GatesRun() {
		if gate == GateDiffCoverage {
			found = true
		}
	}
	if !found {
		t.Errorf("GatesRun()=%v omits the coverage gate that ran", result.GatesRun())
	}
}

// TestTheCoveragePathReachesTheBinary guards the wiring between the two tests
// above. Measured is set from the request, so a client that computed it
// correctly while never passing --coverage would satisfy both of them and still
// have measured nothing — the report would claim a gate that never opened the
// file.
func TestTheCoveragePathReachesTheBinary(t *testing.T) {
	profile := lcov(t)
	root := t.TempDir()
	// The recorder writes each argument on its own line and the diff it was
	// handed beside it, then answers cleanly. Written to a file rather than
	// echoed back through the reply because one of the arguments contains a
	// newline by design — the planned list — and folding that into a JSON
	// string would test the fake's quoting rather than the client's.
	record := filepath.Join(t.TempDir(), "argv")
	recorder := fake(t, "#!/bin/sh\n"+
		"for a in \"$@\"; do printf '%s\\n' \"$a\"; done > '"+record+"'\n"+
		"cat > '"+record+".stdin'\n"+
		"echo '"+cleanReply+"'\n")

	client := New(recorder, root)
	client.Timeout = 5 * time.Second
	if _, err := client.Check(context.Background(), Request{
		Diff:         oneFileDiff,
		Planned:      []string{"src/a.go", "src/b.go"},
		CoveragePath: profile,
	}); err != nil {
		t.Fatalf("check: %v", err)
	}

	argv, err := os.ReadFile(record)
	if err != nil {
		t.Fatalf("the fake recorded no arguments: %v", err)
	}
	lines := strings.Split(strings.TrimRight(string(argv), "\n"), "\n")
	// One argument per line, so the planned value spans two of them: the flag,
	// then "src/a.go", then "src/b.go" as the continuation of the same value.
	want := []string{"check", "--planned", "src/a.go", "src/b.go", "--coverage", profile, "--root", root}
	if strings.Join(lines, "|") != strings.Join(want, "|") {
		t.Errorf("invoked with %q, want %q", lines, want)
	}

	// And the diff goes on stdin, which is the only way the gates see anything
	// at all. A client that assembled every flag correctly and sent no diff
	// would get a clean report over zero files.
	stdin, err := os.ReadFile(record + ".stdin")
	if err != nil {
		t.Fatalf("the fake recorded no stdin: %v", err)
	}
	if string(stdin) != oneFileDiff {
		t.Errorf("stdin=%q, want the diff verbatim", stdin)
	}
}

// TestAPlannedPathThatTheEncodingCannotCarryIsRefused covers the one input this
// client rejects before exec'ing anything.
//
// `--planned` separates paths by newline. A Unix filename may contain one, and
// passing it through would split a single planned file into two patterns
// matching nothing — so the file the task planned would come back classified as
// an orphan of a path that names no file, and the diff would be reported out of
// scope for a reason nobody could see.
func TestAPlannedPathThatTheEncodingCannotCarryIsRefused(t *testing.T) {
	result, err := checkWith(t, reply(t, cleanReply), Request{
		Diff:    oneFileDiff,
		Planned: []string{"src/ok.go", "src/two\nlines.go"},
	})
	if err == nil {
		t.Fatalf("a path the flag cannot carry must be refused, got %+v", result)
	}
	if !strings.Contains(err.Error(), "line break") {
		t.Errorf("refused for the wrong reason: %v", err)
	}
}

// TestNoBinaryIsAnErrorNamingTheRemedy: an unconfigured client is the most
// likely way for this boundary to be absent in production, and it must be the
// least ambiguous. A nil client reaching a caller as "no findings" is the whole
// defect this package was written to make impossible.
func TestNoBinaryIsAnErrorNamingTheRemedy(t *testing.T) {
	for _, tc := range []struct {
		name   string
		client *Client
	}{
		{"a nil client", nil},
		{"a client with no binary", &Client{}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			result, err := tc.client.Check(context.Background(), Request{Diff: oneFileDiff})
			if !errors.Is(err, ErrNoBinary) {
				t.Fatalf("err=%v, want ErrNoBinary", err)
			}
			if result != nil {
				t.Fatalf("result=%+v, want nil", result)
			}
			if err := tc.client.Available(context.Background()); !errors.Is(err, ErrNoBinary) {
				t.Fatalf("Available()=%v, want ErrNoBinary", err)
			}
		})
	}
}

// TestAReplyPastTheCapIsRefusedRatherThanTruncated drives the output bound with
// a small limit rather than by generating megabytes.
//
// A truncated reply can still be valid JSON if the cut lands after a complete
// findings array, so decoding the prefix would report a capped sample of the
// gates' findings as everything they found — a blocking secret finding dropped
// because it sorted late.
func TestAReplyPastTheCapIsRefusedRatherThanTruncated(t *testing.T) {
	client := New(reply(t, cleanReply), t.TempDir())
	client.Timeout = 5 * time.Second
	client.maxOutput = 16

	result, err := client.Check(context.Background(), Request{Diff: oneFileDiff})
	if err == nil {
		t.Fatalf("a reply past the cap must be refused, got %+v", result)
	}
	if !strings.Contains(err.Error(), "more than 16 bytes") {
		t.Errorf("the refusal must name the bound it hit: %v", err)
	}
}

// TestAZeroValueClientIsBoundedRatherThanUnbounded: a caller that built a
// Client as a struct literal instead of through New gets the defaults, because
// a cap that falls back to "none" is the failure the cap exists for.
func TestAZeroValueClientIsBoundedRatherThanUnbounded(t *testing.T) {
	client := &Client{Binary: reply(t, cleanReply), Root: t.TempDir()}
	if client.outputBound() != maxOutput {
		t.Errorf("outputBound()=%d, want the default %d", client.outputBound(), maxOutput)
	}
	if client.stderrBound() != maxStderr {
		t.Errorf("stderrBound()=%d, want the default %d", client.stderrBound(), maxStderr)
	}
	// And it still works: the fallback is a bound, not a refusal.
	if _, err := client.Check(context.Background(), Request{Diff: oneFileDiff}); err != nil {
		t.Errorf("a zero-value client with a binary must still run: %v", err)
	}
}

// TestAWedgedVerifierCannotHoldTheCallerOpen. The bound covers Start as well as
// Wait — see proc.RunBounded — so a child that never reads its stdin and never
// exits still returns control.
func TestAWedgedVerifierCannotHoldTheCallerOpen(t *testing.T) {
	// `exec sleep` rather than a shell loop: the shell would stay as a parent
	// holding the pipe, which makes this a test of process-group teardown
	// instead of the deadline.
	client := New(fake(t, "#!/bin/sh\nexec sleep 300\n"), t.TempDir())
	client.Timeout = 500 * time.Millisecond

	start := time.Now()
	result, err := client.Check(context.Background(), Request{Diff: oneFileDiff})
	elapsed := time.Since(start)

	if err == nil {
		t.Fatalf("a verifier that never answers must be an error, got %+v", result)
	}
	if !strings.Contains(err.Error(), "timed out") {
		t.Errorf("err=%v, want a timeout", err)
	}
	// Generous: the assertion is that the deadline bounds the call at all, not
	// that it fires to the millisecond. A tight bound here would be a flake on
	// a loaded machine and would not test anything the loose one does not.
	if elapsed > 30*time.Second {
		t.Errorf("took %s; the deadline did not bound the call", elapsed)
	}
}

// TestTheHealthProbeRefusesTheWrongProcess. Available is how a caller decides
// whether this boundary is usable at all, so it must not accept a healthy reply
// from some other program sitting at the configured path.
func TestTheHealthProbeRefusesTheWrongProcess(t *testing.T) {
	for _, tc := range []struct {
		name string
		json string
		want string
	}{
		{
			name: "a different binary answering",
			json: `{"ok":true,"store":"dc-store","schema_version":1}`,
			want: "identified itself",
		},
		{
			name: "a schema this harness does not speak",
			json: `{"ok":true,"verifier":"dc-verify","schema_version":2}`,
			want: "speaks schema 2",
		},
		{
			// Absent rather than wrong. Without the pointer this decodes to 0,
			// which reads as "older than anything" and would send the client
			// down a compatibility path nobody chose.
			name: "a reply carrying no schema version at all",
			json: `{"ok":true,"verifier":"dc-verify"}`,
			want: "no schema_version",
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			client := New(reply(t, tc.json), t.TempDir())
			client.Timeout = 5 * time.Second
			err := client.Available(context.Background())
			if err == nil {
				t.Fatal("must be refused")
			}
			if !strings.Contains(err.Error(), tc.want) {
				t.Errorf("refused for the wrong reason: got %q, wanted %q", err, tc.want)
			}
		})
	}
}

// TestBlockingReadsTheSeverityTheGateAssigned. The caller turns this into
// whether a gap blocks verification, and an advisory TODO marker must not
// become a blocking gap — nor a planted credential a note.
func TestBlockingReadsTheSeverityTheGateAssigned(t *testing.T) {
	if !(Finding{Severity: SeverityBlocking}).Blocking() {
		t.Error("a blocking finding must report itself blocking")
	}
	if (Finding{Severity: SeverityAdvisory}).Blocking() {
		t.Error("an advisory finding must not block")
	}
	// The empty severity is what a reply with the field absent decodes to. It
	// is refused in validate, so it can never reach a caller — this pins the
	// direction it would fail in if it ever did.
	if (Finding{}).Blocking() {
		t.Error("a finding with no severity must not be treated as blocking")
	}
}
