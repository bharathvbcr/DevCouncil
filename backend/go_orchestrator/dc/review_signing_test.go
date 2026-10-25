package dc

import (
	"crypto/ed25519"
	"encoding/base64"
	"os"
	"path/filepath"
	"testing"
)

func validRecord() ReviewRecord {
	return ReviewRecord{
		Schema: ReviewRecordSchema, TaskID: "TASK-1", AcceptanceCriterionID: "AC-1",
		SubjectHash: "tree-sha256:aa", Harness: "manvi",
		CriterionSHA256: CriterionDigest(AcceptanceCriterion{ID: "AC-1", Description: "x", Method: VerifyLLMReview}),
		Rubric:          ReviewRubric{Path: "docs/r.md", SHA256: zeros64()},
		Isolation:       ReviewIsolation{Probe: IsolationPassed},
		Reviewers: []ReviewerRun{
			{ID: "a", Persona: "critic", Status: ReviewerCompleted},
			{ID: "b", Persona: "critic", Status: ReviewerCompleted},
		},
	}
}

// zeros64 is 64 lowercase hex digits, the shape validSHA256 wants.
func zeros64() string {
	return "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}

func trust(t *testing.T, keyID string, priv ed25519.PrivateKey) ReviewSigners {
	t.Helper()
	root := t.TempDir()
	line, err := PublicSignerLine(keyID, priv)
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "review_signers.json"), []byte(`{"signers":[`+line+`]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	signers, err := LoadReviewSigners(root)
	if err != nil {
		t.Fatal(err)
	}
	return signers
}

// A record the trusted key signed verifies; nothing else does.
func TestAReviewVerifiesOnlyUnderTheKeyThatSignedIt(t *testing.T) {
	_, priv, _ := ed25519.GenerateKey(nil)
	signers := trust(t, "manvi", priv)

	r := validRecord()
	if err := r.Sign("manvi", priv); err != nil {
		t.Fatal(err)
	}
	if err := r.Validate(); err != nil {
		t.Fatalf("a signed record did not validate: %v", err)
	}
	if err := r.VerifySignature(signers); err != nil {
		t.Fatalf("a trusted review did not verify: %v", err)
	}

	// Any change to the signed bytes breaks the signature.
	for name, mutate := range map[string]func(*ReviewRecord){
		"subject": func(r *ReviewRecord) { r.SubjectHash = "tree-sha256:bb" },
		"finding": func(r *ReviewRecord) {
			r.Findings = append(r.Findings, ReviewFinding{ID: "F", Severity: SeverityNit, Summary: "x", Resolution: ResolutionOpen})
		},
		"harness":  func(r *ReviewRecord) { r.Harness = "someone else" },
		"criteria": func(r *ReviewRecord) { r.AcceptanceCriterionID = "AC-2" },
	} {
		t.Run(name, func(t *testing.T) {
			tampered := r
			mutate(&tampered)
			if err := tampered.VerifySignature(signers); err == nil {
				t.Error("a changed record still verified")
			}
		})
	}
}

// A key the repository does not trust, and an empty trust set, both refuse.
func TestAnUntrustedOrAbsentKeyIsRefused(t *testing.T) {
	_, trustedPriv, _ := ed25519.GenerateKey(nil)
	_, roguePriv, _ := ed25519.GenerateKey(nil)
	signers := trust(t, "manvi", trustedPriv)

	rogue := validRecord()
	if err := rogue.Sign("manvi", roguePriv); err != nil { // same key_id, wrong key
		t.Fatal(err)
	}
	if err := rogue.VerifySignature(signers); err == nil {
		t.Error("a review signed by a rogue key under a trusted key's id verified")
	}

	other := validRecord()
	if err := other.Sign("other", roguePriv); err != nil {
		t.Fatal(err)
	}
	if err := other.VerifySignature(signers); err == nil {
		t.Error("a review signed by an unknown key id verified")
	}

	if err := other.VerifySignature(ReviewSigners{}); err == nil {
		t.Error("a review verified against an empty trust set")
	}
}

// keygen writes a usable private key and a signer line that loads back to the
// same key, and the private file must be 0600.
func TestKeygenRoundTrips(t *testing.T) {
	keyFile, signerLine, err := GenerateSigningKey("manvi-prod")
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	keyPath := filepath.Join(dir, "k.json")
	if err := os.WriteFile(keyPath, keyFile, 0o600); err != nil {
		t.Fatal(err)
	}
	keyID, priv, err := LoadSigningKey(keyPath)
	if err != nil {
		t.Fatal(err)
	}
	if keyID != "manvi-prod" {
		t.Fatalf("key id = %q", keyID)
	}
	signersDir := filepath.Join(dir, ".devcouncil")
	if err := os.MkdirAll(signersDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(signersDir, "review_signers.json"), []byte(`{"signers":[`+signerLine+`]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	signers, err := LoadReviewSigners(dir)
	if err != nil {
		t.Fatal(err)
	}
	r := validRecord()
	if err := r.Sign(keyID, priv); err != nil {
		t.Fatal(err)
	}
	if err := r.VerifySignature(signers); err != nil {
		t.Fatalf("keygen's key did not verify through its signer line: %v", err)
	}
}

// A loose-permission private key is refused, as an ssh client refuses one.
func TestALoosePrivateKeyIsRefused(t *testing.T) {
	keyFile, _, err := GenerateSigningKey("k")
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "k.json")
	if err := os.WriteFile(path, keyFile, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, _, err := LoadSigningKey(path); err == nil {
		t.Error("a group/world-readable signing key was accepted")
	}
}

// An all-zero public key — what a blank or truncated config pastes in — is
// refused by the loader rather than entering the trust set.
func TestAnAllZeroPublicKeyIsRefused(t *testing.T) {
	zero := base64.StdEncoding.EncodeToString(make([]byte, ed25519.PublicKeySize))
	root := t.TempDir()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	line := `{"key_id":"zero","public_key":"` + zero + `"}`
	if err := os.WriteFile(filepath.Join(dir, "review_signers.json"), []byte(`{"signers":[`+line+`]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadReviewSigners(root); err == nil {
		t.Error("an all-zero public key was accepted into the trust set")
	}
}

// A missing signers file is an empty set, not an error: verify then blocks
// rather than opening the gate.
func TestMissingSignersFileIsAnEmptySet(t *testing.T) {
	signers, err := LoadReviewSigners(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if signers.Count() != 0 {
		t.Fatalf("count = %d, want 0", signers.Count())
	}
}

// A duplicate key id in the signers file is refused rather than letting one
// shadow the other.
func TestDuplicateSignerIDIsRefused(t *testing.T) {
	_, priv, _ := ed25519.GenerateKey(nil)
	line, err := PublicSignerLine("dup", priv)
	if err != nil {
		t.Fatal(err)
	}
	root := t.TempDir()
	dir := filepath.Join(root, ".devcouncil")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "review_signers.json"), []byte(`{"signers":[`+line+`,`+line+`]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadReviewSigners(root); err == nil {
		t.Error("a duplicate signer id was accepted")
	}
}
