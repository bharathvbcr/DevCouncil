package prgateapp

import (
	"bytes"
	"context"
	"crypto"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

const (
	// DefaultGitHubAPIBase is the production GitHub REST API origin.
	DefaultGitHubAPIBase = "https://api.github.com"
	// CheckRunName is the GitHub Checks API check name posted by this App.
	CheckRunName = "devcouncil/pr-gate"
	// MaxCheckRunAnnotations matches GitHub's per-request Checks API ceiling.
	MaxCheckRunAnnotations = 50
	// MaxCheckRunSummaryBytes stays safely below GitHub's 65,535-byte summary cap.
	MaxCheckRunSummaryBytes = 60_000
)

// CheckRunAnnotation is one GitHub Checks API file annotation.
type CheckRunAnnotation struct {
	Path            string `json:"path"`
	StartLine       int    `json:"start_line"`
	EndLine         int    `json:"end_line"`
	AnnotationLevel string `json:"annotation_level"` // notice | warning | failure
	Message         string `json:"message"`
	Title           string `json:"title,omitempty"`
}

// CheckRunOutput is the `output` object of a GitHub Check Run.
type CheckRunOutput struct {
	Title       string               `json:"title"`
	Summary     string               `json:"summary"`
	Annotations []CheckRunAnnotation `json:"annotations,omitempty"`
}

// CheckRunRequest is the payload sent to `POST /repos/{owner}/{repo}/check-runs`
// or `PATCH /repos/{owner}/{repo}/check-runs/{check_run_id}`.
type CheckRunRequest struct {
	Name       string         `json:"name"`
	HeadSHA    string         `json:"head_sha,omitempty"`
	Status     string         `json:"status"`               // queued | in_progress | completed
	Conclusion string         `json:"conclusion,omitempty"` // success | failure | action_required (NEVER neutral on degraded)
	Output     CheckRunOutput `json:"output"`
}

// CheckRunResponse holds the GitHub Check Run response fields we care about.
type CheckRunResponse struct {
	ID      int64  `json:"id"`
	HTMLURL string `json:"html_url,omitempty"`
}

// GitHubClient abstracts GitHub App token exchange and Checks API publishing
// so unit tests can inject a mock or `httptest` server.
type GitHubClient interface {
	CreateInstallationToken(ctx context.Context, installationID int64, repoName string) (string, error)
	CreateCheckRun(ctx context.Context, token, owner, repo string, req CheckRunRequest) (*CheckRunResponse, error)
	UpdateCheckRun(ctx context.Context, token, owner, repo string, checkRunID int64, req CheckRunRequest) (*CheckRunResponse, error)
}

// ParseRSAPrivateKeyPEM decodes a PKCS#1 (`RSA PRIVATE KEY`) or PKCS#8
// (`PRIVATE KEY`) PEM-encoded RSA private key.
func ParseRSAPrivateKeyPEM(pemBytes []byte) (*rsa.PrivateKey, error) {
	block, _ := pem.Decode(pemBytes)
	if block == nil {
		return nil, errors.New("no PEM block found in GitHub App private key")
	}
	if key, err := x509.ParsePKCS1PrivateKey(block.Bytes); err == nil {
		return key, nil
	}
	parsed, err := x509.ParsePKCS8PrivateKey(block.Bytes)
	if err != nil {
		return nil, fmt.Errorf("parse GitHub App RSA private key: %w", err)
	}
	rsaKey, ok := parsed.(*rsa.PrivateKey)
	if !ok {
		return nil, errors.New("GitHub App private key is not RSA")
	}
	return rsaKey, nil
}

// MintAppJWT constructs an RS256-signed GitHub App JWT valid for 9 minutes
// (issued 60s in the past to tolerate clock skew, within GitHub's 10m max).
func MintAppJWT(appID string, key *rsa.PrivateKey, now time.Time) (string, error) {
	appID = strings.TrimSpace(appID)
	if appID == "" {
		return "", errors.New("GitHub App ID is empty")
	}
	if key == nil {
		return "", errors.New("GitHub App private key is nil")
	}
	headerJSON, err := json.Marshal(map[string]string{
		"alg": "RS256",
		"typ": "JWT",
	})
	if err != nil {
		return "", err
	}
	claimsJSON, err := json.Marshal(map[string]any{
		"iat": now.Add(-60 * time.Second).Unix(),
		"exp": now.Add(9 * time.Minute).Unix(),
		"iss": appID,
	})
	if err != nil {
		return "", err
	}
	enc := base64.RawURLEncoding
	signingInput := enc.EncodeToString(headerJSON) + "." + enc.EncodeToString(claimsJSON)
	digest := sha256.Sum256([]byte(signingInput))
	sig, err := rsa.SignPKCS1v15(rand.Reader, key, crypto.SHA256, digest[:])
	if err != nil {
		return "", fmt.Errorf("sign GitHub App JWT: %w", err)
	}
	return signingInput + "." + enc.EncodeToString(sig), nil
}

// HTTPGitHubClient is the pure-stdlib GitHub App REST API client.
type HTTPGitHubClient struct {
	AppID      string
	PrivateKey *rsa.PrivateKey
	APIBase    string
	HTTPClient *http.Client
	Now        func() time.Time
}

func (c *HTTPGitHubClient) baseURL() string {
	base := strings.TrimRight(strings.TrimSpace(c.APIBase), "/")
	if base == "" {
		return DefaultGitHubAPIBase
	}
	return base
}

func (c *HTTPGitHubClient) httpClient() *http.Client {
	if c.HTTPClient != nil {
		return c.HTTPClient
	}
	return &http.Client{Timeout: 20 * time.Second}
}

func (c *HTTPGitHubClient) now() time.Time {
	if c.Now != nil {
		return c.Now()
	}
	return time.Now()
}

// CreateInstallationToken exchanges an RS256 App JWT for a repository-scoped
// installation access token (`contents: read`, `checks: write`).
func (c *HTTPGitHubClient) CreateInstallationToken(ctx context.Context, installationID int64, repoName string) (string, error) {
	if installationID <= 0 {
		return "", fmt.Errorf("invalid GitHub installation ID: %d", installationID)
	}
	jwtToken, err := MintAppJWT(c.AppID, c.PrivateKey, c.now())
	if err != nil {
		return "", err
	}
	reqBody := map[string]any{
		"permissions": map[string]string{
			"contents": "read",
			"checks":   "write",
		},
	}
	if strings.TrimSpace(repoName) != "" {
		reqBody["repositories"] = []string{strings.TrimSpace(repoName)}
	}
	payload, err := json.Marshal(reqBody)
	if err != nil {
		return "", err
	}
	endpoint := fmt.Sprintf("%s/app/installations/%d/access_tokens", c.baseURL(), installationID)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, bytes.NewReader(payload))
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "Bearer "+jwtToken)
	req.Header.Set("Accept", "application/vnd.github+json")
	req.Header.Set("X-GitHub-Api-Version", "2022-11-28")
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", "devcouncil-pr-gate-app")

	resp, err := c.httpClient().Do(req)
	if err != nil {
		return "", fmt.Errorf("request installation token: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return "", fmt.Errorf("read installation token response: %w", err)
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return "", fmt.Errorf("github installation token failed (HTTP %d)", resp.StatusCode)
	}
	var parsed struct {
		Token string `json:"token"`
	}
	if err := json.Unmarshal(body, &parsed); err != nil {
		return "", fmt.Errorf("decode installation token response: %w", err)
	}
	if strings.TrimSpace(parsed.Token) == "" {
		return "", errors.New("github installation token response contained empty token")
	}
	return parsed.Token, nil
}

// CreateCheckRun publishes a Checks API check run on `owner/repo`.
func (c *HTTPGitHubClient) CreateCheckRun(ctx context.Context, token, owner, repo string, check CheckRunRequest) (*CheckRunResponse, error) {
	endpoint := fmt.Sprintf("%s/repos/%s/%s/check-runs", c.baseURL(), url.PathEscape(owner), url.PathEscape(repo))
	return c.doCheckRunRequest(ctx, http.MethodPost, endpoint, token, owner, repo, check)
}

// UpdateCheckRun completes or updates an existing Checks API check run on `owner/repo`.
func (c *HTTPGitHubClient) UpdateCheckRun(ctx context.Context, token, owner, repo string, checkRunID int64, check CheckRunRequest) (*CheckRunResponse, error) {
	if checkRunID <= 0 {
		return nil, fmt.Errorf("invalid check_run_id: %d", checkRunID)
	}
	endpoint := fmt.Sprintf("%s/repos/%s/%s/check-runs/%d", c.baseURL(), url.PathEscape(owner), url.PathEscape(repo), checkRunID)
	return c.doCheckRunRequest(ctx, http.MethodPatch, endpoint, token, owner, repo, check)
}

func (c *HTTPGitHubClient) doCheckRunRequest(ctx context.Context, method, endpoint, token, owner, repo string, check CheckRunRequest) (*CheckRunResponse, error) {
	if strings.TrimSpace(token) == "" {
		return nil, errors.New("installation token is empty")
	}
	if strings.TrimSpace(owner) == "" || strings.TrimSpace(repo) == "" {
		return nil, errors.New("owner and repo are required")
	}
	payload, err := json.Marshal(check)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, method, endpoint, bytes.NewReader(payload))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Accept", "application/vnd.github+json")
	req.Header.Set("X-GitHub-Api-Version", "2022-11-28")
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", "devcouncil-pr-gate-app")

	resp, err := c.httpClient().Do(req)
	if err != nil {
		return nil, fmt.Errorf("%s check run: %w", method, err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil, fmt.Errorf("read check run response: %w", err)
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("github %s check run failed (HTTP %d)", method, resp.StatusCode)
	}
	var out CheckRunResponse
	if err := json.Unmarshal(body, &out); err != nil {
		return nil, fmt.Errorf("decode check run response: %w", err)
	}
	return &out, nil
}
