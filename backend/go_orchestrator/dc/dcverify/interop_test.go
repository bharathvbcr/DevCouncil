// Package dcverify holds the Go side's contract checks for the dcverify
// binary. There is no Go client here yet, and that is the point of the file
// rather than an oversight: the host's verify plane does not spawn dcverify
// (TASK-P7-1 / GAP-P7-DCVERIFY-UNWIRED), Manvi's runRigor does, so nothing on
// this side reads a dcverify reply today.
//
// Until that wiring lands, dcverify is the one binary in the analysis plane
// that this repository builds, installs and documents while holding it to
// nothing. dcstore answers to dc/store, dcgrep answers to dc/dcgrep, and both
// are checked against testsupport's shared wire contract by tests that exec the
// real binary. This closes that asymmetry at the only place it can be closed
// without inventing the client first.
package dcverify

import (
	"encoding/json"
	"slices"
	"testing"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/testsupport"
)

// These are deliberately about the boundary and not about verification results.
// What dcverify decides — which findings a diff earns, how coverage is scored —
// belongs to the Rust suite, and restating it here would make a second owner for
// assertions that already have one. What is checked here is only what a Go
// caller has to be able to rely on the moment TASK-P7-1 wires one up: that a
// reply is one diagnosed JSON object, and that the schema version it advertises
// is the one this repository's protocol document names.

// advertisedSchemaVersion is the number protocols/evidence/v1.md names when it
// says the evidence protocol is advertised independently of
// `dcverify health.schema_version=1`.
//
// It is pinned because it is a constant on both sides of a process boundary,
// and the document is the side with no compiler. Raising it in Rust without
// moving the document and the hosts that read it is precisely the drift that is
// invisible from either side alone.
const advertisedSchemaVersion = 1

// advertisedEvidenceSchema is the evidence-bundle schema version the same
// protocol document describes. dcverify reports the set it accepts rather than
// a single number, so this is checked for membership: adding a second accepted
// version is a compatible change, dropping this one is not.
const advertisedEvidenceSchema = 1

// health runs the handshake once and returns the raw reply, having already held
// it to the wire contract every binary in the analysis plane answers.
func health(t *testing.T) []byte {
	t.Helper()
	bin := testsupport.DCVerify(t)
	args := []string{"health"}

	stdout, exit, err := testsupport.RunChild(t, bin, args, "")
	if err != nil {
		t.Fatalf("run dcverify health: %v", err)
	}
	testsupport.AssertOneDiagnosedObject(t, "dcverify", args, stdout, exit)
	if exit != 0 {
		t.Fatalf("dcverify health exited %d; the handshake a caller probes with must "+
			"succeed on a working binary: %s", exit, stdout)
	}
	return stdout
}

// TestTheHealthHandshakeIsOneDiagnosedReply is the positive control. Every
// assertion about a misbehaving invocation below is only meaningful if the
// binary passes the same contract when it is behaving.
func TestTheHealthHandshakeIsOneDiagnosedReply(t *testing.T) {
	stdout := health(t)

	var reply struct {
		OK       bool   `json:"ok"`
		Verifier string `json:"verifier"`
	}
	if err := json.Unmarshal(stdout, &reply); err != nil {
		t.Fatalf("decode dcverify health: %v (%q)", err, stdout)
	}
	if !reply.OK {
		t.Fatalf("dcverify health reported ok:false while exiting zero: %s", stdout)
	}
	// The reply says which binary answered. dcstore's health names itself under
	// "store" and this one under "verifier", so a caller that probed the wrong
	// path would otherwise read a healthy reply from the wrong process.
	if reply.Verifier != "dc-verify" {
		t.Errorf("dcverify health names the verifier %q, want %q: %s",
			reply.Verifier, "dc-verify", stdout)
	}
}

// TestTheHealthHandshakeAdvertisesTheDocumentedSchemaVersions holds the binary
// to the numbers protocols/evidence/v1.md publishes. A host decides whether it
// can speak to this verifier by reading them, so they are an interface, not an
// implementation detail.
func TestTheHealthHandshakeAdvertisesTheDocumentedSchemaVersions(t *testing.T) {
	stdout := health(t)

	// Pointers, so that an absent key is distinguishable from a zero. A version
	// field that silently decodes to 0 would read as "older than anything" and
	// send a caller down a compatibility path nobody chose.
	var reply struct {
		SchemaVersion          *int  `json:"schema_version"`
		EvidenceSchemaVersions []int `json:"evidence_schema_versions"`
	}
	if err := json.Unmarshal(stdout, &reply); err != nil {
		t.Fatalf("decode dcverify health: %v (%q)", err, stdout)
	}

	if reply.SchemaVersion == nil {
		t.Fatalf("dcverify health carries no schema_version; protocols/evidence/v1.md "+
			"tells hosts to read one: %s", stdout)
	}
	if *reply.SchemaVersion != advertisedSchemaVersion {
		t.Errorf("dcverify health advertises schema_version %d, but "+
			"protocols/evidence/v1.md names %d; move the document and the hosts that "+
			"read it in the same change, or this number means nothing",
			*reply.SchemaVersion, advertisedSchemaVersion)
	}

	if len(reply.EvidenceSchemaVersions) == 0 {
		t.Fatalf("dcverify health accepts no evidence schema versions; the evidence "+
			"protocol is advertised through this field: %s", stdout)
	}
	if !slices.Contains(reply.EvidenceSchemaVersions, advertisedEvidenceSchema) {
		t.Errorf("dcverify health accepts evidence schemas %v, which does not include "+
			"the documented %d; a bundle written to protocols/evidence/v1.md would be "+
			"refused", reply.EvidenceSchemaVersions, advertisedEvidenceSchema)
	}
}

// TestABadInvocationIsDiagnosedRatherThanSilent is the adversarial half. An
// exit code with no diagnosis in the reply leaves a Go caller reporting
// "failed: exit status 2" and nothing an operator can act on, which is the
// failure AssertOneDiagnosedObject exists to name.
func TestABadInvocationIsDiagnosedRatherThanSilent(t *testing.T) {
	bin := testsupport.DCVerify(t)

	for _, tc := range []struct {
		name string
		args []string
	}{
		{name: "unknown subcommand", args: []string{"no-such-subcommand"}},
		{name: "unknown flag", args: []string{"check", "--no-such-flag"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			stdout, exit, err := testsupport.RunChild(t, bin, tc.args, "")
			if err != nil {
				t.Fatalf("run dcverify %v: %v", tc.args, err)
			}
			if exit == 0 {
				t.Fatalf("dcverify %v exited zero; an invocation it cannot carry out must "+
					"not report success: %s", tc.args, stdout)
			}
			// The contract does the rest: one JSON object, an "ok" field, and a
			// non-zero exit that carries its own reason.
			testsupport.AssertOneDiagnosedObject(t, "dcverify", tc.args, stdout, exit)
		})
	}
}
