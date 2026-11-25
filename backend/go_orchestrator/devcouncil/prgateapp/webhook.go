// Package prgateapp implements the hosted DevCouncil PR Gate GitHub App
// webhook server using only the Go standard library.
//
// Security invariants:
//   - Webhook HMAC-SHA256 verification (`X-Hub-Signature-256`) uses `hmac.Equal`
//     and fails closed whenever the webhook secret is unset.
//   - `X-GitHub-Delivery` replay protection drops duplicate deliveries before
//     any GitHub API, relay entitlement, or runner work occurs, and clears the
//     delivery ID on transient failure so GitHub redeliveries can retry.
//   - DeliveryCache is held in-memory per replica (single-replica deployment;
//     restarts clear recent delivery IDs, which is safe because Check Runs are
//     idempotent per commit SHA).
//   - Webhook deliveries return `202 Accepted` immediately and execute on a
//     bounded worker pool with per-installation/repo rate limiting so long
//     analyses never hit GitHub's 10-second webhook delivery timeout.
//   - Public repositories always run 100% free and bypass relay entitlement.
//   - Private and internal repositories require server-side HMAC-authenticated
//     entitlement from `gitpulse-relay` and fail closed (`action_required` /
//     `failure`) when unentitled, over quota, or unconfigured.
//   - Degraded gate evaluations ALWAYS conclude as `failure` — NEVER `neutral`.
package prgateapp

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"strings"
	"sync"
	"time"
)

const (
	// MaxWebhookBodyBytes bounds incoming webhook payload reads.
	MaxWebhookBodyBytes = 1 << 20 // 1 MiB
	// DefaultDeliveryTTL is how long delivery IDs remain in the replay cache.
	DefaultDeliveryTTL = 24 * time.Hour
	// DefaultDeliveryMaxEntries caps memory growth of the replay cache.
	DefaultDeliveryMaxEntries = 10_000
)

// PullRequestEvent is the subset of GitHub's `pull_request` webhook payload
// consumed by the PR Gate GitHub App.
type PullRequestEvent struct {
	Action       string          `json:"action"`
	Number       int             `json:"number"`
	PullRequest  PullRequestBody `json:"pull_request"`
	Repository   RepositoryBody  `json:"repository"`
	Installation InstallationRef `json:"installation"`
}

// PullRequestBody carries the PR base and head commits.
type PullRequestBody struct {
	Number int          `json:"number"`
	State  string       `json:"state"`
	Base   GitCommitRef `json:"base"`
	Head   GitCommitRef `json:"head"`
}

// GitCommitRef identifies a commit SHA and its repository.
type GitCommitRef struct {
	SHA  string          `json:"sha"`
	Repo *RepositoryBody `json:"repo,omitempty"`
}

// RepositoryBody identifies the target repository and its visibility.
type RepositoryBody struct {
	ID         int64        `json:"id"`
	Name       string       `json:"name"`
	FullName   string       `json:"full_name"`
	Private    bool         `json:"private"`
	Visibility string       `json:"visibility"`
	CloneURL   string       `json:"clone_url"`
	Owner      AccountOwner `json:"owner"`
}

// AccountOwner identifies the repository owner (user or organization).
type AccountOwner struct {
	ID    int64  `json:"id"`
	Login string `json:"login"`
	Type  string `json:"type"`
}

// InstallationRef carries the GitHub App installation ID.
type InstallationRef struct {
	ID int64 `json:"id"`
}

// IsPublic reports whether the repository is unambiguously public.
// Fail-closed: any repository marked `private: true` or with non-public
// visibility (e.g. `private` or `internal`) returns false.
func (r RepositoryBody) IsPublic() bool {
	if r.Private {
		return false
	}
	vis := strings.ToLower(strings.TrimSpace(r.Visibility))
	return vis == "" || vis == "public"
}

// NormalizedVisibility returns `"public"`, `"private"`, or `"internal"`.
func (r RepositoryBody) NormalizedVisibility() string {
	vis := strings.ToLower(strings.TrimSpace(r.Visibility))
	if vis == "internal" {
		return "internal"
	}
	if r.Private || vis == "private" {
		return "private"
	}
	return "public"
}

// VerifyWebhookSignature validates GitHub's `X-Hub-Signature-256` header
// (`sha256=<64-hex>`) in constant time. Fails closed when secret is empty.
func VerifyWebhookSignature(secret string, body []byte, sigHeader string) bool {
	if secret == "" {
		return false
	}
	const prefix = "sha256="
	if !strings.HasPrefix(sigHeader, prefix) {
		return false
	}
	hexSig := strings.TrimSpace(sigHeader[len(prefix):])
	if len(hexSig) != 64 {
		return false
	}
	gotMAC, err := hex.DecodeString(hexSig)
	if err != nil || len(gotMAC) != sha256.Size {
		return false
	}
	mac := hmac.New(sha256.New, []byte(secret))
	_, _ = mac.Write(body)
	expectedMAC := mac.Sum(nil)
	return hmac.Equal(gotMAC, expectedMAC)
}

// ComputeWebhookSignature formats a valid `X-Hub-Signature-256` header value.
func ComputeWebhookSignature(secret string, body []byte) string {
	mac := hmac.New(sha256.New, []byte(secret))
	_, _ = mac.Write(body)
	return "sha256=" + hex.EncodeToString(mac.Sum(nil))
}

// DeliveryCache is a concurrency-safe, bounded in-memory TTL replay cache for
// `X-GitHub-Delivery` headers (scoped to a single app replica).
type DeliveryCache struct {
	mu         sync.Mutex
	ttl        time.Duration
	maxEntries int
	now        func() time.Time
	entries    map[string]time.Time
	order      []string
}

// NewDeliveryCache constructs a DeliveryCache with sensible bounds.
func NewDeliveryCache(ttl time.Duration, maxEntries int) *DeliveryCache {
	if ttl <= 0 {
		ttl = DefaultDeliveryTTL
	}
	if maxEntries <= 0 {
		maxEntries = DefaultDeliveryMaxEntries
	}
	return &DeliveryCache{
		ttl:        ttl,
		maxEntries: maxEntries,
		now:        time.Now,
		entries:    make(map[string]time.Time),
	}
}

// SeenOrRecord returns true if deliveryID was already observed within the TTL
// window. Otherwise it records deliveryID and returns false.
func (c *DeliveryCache) SeenOrRecord(deliveryID string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()

	now := c.now()
	c.evictExpiredLocked(now)

	if exp, ok := c.entries[deliveryID]; ok && now.Before(exp) {
		return true
	}
	if len(c.entries) >= c.maxEntries {
		c.evictOldestLocked()
	}
	c.entries[deliveryID] = now.Add(c.ttl)
	c.order = append(c.order, deliveryID)
	return false
}

// Remove deletes deliveryID from the cache so a transient failure (queue full,
// upstream GitHub 5xx) does not permanently suppress GitHub redeliveries.
func (c *DeliveryCache) Remove(deliveryID string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.entries, deliveryID)
}

func (c *DeliveryCache) evictExpiredLocked(now time.Time) {
	keep := c.order[:0]
	for _, id := range c.order {
		if exp, ok := c.entries[id]; ok {
			if now.Before(exp) {
				keep = append(keep, id)
			} else {
				delete(c.entries, id)
			}
		}
	}
	c.order = keep
}

func (c *DeliveryCache) evictOldestLocked() {
	for len(c.order) > 0 && len(c.entries) >= c.maxEntries {
		oldest := c.order[0]
		c.order = c.order[1:]
		delete(c.entries, oldest)
	}
}

// KeyedRateLimiter is a bounded per-key token bucket limiter protecting
// clone/analysis workers from webhook floods on public or private repos.
type KeyedRateLimiter struct {
	mu         sync.Mutex
	ratePerSec float64
	burst      float64
	maxKeys    int
	now        func() time.Time
	buckets    map[string]*tokenBucket
}

type tokenBucket struct {
	tokens   float64
	lastSeen time.Time
}

// NewKeyedRateLimiter creates a per-key token bucket limiter.
func NewKeyedRateLimiter(perMinute int, burst int) *KeyedRateLimiter {
	if perMinute <= 0 {
		perMinute = 30
	}
	if burst <= 0 {
		burst = perMinute
	}
	return &KeyedRateLimiter{
		ratePerSec: float64(perMinute) / 60.0,
		burst:      float64(burst),
		maxKeys:    10_000,
		now:        time.Now,
		buckets:    make(map[string]*tokenBucket),
	}
}

// Allow consumes one token for key, returning false when rate limited.
func (l *KeyedRateLimiter) Allow(key string) bool {
	l.mu.Lock()
	defer l.mu.Unlock()

	now := l.now()
	b, ok := l.buckets[key]
	if !ok {
		if len(l.buckets) >= l.maxKeys {
			cutoff := now.Add(-15 * time.Minute)
			for k, existing := range l.buckets {
				if existing.lastSeen.Before(cutoff) {
					delete(l.buckets, k)
				}
			}
			// If still at cap, evict one entry to stay bounded.
			if len(l.buckets) >= l.maxKeys {
				for k := range l.buckets {
					delete(l.buckets, k)
					break
				}
			}
		}
		l.buckets[key] = &tokenBucket{
			tokens:   l.burst - 1.0,
			lastSeen: now,
		}
		return true
	}

	elapsed := now.Sub(b.lastSeen).Seconds()
	if elapsed > 0 {
		b.tokens += elapsed * l.ratePerSec
		if b.tokens > l.burst {
			b.tokens = l.burst
		}
		b.lastSeen = now
	}
	if b.tokens < 1.0 {
		return false
	}
	b.tokens -= 1.0
	return true
}
