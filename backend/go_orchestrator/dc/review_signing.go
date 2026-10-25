package dc

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// The signature is what makes a review the harness's word rather than
// anyone's. The agent being verified can write repo files and, with a broad
// task allowlist, reach the `dcstore` and `devcouncil` binaries many ways
// (the command.harness_only gate rule is a tripwire, not the boundary — it is
// a denylist over shell spellings and cannot be complete). So a review verify
// will discharge an llm_review criterion from must carry an ed25519 signature
// that checks against a key the repository trusts, and the private key lives
// outside the repository where the agent's gate refuses to read it. An agent
// that cannot sign cannot forge a review, however it reaches the writer.
//
// ed25519 (crypto/ed25519, standard library) is asymmetric on purpose: the
// verifier holds only public keys, which are committed in protected
// `.devcouncil/` config, so reading them buys the agent nothing. A shared
// secret (HMAC) would be readable in the agent's own environment and is
// rejected for that reason.

// ReviewSignersRel is where the trusted public keys live, relative to the
// repository root. It is under `.devcouncil/`, which the write gate refuses to
// let an agent modify, so the set of trusted signers is not the agent's to
// change.
const ReviewSignersRel = ".devcouncil/review_signers.json"

// maxSignersFile bounds the trusted-signers file.
const maxSignersFile = 1 << 20

// ReviewSigners is the set of keys a repository trusts to sign reviews, by key
// id. A record is discharged only when its signature checks against the key
// its key_id names here.
type ReviewSigners struct {
	keys map[string]ed25519.PublicKey
	// Path is where the set was loaded from, for messages.
	Path string
}

// Count is how many keys are trusted.
func (s ReviewSigners) Count() int { return len(s.keys) }

type signerFile struct {
	Signers []struct {
		KeyID     string `json:"key_id"`
		PublicKey string `json:"public_key"`
	} `json:"signers"`
}

// LoadReviewSigners reads the trusted public keys from ReviewSignersRel under
// root. A missing file is an empty set, not an error: verify then blocks every
// llm_review criterion with "no review signers are configured", which is the
// fail-closed direction — an unconfigured repository discharges nothing rather
// than everything.
func LoadReviewSigners(root string) (ReviewSigners, error) {
	path := filepath.Join(root, filepath.FromSlash(ReviewSignersRel))
	out := ReviewSigners{keys: map[string]ed25519.PublicKey{}, Path: path}
	f, err := os.Open(path)
	if os.IsNotExist(err) {
		return out, nil
	}
	if err != nil {
		return out, fmt.Errorf("opening %s: %w", ReviewSignersRel, err)
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil {
		return out, err
	}
	if info.Size() > maxSignersFile {
		return out, fmt.Errorf("%s is larger than %d bytes", ReviewSignersRel, maxSignersFile)
	}
	dec := json.NewDecoder(f)
	dec.DisallowUnknownFields()
	var parsed signerFile
	if err := dec.Decode(&parsed); err != nil {
		return out, fmt.Errorf("%s: %w", ReviewSignersRel, err)
	}
	for i, s := range parsed.Signers {
		if err := checkReviewID(fmt.Sprintf("signers[%d].key_id", i), s.KeyID); err != nil {
			return ReviewSigners{}, err
		}
		if _, dup := out.keys[s.KeyID]; dup {
			return ReviewSigners{}, fmt.Errorf("%s lists key_id %q twice", ReviewSignersRel, s.KeyID)
		}
		pub, err := decodeEd25519Public(s.PublicKey)
		if err != nil {
			return ReviewSigners{}, fmt.Errorf("%s: key_id %q: %w", ReviewSignersRel, s.KeyID, err)
		}
		out.keys[s.KeyID] = pub
	}
	return out, nil
}

// signingBytes is the record's bytes to sign and verify: the record with its
// signature cleared, marshaled from the typed struct so field order is fixed
// and the on-disk spelling (key order, whitespace) does not matter. KeyID is
// kept, so a signature cannot be moved to a record claiming another key.
func (r ReviewRecord) signingBytes() ([]byte, error) {
	r.Signature = ""
	return json.Marshal(r)
}

// Sign fills KeyID and Signature. The record must already be valid in every
// other respect; signing is the last step a producer takes.
func (r *ReviewRecord) Sign(keyID string, priv ed25519.PrivateKey) error {
	if len(priv) != ed25519.PrivateKeySize {
		return fmt.Errorf("signing key is %d bytes, not %d", len(priv), ed25519.PrivateKeySize)
	}
	if err := checkReviewID("key_id", keyID); err != nil {
		return err
	}
	r.KeyID = keyID
	r.Signature = ""
	msg, err := r.signingBytes()
	if err != nil {
		return err
	}
	r.Signature = base64.StdEncoding.EncodeToString(ed25519.Sign(priv, msg))
	return nil
}

// VerifySignature reports whether the record is signed by a trusted key. A
// record with no trusted signers, an unknown key_id, or a signature that does
// not check is refused — never read as signed. ed25519.Verify is the
// constant-time primitive; no comparison here handles a secret.
func (r ReviewRecord) VerifySignature(trusted ReviewSigners) error {
	if trusted.Count() == 0 {
		return fmt.Errorf("no review signers are configured in %s, so no review can be trusted", ReviewSignersRel)
	}
	pub, ok := trusted.keys[r.KeyID]
	if !ok {
		return fmt.Errorf("the review is signed by key %q, which %s does not trust", r.KeyID, ReviewSignersRel)
	}
	sig, err := base64.StdEncoding.DecodeString(r.Signature)
	if err != nil || len(sig) != ed25519.SignatureSize {
		return fmt.Errorf("the review's signature is not a %d-byte ed25519 signature", ed25519.SignatureSize)
	}
	msg, err := r.signingBytes()
	if err != nil {
		return err
	}
	if !ed25519.Verify(pub, msg, sig) {
		return fmt.Errorf("the review's signature does not verify against key %q: its bytes were changed after it was signed", r.KeyID)
	}
	return nil
}

// validSignatureShape checks that a signature is base64 of the right length,
// before any key is consulted. The shape is part of a well-formed record;
// whether it verifies is a question for VerifySignature with the trusted set.
func validSignatureShape(sig string) error {
	raw, err := base64.StdEncoding.DecodeString(strings.TrimSpace(sig))
	if err != nil {
		return fmt.Errorf("review record signature is not base64: %w", err)
	}
	if len(raw) != ed25519.SignatureSize {
		return fmt.Errorf("review record signature is %d bytes, not %d", len(raw), ed25519.SignatureSize)
	}
	return nil
}

func decodeEd25519Public(s string) (ed25519.PublicKey, error) {
	raw, err := base64.StdEncoding.DecodeString(strings.TrimSpace(s))
	if err != nil {
		return nil, fmt.Errorf("public key is not base64: %w", err)
	}
	if len(raw) != ed25519.PublicKeySize {
		return nil, fmt.Errorf("public key is %d bytes, not %d", len(raw), ed25519.PublicKeySize)
	}
	return ed25519.PublicKey(raw), nil
}

// SigningKey is a private signing key and the id it signs under, as the
// harness or operator holds it, outside the repository.
type SigningKey struct {
	KeyID      string `json:"key_id"`
	PrivateKey string `json:"private_key"`
}

// LoadSigningKey reads a private signing key from a file the agent cannot
// reach. The caller is responsible for keeping that path outside the
// repository; this refuses a key file that is group- or world-readable, the
// way an ssh client refuses a loose private key.
func LoadSigningKey(path string) (string, ed25519.PrivateKey, error) {
	info, err := os.Stat(path)
	if err != nil {
		return "", nil, err
	}
	if info.Size() > maxSignersFile {
		return "", nil, fmt.Errorf("%s is larger than %d bytes", path, maxSignersFile)
	}
	if info.Mode().Perm()&0o077 != 0 {
		return "", nil, fmt.Errorf("%s is readable by group or others (%o); a signing key must be 0600", path, info.Mode().Perm())
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return "", nil, err
	}
	var k SigningKey
	if err := json.Unmarshal(data, &k); err != nil {
		return "", nil, fmt.Errorf("%s: %w", path, err)
	}
	if err := checkReviewID("key_id", k.KeyID); err != nil {
		return "", nil, err
	}
	raw, err := base64.StdEncoding.DecodeString(strings.TrimSpace(k.PrivateKey))
	if err != nil {
		return "", nil, fmt.Errorf("%s: private key is not base64: %w", path, err)
	}
	if len(raw) != ed25519.PrivateKeySize {
		return "", nil, fmt.Errorf("%s: private key is %d bytes, not %d", path, len(raw), ed25519.PrivateKeySize)
	}
	return k.KeyID, ed25519.PrivateKey(raw), nil
}

// DecodeAndSign decodes a review that does not yet carry a signature, signs it
// with the given key, and validates the result. It is the operator path:
// `devcouncil review record --sign-key`, where a reviewer harness wrote the
// record and a person at a terminal signs it. Unknown fields and trailing
// data are refused, as for a finished record.
func DecodeAndSign(data []byte, keyID string, priv ed25519.PrivateKey) (ReviewRecord, error) {
	if len(data) > maxRecordBytes {
		return ReviewRecord{}, fmt.Errorf("review record is over %d bytes", maxRecordBytes)
	}
	dec := json.NewDecoder(strings.NewReader(string(data)))
	dec.DisallowUnknownFields()
	var r ReviewRecord
	if err := dec.Decode(&r); err != nil {
		return ReviewRecord{}, fmt.Errorf("review record: %w", err)
	}
	if dec.More() {
		return ReviewRecord{}, fmt.Errorf("review record: trailing data after the object")
	}
	if err := r.Sign(keyID, priv); err != nil {
		return ReviewRecord{}, err
	}
	if err := r.Validate(); err != nil {
		return ReviewRecord{}, err
	}
	return r, nil
}

// GenerateSigningKey makes a new ed25519 key under keyID and returns the
// private-key file bytes (to write 0600, outside the repository) and the
// public signer line (to add to the trusted-signers file).
func GenerateSigningKey(keyID string) (keyFile []byte, signerLine string, err error) {
	if err := checkReviewID("key_id", keyID); err != nil {
		return nil, "", err
	}
	pub, priv, err := ed25519.GenerateKey(nil)
	if err != nil {
		return nil, "", err
	}
	keyFile, err = json.MarshalIndent(SigningKey{
		KeyID:      keyID,
		PrivateKey: base64.StdEncoding.EncodeToString(priv),
	}, "", "  ")
	if err != nil {
		return nil, "", err
	}
	return keyFile, fmt.Sprintf(`{"key_id":%q,"public_key":%q}`, keyID, base64.StdEncoding.EncodeToString(pub)), nil
}

// PublicSignerLine is the entry to add to the trusted-signers file for a
// private key: its key id and its public half.
func PublicSignerLine(keyID string, priv ed25519.PrivateKey) (string, error) {
	if len(priv) != ed25519.PrivateKeySize {
		return "", fmt.Errorf("signing key is %d bytes, not %d", len(priv), ed25519.PrivateKeySize)
	}
	pub := priv.Public().(ed25519.PublicKey)
	return fmt.Sprintf(`{"key_id":%q,"public_key":%q}`, keyID, base64.StdEncoding.EncodeToString(pub)), nil
}
