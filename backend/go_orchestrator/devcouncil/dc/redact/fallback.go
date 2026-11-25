package redact

import (
	"errors"
	"regexp"
)

type fallbackRule struct {
	name        string
	re          *regexp.Regexp
	replacement string
}

// fallbackRules holds high-confidence secret token patterns for pure-Go
// defense-in-depth egress scrubbing. Patterns are ordered longest/most-specific
// prefix first and use non-alphanumeric leading boundaries (not `\b`, because
// `_` is a word char in regex and must still separate tokens in filenames or
// identifiers). Replacements are inert markers that never re-trigger any rule
// or break JSON/Markdown/SARIF structure.
var fallbackRules = []fallbackRule{
	{
		name:        "pem_private_key_block",
		re:          regexp.MustCompile(`(?is)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----`),
		replacement: "[REDACTED:private-key]",
	},
	{
		name:        "pem_private_key_header",
		re:          regexp.MustCompile(`(?i)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----`),
		replacement: "[REDACTED:private-key]",
	},
	{
		name:        "aws_access_key_id",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])(?:AKIA|ASIA|ABIA|ACCA|A3T[A-Z0-9])[A-Z0-9]{16,}`),
		replacement: "${1}[REDACTED:aws-key]",
	},
	{
		name:        "github_pat",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])github_pat_[A-Za-z0-9_]{20,}`),
		replacement: "${1}[REDACTED:github-pat]",
	},
	{
		name:        "github_token",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])gh[pousr]_[A-Za-z0-9_]{20,}`),
		replacement: "${1}[REDACTED:github-token]",
	},
	{
		name:        "anthropic_api_key",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])sk-ant-[A-Za-z0-9_-]{16,}`),
		replacement: "${1}[REDACTED:anthropic-key]",
	},
	{
		name:        "openai_scoped_api_key",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])sk-(?:proj|admin|svcacct)-[A-Za-z0-9_-]{16,}`),
		replacement: "${1}[REDACTED:openai-key]",
	},
	{
		name:        "openrouter_api_key",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])sk-or-v1-[A-Za-z0-9_-]{24,}`),
		replacement: "${1}[REDACTED:openrouter-key]",
	},
	{
		name:        "openai_api_key",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])sk-[A-Za-z0-9]{20,}`),
		replacement: "${1}[REDACTED:sk-key]",
	},
	{
		name:        "stripe_key",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])(?:sk|rk)_(?:live|test|prod)_[A-Za-z0-9]{10,}`),
		replacement: "${1}[REDACTED:stripe-key]",
	},
	{
		name:        "stripe_webhook_secret",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])whsec_[A-Za-z0-9]{18,}`),
		replacement: "${1}[REDACTED:stripe-webhook]",
	},
	{
		name:        "google_api_key",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])AIza[A-Za-z0-9_-]{26,}`),
		replacement: "${1}[REDACTED:google-api-key]",
	},
	{
		name:        "google_oauth_client_secret",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])GOCSPX-[A-Za-z0-9_-]{20,}`),
		replacement: "${1}[REDACTED:google-oauth-secret]",
	},
	{
		name:        "gitlab_pat",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])glpat-[A-Za-z0-9_-]{20,}`),
		replacement: "${1}[REDACTED:gitlab-token]",
	},
	{
		name:        "slack_token",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])xox[bpoase]-[A-Za-z0-9-]{20,}`),
		replacement: "${1}[REDACTED:slack-token]",
	},
	{
		name:        "npm_token",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])npm_[A-Za-z0-9]{30,}`),
		replacement: "${1}[REDACTED:npm-token]",
	},
	{
		name:        "huggingface_token",
		re:          regexp.MustCompile(`(^|[^A-Za-z0-9])hf_[A-Za-z0-9]{24,}`),
		replacement: "${1}[REDACTED:hf-token]",
	},
}

const maxFallbackPasses = 8

// ContainsFallbackSecret reports whether text matches any high-confidence
// fallback secret pattern.
func ContainsFallbackSecret(text string) bool {
	for _, rule := range fallbackRules {
		if rule.re.MatchString(text) {
			return true
		}
	}
	return false
}

// ScrubFallback redacts high-confidence secret token patterns in pure Go and
// verifies convergence before returning. Fails closed if the output still
// matches any secret pattern after maxFallbackPasses.
func ScrubFallback(text string) (string, error) {
	if text == "" {
		return "", nil
	}
	cur := text
	for pass := 0; pass < maxFallbackPasses; pass++ {
		next := cur
		for _, rule := range fallbackRules {
			next = rule.re.ReplaceAllString(next, rule.replacement)
		}
		if next == cur {
			break
		}
		cur = next
	}
	if ContainsFallbackSecret(cur) {
		return "", errors.New("fallback redaction did not converge: secret pattern still present")
	}
	return cur, nil
}
