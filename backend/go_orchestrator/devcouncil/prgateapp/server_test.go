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
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/prgate"
)

const (
	testBaseSHA   = "1111111111111111111111111111111111111111"
	testHeadSHA   = "2222222222222222222222222222222222222222"
	testSecret    = "whsec_test_super_secret_key_12345"
	testAppSecret = "test-pr-gate-app-secret-0123456789"
)

type fakeGitHub struct {
	mu            sync.Mutex
	tokenCalls    int
	createCalls   []CheckRunRequest
	updateCalls   []CheckRunRequest
	tokenToReturn string
	tokenErr      error
	checkErr      error
}

func (f *fakeGitHub) CreateInstallationToken(_ context.Context, _ int64, _ string) (string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.tokenCalls++
	if f.tokenErr != nil {
		return "", f.tokenErr
	}
	if f.tokenToReturn != "" {
		return f.tokenToReturn, nil
	}
	return "ghs_test_installation_token", nil
}

func (f *fakeGitHub) CreateCheckRun(_ context.Context, _, _, _ string, req CheckRunRequest) (*CheckRunResponse, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.checkErr != nil {
		return nil, f.checkErr
	}
	f.createCalls = append(f.createCalls, req)
	return &CheckRunResponse{ID: int64(100 + len(f.createCalls))}, nil
}

func (f *fakeGitHub) UpdateCheckRun(_ context.Context, _, _, _ string, id int64, req CheckRunRequest) (*CheckRunResponse, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.checkErr != nil {
		return nil, f.checkErr
	}
	f.updateCalls = append(f.updateCalls, req)
	return &CheckRunResponse{ID: id}, nil
}

func (f *fakeGitHub) finalCheckRuns() []CheckRunRequest {
	f.mu.Lock()
	defer f.mu.Unlock()
	out := make([]CheckRunRequest, len(f.updateCalls))
	copy(out, f.updateCalls)
	return out
}

type fakeEntitlement struct {
	mu     sync.Mutex
	calls  []EntitlementRequest
	result EntitlementResult
	err    error
}

func (f *fakeEntitlement) CheckPrivateRepo(_ context.Context, req EntitlementRequest) (EntitlementResult, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, req)
	return f.result, f.err
}

type fakeRunner struct {
	mu     sync.Mutex
	calls  []RunRequest
	report prgate.Report
	err    error
	block  chan struct{}
}

func (f *fakeRunner) Run(ctx context.Context, req RunRequest) (prgate.Report, error) {
	if f.block != nil {
		select {
		case <-f.block:
		case <-ctx.Done():
			return prgate.Report{}, ctx.Err()
		}
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, req)
	return f.report, f.err
}

func makePRPayload(t *testing.T, private bool, visibility string) []byte {
	t.Helper()
	ev := PullRequestEvent{
		Action: "opened",
		Number: 42,
		PullRequest: PullRequestBody{
			Number: 42,
			State:  "open",
			Base:   GitCommitRef{SHA: testBaseSHA},
			Head:   GitCommitRef{SHA: testHeadSHA},
		},
		Repository: RepositoryBody{
			ID:         42,
			Name:       "widget",
			FullName:   "acme/widget",
			Private:    private,
			Visibility: visibility,
			CloneURL:   "https://github.com/acme/widget.git",
			Owner: AccountOwner{
				ID:    1001,
				Login: "acme",
				Type:  "Organization",
			},
		},
		Installation: InstallationRef{ID: 777},
	}
	b, err := json.Marshal(ev)
	if err != nil {
		t.Fatalf("marshal payload: %v", err)
	}
	return b
}

func postWebhook(handler http.Handler, body []byte, sig, deliveryID, eventType string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(http.MethodPost, "/webhooks/github", bytes.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	if sig != "" {
		req.Header.Set("X-Hub-Signature-256", sig)
	}
	if deliveryID != "" {
		req.Header.Set("X-GitHub-Delivery", deliveryID)
	}
	if eventType != "" {
		req.Header.Set("X-GitHub-Event", eventType)
	}
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)
	return rec
}

func waitIdle(t *testing.T, srv *Server) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := srv.WaitIdle(ctx); err != nil {
		t.Fatalf("WaitIdle timed out: %v", err)
	}
}

func passingReport() prgate.Report {
	return prgate.Report{
		Schema:  prgate.SchemaVersion,
		Outcome: prgate.OutcomeSuccess,
		Mode:    "enforce",
		BaseSHA: testBaseSHA,
		HeadSHA: testHeadSHA,
		Summary: prgate.SummaryCounts{ChangedFiles: 2},
		Blast:   prgate.BlastQualification{Available: true, Complete: true},
	}
}

// TestSharedCrossRepoEntitlementKATVector asserts the locked cross-repo C2
// HMAC-SHA256 fixture vector (#74) shared between DevCouncil and gitpulse-relay.
func TestSharedCrossRepoEntitlementKATVector(t *testing.T) {
	const (
		secret = "test-pr-gate-app-secret-0123456789"
		ts     = "1700000000"
		body   = `{"head_sha":"0123456789abcdef0123456789abcdef01234567","installation_id":777,"repository_id":"42","repository_name":"acme/private-repo","repository_owner_id":1001,"repository_visibility":"private"}`
	)
	// Verify deterministic JSON serialization matches the fixture byte-for-byte.
	marshaled, err := json.Marshal(EntitlementRequest{
		HeadSHA:              "0123456789abcdef0123456789abcdef01234567",
		InstallationID:       777,
		RepositoryID:         "42",
		RepositoryName:       "acme/private-repo",
		RepositoryOwnerID:    1001,
		RepositoryVisibility: "private",
	})
	if err != nil {
		t.Fatalf("marshal KAT request: %v", err)
	}
	if string(marshaled) != body {
		t.Fatalf("KAT JSON mismatch:\n got: %s\nwant: %s", string(marshaled), body)
	}
	sig := ComputeAppServiceSignature(secret, http.MethodPost, DefaultEntitlementPath, ts, marshaled)
	const wantSig = "sha256=3ea26854e4fcf95bcbb21683168b17dd101bac0c69a6f74dcc740769fab21f06"
	if sig != wantSig {
		t.Fatalf("KAT signature mismatch:\n got: %s\nwant: %s", sig, wantSig)
	}
	if !VerifyAppServiceSignature(secret, http.MethodPost, DefaultEntitlementPath, ts, marshaled, sig) {
		t.Fatalf("VerifyAppServiceSignature failed for valid KAT")
	}
	if VerifyAppServiceSignature("wrong-secret", http.MethodPost, DefaultEntitlementPath, ts, marshaled, sig) {
		t.Fatalf("VerifyAppServiceSignature accepted wrong secret")
	}
}

func TestWebhookSignatureInvalidAndFailClosedWhenUnset(t *testing.T) {
	body := makePRPayload(t, false, "public")

	unsetSrv := NewServer(ServerConfig{})
	defer unsetSrv.Shutdown(context.Background())
	rec := postWebhook(unsetSrv.Handler(), body, ComputeWebhookSignature("any", body), "deliv-unset", "pull_request")
	if rec.Code != http.StatusServiceUnavailable {
		t.Fatalf("expected 503 when secret unset, got %d (%s)", rec.Code, rec.Body.String())
	}

	gh := &fakeGitHub{}
	runner := &fakeRunner{report: passingReport()}
	srv := NewServer(ServerConfig{
		WebhookSecret: testSecret,
		GitHub:        gh,
		Runner:        runner,
	})
	defer srv.Shutdown(context.Background())

	rec = postWebhook(srv.Handler(), body, "sha256="+strings.Repeat("a", 64), "deliv-bad-sig", "pull_request")
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("expected 401 on bad signature, got %d (%s)", rec.Code, rec.Body.String())
	}

	rec = postWebhook(srv.Handler(), body, "", "deliv-missing-sig", "pull_request")
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("expected 401 on missing signature, got %d (%s)", rec.Code, rec.Body.String())
	}
	waitIdle(t, srv)
	if len(runner.calls) != 0 || len(gh.createCalls) != 0 {
		t.Fatalf("invalid signatures must never invoke runner or GitHub API")
	}
}

func TestWebhookAsync202AcceptedAndDuplicateDeliveryReplayIgnored(t *testing.T) {
	body := makePRPayload(t, false, "public")
	sig := ComputeWebhookSignature(testSecret, body)
	gh := &fakeGitHub{}
	runner := &fakeRunner{report: passingReport()}
	srv := NewServer(ServerConfig{
		WebhookSecret: testSecret,
		GitHub:        gh,
		Runner:        runner,
	})
	defer srv.Shutdown(context.Background())

	rec1 := postWebhook(srv.Handler(), body, sig, "delivery-replay-1", "pull_request")
	if rec1.Code != http.StatusAccepted {
		t.Fatalf("first delivery expected 202 Accepted, got %d (%s)", rec1.Code, rec1.Body.String())
	}
	rec2 := postWebhook(srv.Handler(), body, sig, "delivery-replay-1", "pull_request")
	if rec2.Code != http.StatusOK {
		t.Fatalf("replay delivery expected 200 duplicate_ignored, got %d (%s)", rec2.Code, rec2.Body.String())
	}
	if rec2.Header().Get("X-DevCouncil-Duplicate") != "true" {
		t.Fatalf("expected X-DevCouncil-Duplicate: true on replay, got %q", rec2.Header().Get("X-DevCouncil-Duplicate"))
	}
	waitIdle(t, srv)
	if len(runner.calls) != 1 || len(gh.createCalls) != 1 || len(gh.finalCheckRuns()) != 1 {
		t.Fatalf("expected 1 runner, 1 initial in_progress check, 1 final update; got runner=%d create=%d update=%d",
			len(runner.calls), len(gh.createCalls), len(gh.finalCheckRuns()))
	}
	if gh.createCalls[0].Status != "in_progress" {
		t.Fatalf("expected initial check run status in_progress, got %q", gh.createCalls[0].Status)
	}
	if gh.finalCheckRuns()[0].Conclusion != "success" {
		t.Fatalf("expected final conclusion success, got %q", gh.finalCheckRuns()[0].Conclusion)
	}
}

func TestTransientFailureClearsDeliveryCacheForRedelivery(t *testing.T) {
	body := makePRPayload(t, false, "public")
	sig := ComputeWebhookSignature(testSecret, body)
	gh := &fakeGitHub{tokenErr: errors.New("github 502")}
	runner := &fakeRunner{report: passingReport()}
	srv := NewServer(ServerConfig{
		WebhookSecret: testSecret,
		GitHub:        gh,
		Runner:        runner,
	})
	defer srv.Shutdown(context.Background())

	rec1 := postWebhook(srv.Handler(), body, sig, "delivery-retry-1", "pull_request")
	if rec1.Code != http.StatusAccepted {
		t.Fatalf("expected 202, got %d", rec1.Code)
	}
	waitIdle(t, srv)
	if len(runner.calls) != 0 {
		t.Fatalf("runner should not run when token failed")
	}

	// Clear error and redeliver the exact same delivery ID: must be accepted & processed (C4).
	gh.mu.Lock()
	gh.tokenErr = nil
	gh.mu.Unlock()

	rec2 := postWebhook(srv.Handler(), body, sig, "delivery-retry-1", "pull_request")
	if rec2.Code != http.StatusAccepted {
		t.Fatalf("redelivery after failure must be accepted (202), got %d (%s)", rec2.Code, rec2.Body.String())
	}
	waitIdle(t, srv)
	if len(gh.finalCheckRuns()) != 1 || gh.finalCheckRuns()[0].Conclusion != "success" {
		t.Fatalf("expected successful redelivery check run, got %+v", gh.finalCheckRuns())
	}
}

func TestRateLimitAndQueueBackpressureFailClosed(t *testing.T) {
	body := makePRPayload(t, false, "public")
	sig := ComputeWebhookSignature(testSecret, body)

	// 1. Per-installation rate limit (C5)
	gh := &fakeGitHub{}
	runner := &fakeRunner{report: passingReport()}
	srv := NewServer(ServerConfig{
		WebhookSecret:      testSecret,
		GitHub:             gh,
		Runner:             runner,
		RateLimitPerMinute: 1,
	})
	defer srv.Shutdown(context.Background())

	rec1 := postWebhook(srv.Handler(), body, sig, "rate-1", "pull_request")
	if rec1.Code != http.StatusAccepted {
		t.Fatalf("first request expected 202, got %d", rec1.Code)
	}
	rec2 := postWebhook(srv.Handler(), body, sig, "rate-2", "pull_request")
	if rec2.Code != http.StatusTooManyRequests {
		t.Fatalf("second request expected 429 rate_limited, got %d (%s)", rec2.Code, rec2.Body.String())
	}
	waitIdle(t, srv)

	// 2. Queue saturation returns 503 and clears delivery ID (C4 + C5)
	blocker := make(chan struct{})
	qRunner := &fakeRunner{report: passingReport(), block: blocker}
	qSrv := NewServer(ServerConfig{
		WebhookSecret: testSecret,
		GitHub:        &fakeGitHub{},
		Runner:        qRunner,
		WorkerCount:   1,
		QueueCapacity: 1,
	})
	defer func() {
		close(blocker)
		_ = qSrv.Shutdown(context.Background())
	}()

	// Job 1 enters worker; Job 2 fills queue buffer of size 1; Job 3 must 503.
	_ = postWebhook(qSrv.Handler(), body, sig, "q-1", "pull_request")
	time.Sleep(30 * time.Millisecond)
	_ = postWebhook(qSrv.Handler(), body, sig, "q-2", "pull_request")
	rec3 := postWebhook(qSrv.Handler(), body, sig, "q-3", "pull_request")
	if rec3.Code != http.StatusServiceUnavailable {
		t.Fatalf("overflow request expected 503 worker_queue_full, got %d (%s)", rec3.Code, rec3.Body.String())
	}
}

func TestAllowlistedSubprocessEnvStripsHostSecrets(t *testing.T) {
	t.Setenv("DEVCOUNCIL_WEBHOOK_SECRET", "top-secret-webhook")
	t.Setenv("GITPULSE_PR_GATE_APP_SECRET", "top-secret-relay")
	t.Setenv("DEVCOUNCIL_RELAY_TOKEN", "top-secret-token")
	t.Setenv("DEVCOUNCIL_APP_PRIVATE_KEY_FILE", "/etc/secrets/app.pem")

	env := allowlistedSubprocessEnv("/tmp/isolated-home", "/tmp/isolated-tmp")
	joined := strings.Join(env, "\n")
	for _, forbidden := range []string{
		"DEVCOUNCIL_WEBHOOK_SECRET",
		"GITPULSE_PR_GATE_APP_SECRET",
		"DEVCOUNCIL_RELAY_TOKEN",
		"DEVCOUNCIL_APP_PRIVATE_KEY_FILE",
		"top-secret",
	} {
		if strings.Contains(joined, forbidden) {
			t.Fatalf("allowlistedSubprocessEnv leaked %q in env:\n%s", forbidden, joined)
		}
	}
	if !strings.Contains(joined, "HOME=/tmp/isolated-home") {
		t.Fatalf("expected isolated HOME in env: %s", joined)
	}
}

func TestPublicPRRunsFreeWithoutCallingRelayEntitlement(t *testing.T) {
	body := makePRPayload(t, false, "public")
	sig := ComputeWebhookSignature(testSecret, body)
	gh := &fakeGitHub{}
	ent := &fakeEntitlement{result: EntitlementResult{Allowed: false, ReasonCode: "should_not_be_called"}}
	runner := &fakeRunner{report: passingReport()}
	srv := NewServer(ServerConfig{
		WebhookSecret: testSecret,
		GitHub:        gh,
		Entitlements:  ent,
		Runner:        runner,
	})
	defer srv.Shutdown(context.Background())

	rec := postWebhook(srv.Handler(), body, sig, "delivery-public-1", "pull_request")
	if rec.Code != http.StatusAccepted {
		t.Fatalf("expected 202, got %d (%s)", rec.Code, rec.Body.String())
	}
	waitIdle(t, srv)
	if len(ent.calls) != 0 {
		t.Fatalf("public repo must NEVER call relay entitlement checker, got %d calls", len(ent.calls))
	}
	if len(runner.calls) != 1 {
		t.Fatalf("expected 1 runner call, got %d", len(runner.calls))
	}
	finals := gh.finalCheckRuns()
	if len(finals) != 1 {
		t.Fatalf("expected 1 completed check run, got %d", len(finals))
	}
	cr := finals[0]
	if cr.Name != CheckRunName || cr.Conclusion != "success" || cr.HeadSHA != testHeadSHA {
		t.Fatalf("unexpected check run: %+v", cr)
	}
}

func TestPrivatePREntitledPassesAndPostsSanitizedAnnotations(t *testing.T) {
	body := makePRPayload(t, true, "private")
	sig := ComputeWebhookSignature(testSecret, body)
	gh := &fakeGitHub{}
	ent := &fakeEntitlement{result: EntitlementResult{Allowed: true, Tier: "team"}}
	rep := passingReport()
	rep.Outcome = prgate.OutcomeFailure
	rep.Summary.BlockingFindings = 1
	rep.Findings = []prgate.FindingItem{
		{
			ID:       "f1",
			GapType:  "stub_or_placeholder",
			Severity: "high",
			Blocking: true,
			Path:     "pkg/billing/calc.go",
			Line:     27,
			Message:  "Unimplemented stub detected\nin line 27",
		},
	}
	runner := &fakeRunner{report: rep}
	srv := NewServer(ServerConfig{
		WebhookSecret: testSecret,
		GitHub:        gh,
		Entitlements:  ent,
		Runner:        runner,
	})
	defer srv.Shutdown(context.Background())

	rec := postWebhook(srv.Handler(), body, sig, "delivery-priv-ok", "pull_request")
	if rec.Code != http.StatusAccepted {
		t.Fatalf("expected 202, got %d (%s)", rec.Code, rec.Body.String())
	}
	waitIdle(t, srv)
	if len(ent.calls) != 1 || ent.calls[0].RepositoryName != "acme/widget" || ent.calls[0].RepositoryID != "42" || ent.calls[0].RepositoryOwnerID != 1001 || ent.calls[0].HeadSHA != testHeadSHA {
		t.Fatalf("unexpected entitlement call: %+v", ent.calls)
	}
	if len(runner.calls) != 1 {
		t.Fatalf("expected runner to execute for entitled private repo")
	}
	cr := gh.finalCheckRuns()[0]
	if cr.Conclusion != "failure" {
		t.Fatalf("expected blocking finding conclusion failure, got %q", cr.Conclusion)
	}
	if len(cr.Output.Annotations) != 1 || cr.Output.Annotations[0].Path != "pkg/billing/calc.go" || cr.Output.Annotations[0].StartLine != 27 {
		t.Fatalf("unexpected annotations: %+v", cr.Output.Annotations)
	}
	if strings.Contains(cr.Output.Annotations[0].Message, "\n") {
		t.Fatalf("annotation message must strip control characters: %q", cr.Output.Annotations[0].Message)
	}
}

func TestPrivatePRQuotaAndTierDeniedFailClosed(t *testing.T) {
	var mu sync.Mutex
	var relayStatus int
	var relayPayload string
	relaySrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != DefaultEntitlementPath {
			t.Errorf("unexpected relay path %q", r.URL.Path)
			w.WriteHeader(http.StatusNotFound)
			return
		}
		rawBody, _ := io.ReadAll(r.Body)
		ts := r.Header.Get(HeaderAppTimestamp)
		sig := r.Header.Get(HeaderAppSignature)
		if !VerifyAppServiceSignature(testAppSecret, r.Method, r.URL.Path, ts, rawBody, sig) {
			t.Errorf("relay HMAC verification failed: ts=%q sig=%q", ts, sig)
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		var req EntitlementRequest
		if err := json.Unmarshal(rawBody, &req); err != nil || req.RepositoryID != "42" || req.RepositoryOwnerID != 1001 || req.HeadSHA != testHeadSHA {
			t.Errorf("unexpected entitlement payload: %s", string(rawBody))
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		mu.Lock()
		st, pl := relayStatus, relayPayload
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(st)
		_, _ = w.Write([]byte(pl))
	}))
	defer relaySrv.Close()

	entClient := &RelayEntitlementClient{
		BaseURL:      relaySrv.URL,
		ServiceToken: testAppSecret,
	}

	cases := []struct {
		name           string
		status         int
		payload        string
		wantConclusion string
	}{
		{
			name:           "feature_required_403",
			status:         http.StatusForbidden,
			payload:        `{"allowed":false,"error":"feature_required","tier_required":true}`,
			wantConclusion: "action_required",
		},
		{
			name:           "quota_exceeded_429",
			status:         http.StatusTooManyRequests,
			payload:        `{"allowed":false,"error":"pr_gate_quota_exceeded","quota_limited":true}`,
			wantConclusion: "action_required",
		},
		{
			name:           "relay_500_fail_closed",
			status:         http.StatusInternalServerError,
			payload:        `{"error":"internal"}`,
			wantConclusion: "failure",
		},
	}

	for i, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			mu.Lock()
			relayStatus = tc.status
			relayPayload = tc.payload
			mu.Unlock()

			gh := &fakeGitHub{}
			runner := &fakeRunner{report: passingReport()}
			srv := NewServer(ServerConfig{
				WebhookSecret: testSecret,
				GitHub:        gh,
				Entitlements:  entClient,
				Runner:        runner,
			})
			defer srv.Shutdown(context.Background())

			body := makePRPayload(t, true, "private")
			sig := ComputeWebhookSignature(testSecret, body)
			rec := postWebhook(srv.Handler(), body, sig, "delivery-deny-"+string(rune('a'+i)), "pull_request")
			if rec.Code != http.StatusAccepted {
				t.Fatalf("expected 202, got %d (%s)", rec.Code, rec.Body.String())
			}
			waitIdle(t, srv)
			if len(runner.calls) != 0 {
				t.Fatalf("unentitled private PR must NEVER invoke runner")
			}
			finals := gh.finalCheckRuns()
			if len(finals) != 1 {
				t.Fatalf("expected 1 completed check run posted, got %d", len(finals))
			}
			got := finals[0].Conclusion
			if got != tc.wantConclusion {
				t.Fatalf("expected conclusion %q, got %q", tc.wantConclusion, got)
			}
			if got == "neutral" || got == "success" {
				t.Fatalf("denied private PR must never conclude %q", got)
			}
		})
	}
}

func TestDegradedEvaluationAlwaysConcludesFailureNeverNeutral(t *testing.T) {
	cases := []struct {
		name   string
		report prgate.Report
		runErr error
	}{
		{
			name: "degraded_outcome_error",
			report: prgate.Report{
				Schema:  prgate.SchemaVersion,
				Outcome: prgate.OutcomeError,
				Mode:    "enforce",
				BaseSHA: testBaseSHA,
				HeadSHA: testHeadSHA,
				Summary: prgate.SummaryCounts{Degraded: 1},
				Degraded: []prgate.DegradedItem{
					{Code: "devmap_unavailable", Message: "devmap build timed out"},
				},
			},
		},
		{
			name: "degraded_items_even_if_outcome_claims_success",
			report: prgate.Report{
				Schema:  prgate.SchemaVersion,
				Outcome: prgate.OutcomeSuccess,
				Mode:    "enforce",
				BaseSHA: testBaseSHA,
				HeadSHA: testHeadSHA,
				Summary: prgate.SummaryCounts{Degraded: 1},
				Degraded: []prgate.DegradedItem{
					{Code: "blast_incomplete", Path: "pkg/core.go", Message: "walk incomplete"},
				},
			},
		},
		{
			name:   "runner_process_error",
			runErr: errors.New("shallow clone timeout"),
		},
	}

	for i, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			gh := &fakeGitHub{}
			runner := &fakeRunner{report: tc.report, err: tc.runErr}
			srv := NewServer(ServerConfig{
				WebhookSecret: testSecret,
				GitHub:        gh,
				Runner:        runner,
			})
			defer srv.Shutdown(context.Background())

			body := makePRPayload(t, false, "public")
			sig := ComputeWebhookSignature(testSecret, body)
			rec := postWebhook(srv.Handler(), body, sig, "delivery-deg-"+string(rune('a'+i)), "pull_request")
			if rec.Code != http.StatusAccepted {
				t.Fatalf("expected 202, got %d (%s)", rec.Code, rec.Body.String())
			}
			waitIdle(t, srv)
			finals := gh.finalCheckRuns()
			if len(finals) != 1 {
				t.Fatalf("expected 1 check run, got %d", len(finals))
			}
			cr := finals[0]
			if cr.Conclusion != "failure" {
				t.Fatalf("degraded MUST conclude 'failure' (NEVER 'neutral' or 'success'), got %q", cr.Conclusion)
			}
		})
	}
}

func TestHTTPGitHubClientRS256JWTAndChecksAPI(t *testing.T) {
	privKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatalf("generate RSA key: %v", err)
	}
	pemBytes := pem.EncodeToMemory(&pem.Block{
		Type:  "RSA PRIVATE KEY",
		Bytes: x509.MarshalPKCS1PrivateKey(privKey),
	})
	parsedKey, err := ParseRSAPrivateKeyPEM(pemBytes)
	if err != nil {
		t.Fatalf("parse PEM key: %v", err)
	}

	var gotCreate, gotUpdate CheckRunRequest
	ghSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == http.MethodPost && r.URL.Path == "/app/installations/777/access_tokens":
			auth := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
			parts := strings.Split(auth, ".")
			if len(parts) != 3 {
				t.Errorf("malformed JWT: %q", auth)
				w.WriteHeader(http.StatusUnauthorized)
				return
			}
			signingInput := parts[0] + "." + parts[1]
			sigBytes, err := base64.RawURLEncoding.DecodeString(parts[2])
			if err != nil {
				t.Errorf("decode JWT sig: %v", err)
				w.WriteHeader(http.StatusUnauthorized)
				return
			}
			digest := sha256.Sum256([]byte(signingInput))
			if err := rsa.VerifyPKCS1v15(&privKey.PublicKey, crypto.SHA256, digest[:], sigBytes); err != nil {
				t.Errorf("RS256 JWT signature verification failed: %v", err)
				w.WriteHeader(http.StatusUnauthorized)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusCreated)
			_, _ = w.Write([]byte(`{"token":"ghs_minted_scoped_token"}`))

		case r.Method == http.MethodPost && r.URL.Path == "/repos/acme/widget/check-runs":
			if r.Header.Get("Authorization") != "Bearer ghs_minted_scoped_token" {
				w.WriteHeader(http.StatusUnauthorized)
				return
			}
			if err := json.NewDecoder(r.Body).Decode(&gotCreate); err != nil {
				t.Errorf("decode create check run: %v", err)
				w.WriteHeader(http.StatusBadRequest)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusCreated)
			_, _ = w.Write([]byte(`{"id":55501,"html_url":"https://github.com/acme/widget/runs/55501"}`))

		case r.Method == http.MethodPatch && r.URL.Path == "/repos/acme/widget/check-runs/55501":
			if r.Header.Get("Authorization") != "Bearer ghs_minted_scoped_token" {
				w.WriteHeader(http.StatusUnauthorized)
				return
			}
			if err := json.NewDecoder(r.Body).Decode(&gotUpdate); err != nil {
				t.Errorf("decode update check run: %v", err)
				w.WriteHeader(http.StatusBadRequest)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write([]byte(`{"id":55501,"html_url":"https://github.com/acme/widget/runs/55501"}`))

		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer ghSrv.Close()

	client := &HTTPGitHubClient{
		AppID:      "123456",
		PrivateKey: parsedKey,
		APIBase:    ghSrv.URL,
		Now:        func() time.Time { return time.Unix(1_700_000_000, 0) },
	}
	token, err := client.CreateInstallationToken(context.Background(), 777, "widget")
	if err != nil {
		t.Fatalf("CreateInstallationToken: %v", err)
	}
	if token != "ghs_minted_scoped_token" {
		t.Fatalf("unexpected token %q", token)
	}
	created, err := client.CreateCheckRun(context.Background(), token, "acme", "widget", CheckRunRequest{
		Name:    CheckRunName,
		HeadSHA: testHeadSHA,
		Status:  "in_progress",
		Output:  CheckRunOutput{Title: "Running", Summary: "In progress"},
	})
	if err != nil {
		t.Fatalf("CreateCheckRun: %v", err)
	}
	updated, err := client.UpdateCheckRun(context.Background(), token, "acme", "widget", created.ID, CheckRunRequest{
		Name:       CheckRunName,
		HeadSHA:    testHeadSHA,
		Status:     "completed",
		Conclusion: "success",
		Output:     CheckRunOutput{Title: "Passed", Summary: "All static gates passed."},
	})
	if err != nil {
		t.Fatalf("UpdateCheckRun: %v", err)
	}
	if created.ID != 55501 || updated.ID != 55501 || gotCreate.Status != "in_progress" || gotUpdate.Conclusion != "success" {
		t.Fatalf("unexpected check run exchange: created=%+v updated=%+v create=%+v update=%+v", created, updated, gotCreate, gotUpdate)
	}
}
