package prgateapp

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/dc/redact"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gitrev"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/prgate"
)

const (
	// DefaultWorkerCount bounds concurrent PR clone + analysis jobs.
	DefaultWorkerCount = 4
	// DefaultQueueCapacity bounds pending accepted webhooks before returning 503.
	DefaultQueueCapacity = 64
	// DefaultJobTimeout bounds each background PR Gate execution.
	DefaultJobTimeout = 5 * time.Minute
	// DefaultRateLimitPerMinute bounds accepted webhooks per installation/repo.
	DefaultRateLimitPerMinute = 30
)

// ServerConfig configures the hosted DevCouncil PR Gate GitHub App server.
type ServerConfig struct {
	WebhookSecret      string
	GitHub             GitHubClient
	Entitlements       EntitlementChecker
	Runner             Runner
	Scrubber           *redact.Scrubber
	Deliveries         *DeliveryCache
	RateLimiter        *KeyedRateLimiter
	WorkerCount        int
	QueueCapacity      int
	JobTimeout         time.Duration
	RateLimitPerMinute int
}

type prGateJob struct {
	deliveryID string
	event      PullRequestEvent
	owner      string
	repoName   string
}

// Server serves GitHub App webhooks and runs PR Gate analyses asynchronously on
// a bounded worker pool so webhook responses always beat GitHub's 10s timeout.
type Server struct {
	cfg        ServerConfig
	deliveries *DeliveryCache
	limiter    *KeyedRateLimiter
	jobTimeout time.Duration

	mu       sync.RWMutex
	closed   bool
	jobs     chan prGateJob
	wg       sync.WaitGroup
	inFlight sync.WaitGroup
	baseCtx  context.Context
	cancel   context.CancelFunc
}

// NewServer constructs a Server and starts its bounded worker pool.
// Call Shutdown(ctx) when stopping the server to drain in-flight jobs cleanly.
func NewServer(cfg ServerConfig) *Server {
	deliveries := cfg.Deliveries
	if deliveries == nil {
		deliveries = NewDeliveryCache(DefaultDeliveryTTL, DefaultDeliveryMaxEntries)
	}
	limiter := cfg.RateLimiter
	if limiter == nil {
		rpm := cfg.RateLimitPerMinute
		if rpm <= 0 {
			rpm = DefaultRateLimitPerMinute
		}
		limiter = NewKeyedRateLimiter(rpm, rpm)
	}
	workers := cfg.WorkerCount
	if workers <= 0 {
		workers = DefaultWorkerCount
	}
	queueCap := cfg.QueueCapacity
	if queueCap <= 0 {
		queueCap = DefaultQueueCapacity
	}
	jobTimeout := cfg.JobTimeout
	if jobTimeout <= 0 {
		jobTimeout = DefaultJobTimeout
	}

	baseCtx, cancel := context.WithCancel(context.Background())
	s := &Server{
		cfg:        cfg,
		deliveries: deliveries,
		limiter:    limiter,
		jobTimeout: jobTimeout,
		jobs:       make(chan prGateJob, queueCap),
		baseCtx:    baseCtx,
		cancel:     cancel,
	}
	for i := 0; i < workers; i++ {
		s.wg.Add(1)
		go s.workerLoop()
	}
	return s
}

// WaitIdle blocks until all currently queued and running PR Gate jobs finish,
// or until ctx expires. Useful for deterministic tests and graceful draining.
func (s *Server) WaitIdle(ctx context.Context) error {
	done := make(chan struct{})
	go func() {
		s.inFlight.Wait()
		close(done)
	}()
	select {
	case <-done:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

// Shutdown stops accepting new webhook jobs and waits for active workers to
// drain cleanly until ctx expires.
func (s *Server) Shutdown(ctx context.Context) error {
	s.mu.Lock()
	if !s.closed {
		s.closed = true
		close(s.jobs)
	}
	s.mu.Unlock()

	done := make(chan struct{})
	go func() {
		s.wg.Wait()
		close(done)
	}()
	select {
	case <-done:
		s.cancel()
		return nil
	case <-ctx.Done():
		s.cancel()
		return ctx.Err()
	}
}

// Handler returns the HTTP handler exposing `GET /healthz` and
// `POST /webhooks/github`.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/healthz", s.handleHealthz)
	mux.HandleFunc("/webhooks/github", s.handleGitHubWebhook)
	return mux
}

func (s *Server) handleHealthz(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		writeJSON(w, http.StatusMethodNotAllowed, map[string]string{"error": "method_not_allowed"})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"ok":                 true,
		"webhook_configured": s.cfg.WebhookSecret != "",
	})
}

func (s *Server) handleGitHubWebhook(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		writeJSON(w, http.StatusMethodNotAllowed, map[string]string{"error": "method_not_allowed"})
		return
	}
	// Fail closed when webhook secret is unset.
	if s.cfg.WebhookSecret == "" {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "webhook_secret_unconfigured"})
		return
	}

	body, err := io.ReadAll(io.LimitReader(r.Body, MaxWebhookBodyBytes+1))
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "read_body_failed"})
		return
	}
	if len(body) > MaxWebhookBodyBytes {
		writeJSON(w, http.StatusRequestEntityTooLarge, map[string]string{"error": "payload_too_large"})
		return
	}

	sigHeader := r.Header.Get("X-Hub-Signature-256")
	if !VerifyWebhookSignature(s.cfg.WebhookSecret, body, sigHeader) {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "invalid_signature"})
		return
	}

	deliveryID := strings.TrimSpace(r.Header.Get("X-GitHub-Delivery"))
	if deliveryID == "" || len(deliveryID) > 128 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "missing_delivery_id"})
		return
	}

	eventType := strings.TrimSpace(r.Header.Get("X-GitHub-Event"))
	if eventType == "ping" {
		writeJSON(w, http.StatusOK, map[string]string{"status": "pong"})
		return
	}
	if eventType != "pull_request" {
		writeJSON(w, http.StatusOK, map[string]string{"status": "ignored_event"})
		return
	}

	var ev PullRequestEvent
	if err := json.Unmarshal(body, &ev); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_json"})
		return
	}

	switch ev.Action {
	case "opened", "synchronize", "reopened":
	default:
		writeJSON(w, http.StatusOK, map[string]string{"status": "ignored_action"})
		return
	}

	owner, repoName, ok := splitOwnerRepo(ev.Repository.FullName)
	if !ok {
		owner = strings.TrimSpace(ev.Repository.Owner.Login)
		repoName = strings.TrimSpace(ev.Repository.Name)
	}
	if owner == "" || repoName == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_repository"})
		return
	}
	if ev.Installation.ID <= 0 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "missing_installation_id"})
		return
	}
	if !gitrev.IsSHA40(ev.PullRequest.Base.SHA) || !gitrev.IsSHA40(ev.PullRequest.Head.SHA) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid_commit_sha"})
		return
	}
	if s.cfg.GitHub == nil {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "github_client_unconfigured"})
		return
	}

	// Replay check BEFORE consuming rate-limit budget or enqueueing.
	if s.deliveries.SeenOrRecord(deliveryID) {
		w.Header().Set("X-DevCouncil-Duplicate", "true")
		writeJSON(w, http.StatusOK, map[string]any{
			"status":      "duplicate_ignored",
			"duplicate":   true,
			"delivery_id": deliveryID,
		})
		return
	}

	// Per-installation + repository rate limiting (C5). On rejection, clear
	// deliveryID so a later redelivery is not permanently dropped (C4).
	rateKey := fmt.Sprintf("%d:%s/%s", ev.Installation.ID, owner, repoName)
	if !s.limiter.Allow(rateKey) {
		s.deliveries.Remove(deliveryID)
		w.Header().Set("Retry-After", "60")
		writeJSON(w, http.StatusTooManyRequests, map[string]string{"error": "rate_limited"})
		return
	}

	// Enqueue onto bounded worker pool and return 202 Accepted immediately (C1).
	if !s.enqueue(prGateJob{
		deliveryID: deliveryID,
		event:      ev,
		owner:      owner,
		repoName:   repoName,
	}) {
		s.deliveries.Remove(deliveryID)
		w.Header().Set("Retry-After", "30")
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": "worker_queue_full"})
		return
	}

	writeJSON(w, http.StatusAccepted, map[string]any{
		"status":      "accepted",
		"delivery_id": deliveryID,
	})
}

func (s *Server) enqueue(job prGateJob) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed {
		return false
	}
	s.inFlight.Add(1)
	select {
	case s.jobs <- job:
		return true
	default:
		s.inFlight.Done()
		return false
	}
}

func (s *Server) workerLoop() {
	defer s.wg.Done()
	for job := range s.jobs {
		s.processJob(job)
		s.inFlight.Done()
	}
}

func (s *Server) processJob(job prGateJob) {
	ctx, cancel := context.WithTimeout(s.baseCtx, s.jobTimeout)
	defer cancel()

	ev := job.event
	installToken, err := s.cfg.GitHub.CreateInstallationToken(ctx, ev.Installation.ID, job.repoName)
	if err != nil {
		// Transient GitHub auth failure: clear delivery ID so redelivery retries (C4).
		s.deliveries.Remove(job.deliveryID)
		return
	}

	// Post initial `in_progress` Check Run so GitHub displays live progress while
	// analysis runs asynchronously (C1).
	var checkRunID int64
	if initialResp, initErr := s.cfg.GitHub.CreateCheckRun(ctx, installToken, job.owner, job.repoName, CheckRunRequest{
		Name:    CheckRunName,
		HeadSHA: ev.PullRequest.Head.SHA,
		Status:  "in_progress",
		Output: CheckRunOutput{
			Title:   "DevCouncil PR Gate: running verification",
			Summary: "Static verification (secret scan, anti-stub rigor, DevMap blast qualification) is in progress.",
		},
	}); initErr == nil && initialResp != nil {
		checkRunID = initialResp.ID
	}

	publishFinal := func(req CheckRunRequest) {
		var postErr error
		if checkRunID > 0 {
			_, postErr = s.cfg.GitHub.UpdateCheckRun(ctx, installToken, job.owner, job.repoName, checkRunID, req)
		} else {
			_, postErr = s.cfg.GitHub.CreateCheckRun(ctx, installToken, job.owner, job.repoName, req)
		}
		if postErr != nil {
			// Clear delivery ID on upstream GitHub failure so redelivery can retry (C4).
			s.deliveries.Remove(job.deliveryID)
		}
	}

	// Entitlement gate:
	//   - Public repositories ALWAYS run free and never consult the relay.
	//   - Private / internal repositories require server-side HMAC-authenticated
	//     entitlement from `gitpulse-relay` and fail closed on denial/error.
	if !ev.Repository.IsPublic() {
		if deniedCheck := s.checkPrivateEntitlement(ctx, ev, job.owner, job.repoName); deniedCheck != nil {
			publishFinal(*deniedCheck)
			return
		}
	}

	// Dispatch pluggable PR Gate runner.
	var checkReq CheckRunRequest
	if s.cfg.Runner == nil {
		checkReq = buildDegradedCheckRun(ev.PullRequest.Head.SHA, "DevCouncil PR Gate runner is not configured")
	} else {
		prNum := ev.PullRequest.Number
		if prNum == 0 {
			prNum = ev.Number
		}
		cloneURL := strings.TrimSpace(ev.Repository.CloneURL)
		if cloneURL == "" {
			cloneURL = fmt.Sprintf("https://github.com/%s/%s.git", job.owner, job.repoName)
		}
		report, runErr := s.cfg.Runner.Run(ctx, RunRequest{
			Owner:        job.owner,
			Repo:         job.repoName,
			CloneURL:     cloneURL,
			InstallToken: installToken,
			BaseSHA:      ev.PullRequest.Base.SHA,
			HeadSHA:      ev.PullRequest.Head.SHA,
			PullNumber:   prNum,
			Private:      !ev.Repository.IsPublic(),
		})
		if runErr != nil {
			checkReq = buildDegradedCheckRun(ev.PullRequest.Head.SHA, "PR Gate evaluation failed to complete cleanly; failing closed")
		} else {
			checkReq = s.buildReportCheckRun(ctx, ev.PullRequest.Head.SHA, report)
		}
	}

	publishFinal(checkReq)
}

func (s *Server) checkPrivateEntitlement(ctx context.Context, ev PullRequestEvent, owner, repoName string) *CheckRunRequest {
	headSHA := ev.PullRequest.Head.SHA
	if s.cfg.Entitlements == nil {
		req := CheckRunRequest{
			Name:       CheckRunName,
			HeadSHA:    headSHA,
			Status:     "completed",
			Conclusion: "failure",
			Output: CheckRunOutput{
				Title:   "DevCouncil PR Gate: private repository entitlement unconfigured",
				Summary: "Private repository verification requires server-side entitlement verification via GitPulse Relay. Entitlement checking is not configured on this app instance; failing closed.",
			},
		}
		return &req
	}

	entReq := EntitlementRequest{
		HeadSHA:              headSHA,
		InstallationID:       ev.Installation.ID,
		RepositoryID:         fmt.Sprintf("%d", ev.Repository.ID),
		RepositoryName:       owner + "/" + repoName,
		RepositoryOwnerID:    ev.Repository.Owner.ID,
		RepositoryVisibility: ev.Repository.NormalizedVisibility(),
	}
	res, err := s.cfg.Entitlements.CheckPrivateRepo(ctx, entReq)
	if err == nil && res.Allowed {
		return nil
	}

	conclusion := "failure"
	title := "DevCouncil PR Gate: entitlement verification failed"
	summary := "Private repository entitlement verification could not be completed; failing closed."

	if err == nil && (res.QuotaLimited || res.TierRequired || res.ReasonCode == "pr_gate_quota_exceeded" || res.ReasonCode == "quota_exceeded" || res.ReasonCode == "feature_required" || res.ReasonCode == "account_not_found" || res.ReasonCode == "tier_required" || res.ReasonCode == "entitlement_denied") {
		conclusion = "action_required"
		if res.QuotaLimited || strings.Contains(res.ReasonCode, "quota") {
			title = "DevCouncil PR Gate: private repository quota exceeded"
			summary = "This private repository has exceeded its hosted DevCouncil PR Gate quota. Upgrade your GitPulse / DevCouncil subscription plan to resume private PR verification. Public repositories always run free."
		} else {
			title = "DevCouncil PR Gate: paid subscription required for private repositories"
			summary = "Hosted DevCouncil PR Gate requires an active Pro, Team, or Enterprise subscription for private and internal repositories. Public repositories always run 100% free."
		}
	}

	if code := sanitizeCode(res.ReasonCode); code != "" {
		summary += "\n\nReason code: `" + code + "`"
	}

	req := CheckRunRequest{
		Name:       CheckRunName,
		HeadSHA:    headSHA,
		Status:     "completed",
		Conclusion: conclusion,
		Output: CheckRunOutput{
			Title:   title,
			Summary: summary,
		},
	}
	return &req
}

func (s *Server) buildReportCheckRun(ctx context.Context, headSHA string, report prgate.Report) CheckRunRequest {
	isDegraded := len(report.Degraded) > 0 ||
		report.Summary.Degraded > 0 ||
		len(report.Blast.Degraded) > 0 ||
		report.Blast.Error ||
		report.Outcome == prgate.OutcomeError

	// CRITICAL INVARIANT: Degraded NEVER maps to `neutral` or `success`.
	conclusion := "failure"
	if report.Outcome == prgate.OutcomeSuccess && !isDegraded && report.Summary.BlockingFindings == 0 {
		conclusion = "success"
	}

	title := buildCheckTitle(report, conclusion, isDegraded)

	rendered, err := prgate.RenderScrubbed(ctx, report, s.cfg.Scrubber)
	var summary string
	if err != nil {
		// Fail closed if egress scrubbing itself fails: never leak unscrubbed output
		// and never report success when egress scrubbing failed.
		conclusion = "failure"
		title = "DevCouncil PR Gate: egress scrubbing failed (failing closed)"
		summary = "Egress redaction failed while rendering the PR Gate summary; failing closed."
	} else {
		summary = rendered.Markdown
		if len(summary) > MaxCheckRunSummaryBytes {
			summary = summary[:MaxCheckRunSummaryBytes] + "\n\n_(truncated)_"
		}
	}

	annotations := buildCheckAnnotations(report)

	return CheckRunRequest{
		Name:       CheckRunName,
		HeadSHA:    headSHA,
		Status:     "completed",
		Conclusion: conclusion,
		Output: CheckRunOutput{
			Title:       title,
			Summary:     summary,
			Annotations: annotations,
		},
	}
}

func buildDegradedCheckRun(headSHA, summary string) CheckRunRequest {
	return CheckRunRequest{
		Name:       CheckRunName,
		HeadSHA:    headSHA,
		Status:     "completed",
		Conclusion: "failure", // NEVER neutral
		Output: CheckRunOutput{
			Title:   "DevCouncil PR Gate: verification degraded (failing closed)",
			Summary: summary,
		},
	}
}

func buildCheckTitle(report prgate.Report, conclusion string, isDegraded bool) string {
	if isDegraded {
		return fmt.Sprintf("DevCouncil PR Gate: degraded (%d incomplete check(s); failing closed)", max(len(report.Degraded), report.Summary.Degraded))
	}
	if conclusion == "success" {
		return fmt.Sprintf("DevCouncil PR Gate: passed (%d file(s) verified)", report.Summary.ChangedFiles)
	}
	return fmt.Sprintf("DevCouncil PR Gate: blocked (%d blocking finding(s))", report.Summary.BlockingFindings)
}

func buildCheckAnnotations(report prgate.Report) []CheckRunAnnotation {
	var out []CheckRunAnnotation
	for _, f := range report.Findings {
		if len(out) >= MaxCheckRunAnnotations {
			break
		}
		if f.Path == "" || f.Path == "[withheld-secret-path]" {
			continue
		}
		if err := gitrev.CheckRepoPath(f.Path); err != nil {
			continue
		}
		line := f.Line
		if line <= 0 {
			line = 1
		}
		level := "warning"
		if f.Blocking || f.HardSafety {
			level = "failure"
		}
		msg := sanitizeAnnotationText(f.Message)
		if msg == "" {
			msg = "DevCouncil PR Gate finding (" + sanitizeCode(f.GapType) + ")"
		}
		out = append(out, CheckRunAnnotation{
			Path:            f.Path,
			StartLine:       line,
			EndLine:         line,
			AnnotationLevel: level,
			Title:           "devcouncil/" + sanitizeCode(f.GapType),
			Message:         msg,
		})
	}
	for _, d := range report.Degraded {
		if len(out) >= MaxCheckRunAnnotations {
			break
		}
		if d.Path == "" {
			continue
		}
		if err := gitrev.CheckRepoPath(d.Path); err != nil {
			continue
		}
		out = append(out, CheckRunAnnotation{
			Path:            d.Path,
			StartLine:       1,
			EndLine:         1,
			AnnotationLevel: "failure",
			Title:           "devcouncil/degraded/" + sanitizeCode(d.Code),
			Message:         sanitizeAnnotationText(d.Message),
		})
	}
	return out
}

func sanitizeAnnotationText(raw string) string {
	var b strings.Builder
	for _, r := range raw {
		if b.Len() >= 240 {
			break
		}
		if r < 0x20 || r == 0x7f {
			b.WriteByte(' ')
		} else {
			b.WriteRune(r)
		}
	}
	return strings.TrimSpace(b.String())
}

func splitOwnerRepo(fullName string) (string, string, bool) {
	owner, repo, ok := strings.Cut(strings.TrimSpace(fullName), "/")
	if !ok || owner == "" || repo == "" || strings.Contains(repo, "/") {
		return "", "", false
	}
	return owner, repo, true
}

func writeJSON(w http.ResponseWriter, status int, payload any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(payload)
}
