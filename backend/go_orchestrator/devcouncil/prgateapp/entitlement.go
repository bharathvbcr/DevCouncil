package prgateapp

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"
	"time"
)

const (
	// DefaultEntitlementPath is the HMAC-authenticated app entitlement endpoint
	// served by `gitpulse-relay` (locked cross-repo contract #74).
	DefaultEntitlementPath = "/v1/pr-gate/app-authorize"
	// HeaderAppTimestamp carries the Unix timestamp (seconds) covered by the HMAC.
	HeaderAppTimestamp = "X-DevCouncil-Timestamp"
	// HeaderAppSignature carries `sha256=<hex>` HMAC-SHA256 over the canonical request.
	HeaderAppSignature = "X-DevCouncil-Signature"
)

// EntitlementRequest describes the private/internal repository being checked
// server-side against `gitpulse-relay` (keyed on immutable GitHub IDs).
type EntitlementRequest struct {
	HeadSHA              string `json:"head_sha"`
	InstallationID       int64  `json:"installation_id"`
	RepositoryID         string `json:"repository_id"`
	RepositoryName       string `json:"repository_name"`
	RepositoryOwnerID    int64  `json:"repository_owner_id"`
	RepositoryVisibility string `json:"repository_visibility"`
}

// EntitlementResult is the fail-closed entitlement decision returned by the
// relay or an injected test double.
type EntitlementResult struct {
	Allowed      bool   `json:"allowed"`
	Tier         string `json:"tier,omitempty"`
	ReposUsed    int    `json:"repos_used,omitempty"`
	MaxRepos     int    `json:"max_repos,omitempty"`
	Unlimited    bool   `json:"unlimited,omitempty"`
	ReasonCode   string `json:"code,omitempty"`
	QuotaLimited bool   `json:"quota_limited,omitempty"`
	TierRequired bool   `json:"tier_required,omitempty"`
	Message      string `json:"message,omitempty"`
}

// EntitlementChecker verifies whether a private/internal repository is
// entitled to run the hosted DevCouncil PR Gate.
type EntitlementChecker interface {
	CheckPrivateRepo(ctx context.Context, req EntitlementRequest) (EntitlementResult, error)
}

// ComputeAppServiceSignature computes the canonical HMAC-SHA256 signature
// authenticating `devcouncil-app` to `gitpulse-relay`:
//
//	sha256=<hex(HMAC-SHA256(secret, method "\n" path "\n" timestamp "\n" hex(sha256(body))))>
func ComputeAppServiceSignature(secret, method, path, timestamp string, body []byte) string {
	bodySum := sha256.Sum256(body)
	canonical := method + "\n" + path + "\n" + timestamp + "\n" + hex.EncodeToString(bodySum[:])
	mac := hmac.New(sha256.New, []byte(secret))
	_, _ = mac.Write([]byte(canonical))
	return "sha256=" + hex.EncodeToString(mac.Sum(nil))
}

// VerifyAppServiceSignature validates an incoming app service signature in
// constant time (for relay parity tests and verification).
func VerifyAppServiceSignature(secret, method, path, timestamp string, body []byte, sigHeader string) bool {
	if secret == "" || timestamp == "" {
		return false
	}
	expected := ComputeAppServiceSignature(secret, method, path, timestamp, body)
	return hmac.Equal([]byte(expected), []byte(strings.TrimSpace(sigHeader)))
}

// RelayEntitlementClient checks private-repository entitlements against
// `gitpulse-relay` over HTTPS (or loopback HTTP in tests) using an HMAC-SHA256
// app service credential.
type RelayEntitlementClient struct {
	BaseURL      string
	EndpointPath string // defaults to /v1/pr-gate/entitlement
	ServiceToken string // shared HMAC secret between devcouncil-app and gitpulse-relay
	HTTPClient   *http.Client
	Now          func() time.Time
}

func (c *RelayEntitlementClient) endpointPath() string {
	p := strings.TrimSpace(c.EndpointPath)
	if p == "" {
		return DefaultEntitlementPath
	}
	if !strings.HasPrefix(p, "/") {
		p = "/" + p
	}
	return p
}

func (c *RelayEntitlementClient) endpoint() (string, string, error) {
	base := strings.TrimRight(strings.TrimSpace(c.BaseURL), "/")
	if base == "" {
		return "", "", errors.New("relay entitlement URL is not configured")
	}
	if !strings.HasPrefix(base, "https://") && !isLoopbackHTTP(base) {
		return "", "", errors.New("relay entitlement URL must use https://")
	}
	if strings.TrimSpace(c.ServiceToken) == "" {
		return "", "", errors.New("relay app service secret is not configured")
	}
	p := c.endpointPath()
	return base + p, p, nil
}

func isLoopbackHTTP(raw string) bool {
	rest, ok := strings.CutPrefix(raw, "http://")
	if !ok {
		return false
	}
	host := strings.SplitN(rest, "/", 2)[0]
	return host == "127.0.0.1" || strings.HasPrefix(host, "127.0.0.1:") ||
		host == "localhost" || strings.HasPrefix(host, "localhost:") ||
		host == "[::1]" || strings.HasPrefix(host, "[::1]:")
}

// CheckPrivateRepo posts the HMAC-signed entitlement check to `gitpulse-relay`
// and fails closed on any non-200 status, malformed response, or missing
// `allowed: true`.
func (c *RelayEntitlementClient) CheckPrivateRepo(ctx context.Context, req EntitlementRequest) (EntitlementResult, error) {
	endpoint, path, err := c.endpoint()
	if err != nil {
		return EntitlementResult{Allowed: false, ReasonCode: "entitlement_unconfigured"}, err
	}
	payload, err := json.Marshal(req)
	if err != nil {
		return EntitlementResult{Allowed: false, ReasonCode: "marshal_error"}, err
	}
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, bytes.NewReader(payload))
	if err != nil {
		return EntitlementResult{Allowed: false, ReasonCode: "request_error"}, err
	}

	nowFn := c.Now
	if nowFn == nil {
		nowFn = time.Now
	}
	ts := strconv.FormatInt(nowFn().Unix(), 10)
	sig := ComputeAppServiceSignature(strings.TrimSpace(c.ServiceToken), http.MethodPost, path, ts, payload)

	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Accept", "application/json")
	httpReq.Header.Set("User-Agent", "devcouncil-pr-gate-app")
	httpReq.Header.Set(HeaderAppTimestamp, ts)
	httpReq.Header.Set(HeaderAppSignature, sig)

	client := c.HTTPClient
	if client == nil {
		client = &http.Client{Timeout: 15 * time.Second}
	}
	resp, err := client.Do(httpReq)
	if err != nil {
		return EntitlementResult{
			Allowed:    false,
			ReasonCode: "relay_unreachable",
			Message:    "entitlement service unreachable",
		}, fmt.Errorf("relay entitlement request: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(io.LimitReader(resp.Body, 256*1024))
	if err != nil {
		return EntitlementResult{Allowed: false, ReasonCode: "relay_read_error"}, err
	}

	var parsed struct {
		Allowed      bool   `json:"allowed"`
		Tier         string `json:"tier"`
		Plan         string `json:"plan"`
		ReposUsed    int    `json:"repos_used"`
		MaxRepos     int    `json:"max_repos"`
		Unlimited    bool   `json:"unlimited"`
		Reason       string `json:"reason"`
		Error        string `json:"error"`
		Code         string `json:"code"`
		Message      string `json:"message"`
		QuotaLimited bool   `json:"quota_limited"`
		TierRequired bool   `json:"tier_required"`
	}
	_ = json.Unmarshal(body, &parsed)

	tier := sanitizeCode(parsed.Tier)
	if tier == "" {
		tier = sanitizeCode(parsed.Plan)
	}
	code := sanitizeCode(parsed.Error)
	if code == "" {
		code = sanitizeCode(parsed.Code)
	}
	if code == "" {
		code = sanitizeCode(parsed.Reason)
	}

	if resp.StatusCode == http.StatusPaymentRequired || resp.StatusCode == http.StatusForbidden {
		if code == "" {
			code = "feature_required"
		}
		return EntitlementResult{
			Allowed:      false,
			Tier:         tier,
			ReasonCode:   code,
			TierRequired: true,
			QuotaLimited: parsed.QuotaLimited || strings.Contains(code, "quota"),
		}, nil
	}
	if resp.StatusCode == http.StatusTooManyRequests {
		if code == "" {
			code = "pr_gate_quota_exceeded"
		}
		return EntitlementResult{
			Allowed:      false,
			Tier:         tier,
			ReasonCode:   code,
			QuotaLimited: true,
		}, nil
	}
	if resp.StatusCode != http.StatusOK {
		if code == "" {
			code = fmt.Sprintf("relay_http_%d", resp.StatusCode)
		}
		return EntitlementResult{
			Allowed:    false,
			ReasonCode: code,
		}, fmt.Errorf("relay entitlement returned HTTP %d", resp.StatusCode)
	}
	if !parsed.Allowed {
		if code == "" {
			code = "entitlement_denied"
		}
		return EntitlementResult{
			Allowed:      false,
			Tier:         tier,
			ReasonCode:   code,
			QuotaLimited: parsed.QuotaLimited || strings.Contains(code, "quota"),
			TierRequired: parsed.TierRequired || strings.Contains(code, "tier") || strings.Contains(code, "feature"),
		}, nil
	}
	return EntitlementResult{
		Allowed:   true,
		Tier:      tier,
		ReposUsed: parsed.ReposUsed,
		MaxRepos:  parsed.MaxRepos,
		Unlimited: parsed.Unlimited,
	}, nil
}

func sanitizeCode(raw string) string {
	var b strings.Builder
	for i := 0; i < len(raw) && b.Len() < 64; i++ {
		c := raw[i]
		if (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_' || c == '-' || c == '.' {
			b.WriteByte(c)
		}
	}
	return b.String()
}
