// Package gitrev holds shared git revision validation, merge-base resolution,
// repository path validation, and GitHub Actions workflow command escaping.
//
// Both `dctestselect` and `devcouncil pr-gate` share this single implementation
// so revision parsing and workflow annotation escaping cannot drift.
package gitrev

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

var (
	zeroSHARe   = regexp.MustCompile(`^0+$`)
	refTokenRe  = regexp.MustCompile(`^[A-Za-z0-9._/@~^-]+$`)
	shaSyntaxRe = regexp.MustCompile(`^[0-9a-fA-F]{7,64}$`)
	fullSHARe   = regexp.MustCompile(`^[0-9a-f]{40,64}$`)
	sha40Re     = regexp.MustCompile(`^[0-9a-f]{40}$`)
)

// Runner answers one command invocation with bounded stdout.
// It matches cmd/dctestselect's Runner interface so test doubles and guarded
// proc.GitSession wrappers share one revision-resolution path.
type Runner interface {
	Output(ctx context.Context, dir string, argv ...string) ([]byte, error)
}

// SessionRunner adapts a guarded *proc.GitSession to Runner for git commands.
// Every read carries proc.GitArgs guards (fsmonitor, hooksPath, replace refs,
// ext-diff, textconv, color, and filter drivers).
type SessionRunner struct {
	Session *proc.GitSession
	Limit   int
}

const defaultGitRevLimit = 64 << 10

// Output executes a git command through the guarded GitSession.
func (r SessionRunner) Output(ctx context.Context, _ string, argv ...string) ([]byte, error) {
	if r.Session == nil {
		return nil, errors.New("git session is nil")
	}
	if len(argv) == 0 {
		return nil, errors.New("empty git command")
	}
	args := argv
	if args[0] == "git" {
		args = args[1:]
	}
	limit := r.Limit
	if limit <= 0 {
		limit = defaultGitRevLimit
	}
	res, err := r.Session.Run(ctx, limit, args...)
	if err != nil {
		return nil, err
	}
	if res.Truncated {
		return nil, fmt.Errorf("git %s output exceeded %d bytes", strings.Join(args, " "), limit)
	}
	return res.Stdout, nil
}

// IsRefToken reports whether rev is a non-empty revision token that cannot be
// parsed by git as a command-line option or carry shell/control characters.
func IsRefToken(rev string) bool {
	return rev != "" && !strings.HasPrefix(rev, "-") && refTokenRe.MatchString(rev)
}

// IsFullSHA reports whether s is a full lowercase hexadecimal commit ID
// (40 chars for SHA-1, up to 64 chars for SHA-256).
func IsFullSHA(s string) bool {
	return fullSHARe.MatchString(s)
}

// IsSHA40 reports whether s is a strict 40-character lowercase hexadecimal
// Git commit SHA (`^[0-9a-f]{40}$`), as emitted by GitHub Actions pull_request
// events.
func IsSHA40(s string) bool {
	return sha40Re.MatchString(s)
}

// IsSHASyntax reports whether s looks like a hexadecimal commit ID prefix/full SHA.
func IsSHASyntax(s string) bool {
	return shaSyntaxRe.MatchString(s)
}

// IsZeroSHA reports whether s is an all-zero commit ID (e.g. a newly created ref).
func IsZeroSHA(s string) bool {
	return zeroSHARe.MatchString(s)
}

// ResolveRev turns a revision into a full commit ID, refusing anything git
// could interpret as an option.
func ResolveRev(ctx context.Context, run Runner, repo, rev string) (string, error) {
	if !IsRefToken(rev) {
		return "", fmt.Errorf("%q is not a usable revision", rev)
	}
	out, err := run.Output(ctx, repo, "git", "rev-parse", "--verify", "--quiet", rev+"^{commit}")
	if err != nil {
		return "", err
	}
	sha := strings.TrimSpace(string(out))
	if !IsFullSHA(sha) {
		return "", fmt.Errorf("git rev-parse %s answered %q", rev, sha)
	}
	return sha, nil
}

// MergeBase computes the merge base between baseRev and headRev, refusing
// option-like arguments and validating that git returns a full commit ID.
func MergeBase(ctx context.Context, run Runner, repo, baseRev, headRev string) (string, error) {
	if !IsRefToken(baseRev) {
		return "", fmt.Errorf("%q is not a usable base revision", baseRev)
	}
	if !IsRefToken(headRev) {
		return "", fmt.Errorf("%q is not a usable head revision", headRev)
	}
	out, err := run.Output(ctx, repo, "git", "merge-base", baseRev, headRev)
	if err != nil {
		return "", err
	}
	sha := strings.TrimSpace(string(out))
	if !IsFullSHA(sha) {
		return "", fmt.Errorf("git merge-base answered %q", sha)
	}
	return sha, nil
}

// ResolveRevSession resolves rev to a full commit SHA using a guarded proc.GitSession.
func ResolveRevSession(ctx context.Context, git *proc.GitSession, rev string) (string, error) {
	if git == nil {
		return "", errors.New("git session is nil")
	}
	return ResolveRev(ctx, SessionRunner{Session: git}, git.Root(), rev)
}

// MergeBaseSession computes merge-base(baseRev, headRev) using a guarded proc.GitSession.
func MergeBaseSession(ctx context.Context, git *proc.GitSession, baseRev, headRev string) (string, error) {
	if git == nil {
		return "", errors.New("git session is nil")
	}
	return MergeBase(ctx, SessionRunner{Session: git}, git.Root(), baseRev, headRev)
}

// CheckRepoPath accepts a normalised slash-separated path contained inside a
// repository, rejecting empty paths, NUL bytes, backslashes, absolute paths,
// drive prefixes, and `.` / `..` segments.
func CheckRepoPath(p string) error {
	switch {
	case p == "":
		return errors.New("empty path")
	case strings.ContainsRune(p, 0) || strings.Contains(p, "\\"):
		return fmt.Errorf("path %q is not slash-separated text", p)
	case strings.HasPrefix(p, "/") || (len(p) > 1 && p[1] == ':'):
		return fmt.Errorf("path %q is absolute", p)
	}
	for _, part := range strings.Split(p, "/") {
		if part == ".." || part == "" || part == "." {
			return fmt.Errorf("path %q is not a normalised repository path", p)
		}
	}
	return nil
}
