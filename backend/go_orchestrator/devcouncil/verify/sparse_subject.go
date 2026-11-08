package verify

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

// sparseCone describes a validated Git cone-mode (`core.sparseCheckoutCone=true`)
// working tree projection.
type sparseCone struct {
	fullTree          bool
	includedDirs      map[string]bool
	recursivePrefixes []string
}

func (c *sparseCone) contains(slashPath string) bool {
	if c == nil || c.fullTree {
		return true
	}
	p := strings.TrimPrefix(slashPath, "./")
	dir := path.Dir(p)
	if dir == "." || dir == "" {
		return true
	}
	if c.includedDirs[dir] {
		return true
	}
	for _, prefix := range c.recursivePrefixes {
		if strings.HasPrefix(p, prefix+"/") {
			return true
		}
	}
	return false
}

func loadSparseCone(ctx context.Context, git *proc.GitSession, root string) (*sparseCone, error) {
	sparse, err := runGit(ctx, git, 1<<10, "config", "--bool", "core.sparseCheckout")
	if err != nil || strings.TrimSpace(string(sparse)) != "true" {
		return nil, nil
	}
	coneCfg, err := runGit(ctx, git, 1<<10, "config", "--bool", "core.sparseCheckoutCone")
	if err != nil || strings.TrimSpace(string(coneCfg)) != "true" {
		return nil, errors.New(
			"the repository is a non-cone sparse checkout: only core.sparseCheckoutCone=true can be verified safely",
		)
	}
	out, err := runGit(ctx, git, 1<<12, "rev-parse", "--git-path", "info/sparse-checkout")
	if err != nil {
		return nil, fmt.Errorf("locating info/sparse-checkout: %w", err)
	}
	scPath := strings.TrimSpace(string(out))
	if !filepath.IsAbs(scPath) {
		scPath = filepath.Join(root, scPath)
	}
	body, err := readSmall(scPath, maxIgnoreSource)
	if err != nil {
		return nil, fmt.Errorf("reading info/sparse-checkout (%s): %w", scPath, err)
	}
	return parseSparseCone(body)
}

func parseSparseCone(body []byte) (*sparseCone, error) {
	sawRootAll := false
	sawRootExcludeSubdirs := false
	included := make(map[string]bool)
	nonRecursive := make(map[string]bool)

	for _, raw := range bytes.Split(body, []byte("\n")) {
		line := strings.TrimSpace(string(bytes.TrimRight(raw, "\r")))
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		switch {
		case line == "/*":
			sawRootAll = true
		case line == "!/*/":
			sawRootExcludeSubdirs = true
		case strings.HasPrefix(line, "!/") && strings.HasSuffix(line, "/*/"):
			dir := strings.TrimSuffix(strings.TrimPrefix(line, "!/"), "/*/")
			if !validConeDir(dir) {
				return nil, fmt.Errorf("non-cone pattern %q in info/sparse-checkout", line)
			}
			nonRecursive[dir] = true
		case strings.HasPrefix(line, "/") && strings.HasSuffix(line, "/"):
			dir := strings.TrimSuffix(strings.TrimPrefix(line, "/"), "/")
			if !validConeDir(dir) {
				return nil, fmt.Errorf("non-cone pattern %q in info/sparse-checkout", line)
			}
			included[dir] = true
		default:
			return nil, fmt.Errorf("non-cone pattern %q in info/sparse-checkout", line)
		}
	}

	if !sawRootAll {
		return nil, errors.New("info/sparse-checkout lacks root cone pattern '/*'")
	}
	if !sawRootExcludeSubdirs {
		return &sparseCone{fullTree: true}, nil
	}

	var recursive []string
	for dir := range included {
		if !nonRecursive[dir] {
			recursive = append(recursive, dir)
		}
	}
	return &sparseCone{
		includedDirs:      included,
		recursivePrefixes: recursive,
	}, nil
}

func validConeDir(dir string) bool {
	if dir == "" || strings.ContainsAny(dir, "*?[\\") {
		return false
	}
	for _, seg := range strings.Split(dir, "/") {
		if seg == "" || seg == "." || seg == ".." || seg == ".git" {
			return false
		}
	}
	return true
}

func markOutOfConeSkipWorktree(
	ctx context.Context,
	git *proc.GitSession,
	env []string,
	root string,
	cone *sparseCone,
) error {
	entries, err := listScratchIndex(ctx, git, env)
	if err != nil {
		return err
	}
	var skipBuf bytes.Buffer
	count := 0
	for _, e := range entries {
		if cone.contains(e.path) {
			continue
		}
		full := filepath.Join(root, filepath.FromSlash(e.path))
		if _, statErr := os.Lstat(full); errors.Is(statErr, os.ErrNotExist) {
			skipBuf.WriteString(e.path)
			skipBuf.WriteByte(0)
			count++
		}
	}
	if count == 0 {
		return nil
	}
	return runGitStdinEnv(
		ctx,
		git,
		1<<20,
		subjectBound,
		env,
		skipBuf.Bytes(),
		"update-index",
		"--skip-worktree",
		"-z",
		"--stdin",
	)
}

func runGitStdinEnv(
	ctx context.Context,
	git *proc.GitSession,
	limit int,
	bound time.Duration,
	env []string,
	stdin []byte,
	args ...string,
) error {
	ctx, cancel := context.WithTimeout(ctx, bound)
	defer cancel()
	cmd, err := git.Command(ctx, args...)
	if err != nil {
		return err
	}
	cmd.Env = append(cmd.Env, env...)
	cmd.Stdin = bytes.NewReader(stdin)
	stdout := &proc.CappedBuffer{Limit: limit}
	stderr := &proc.CappedBuffer{Limit: limit}
	cmd.Stdout = stdout
	cmd.Stderr = stderr
	cmd.WaitDelay = 2 * time.Second

	runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
	if timedOut {
		return fmt.Errorf("git %s did not return within the deadline: %w", strings.Join(args, " "), ctx.Err())
	}
	if runErr != nil {
		detail := strings.TrimSpace(string(stderr.Bytes()))
		if detail != "" {
			return fmt.Errorf("git %s: %w: %s", strings.Join(args, " "), runErr, detail)
		}
		return fmt.Errorf("git %s: %w", strings.Join(args, " "), runErr)
	}
	if stdout.Overflowed() || stderr.Overflowed() {
		return fmt.Errorf("git %s: %w", strings.Join(args, " "), ErrGitOutputTruncated)
	}
	return nil
}

// blobKey names what a streamed blob is digested as: object id and entry mode.
func blobKey(e indexEntry) string { return e.oid + "\x00" + e.mode }

func streamCatFileBlobs(
	ctx context.Context,
	git *proc.GitSession,
	entries []indexEntry,
	consume func(e indexEntry, r io.Reader, size int64) error,
) error {
	if len(entries) == 0 {
		return nil
	}
	ctx, cancel := context.WithTimeout(ctx, subjectBound)
	defer cancel()

	// Deduplicate so identical blobs across paths are streamed once. The key
	// includes the mode: a file and a symlink can share one blob, and the two
	// are digested differently (content hash versus link text).
	seen := make(map[string]bool, len(entries))
	var unique []indexEntry
	var reqBuf bytes.Buffer
	for _, e := range entries {
		if seen[blobKey(e)] {
			continue
		}
		seen[blobKey(e)] = true
		unique = append(unique, e)
		reqBuf.WriteString(e.oid)
		reqBuf.WriteByte('\n')
	}

	cmd, err := git.Command(ctx, "cat-file", "--batch")
	if err != nil {
		return err
	}
	cmd.Stdin = bytes.NewReader(reqBuf.Bytes())
	stderr := &proc.CappedBuffer{Limit: 1 << 20}
	cmd.Stderr = stderr
	cmd.WaitDelay = 2 * time.Second

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	startErr, timedOut := proc.RunBounded(ctx, cmd.Start)
	if timedOut {
		return fmt.Errorf("git cat-file --batch did not start within deadline: %w", ctx.Err())
	}
	if startErr != nil {
		return startErr
	}

	readErr := func() error {
		br := bufio.NewReaderSize(stdout, 64<<10)
		for _, e := range unique {
			header, err := br.ReadString('\n')
			if err != nil {
				return fmt.Errorf("reading cat-file --batch header for %s (%s): %w", e.path, e.oid, err)
			}
			fields := strings.Fields(strings.TrimSpace(header))
			if len(fields) != 3 || fields[1] != "blob" {
				return fmt.Errorf("unexpected cat-file --batch header for %s: %q", e.path, strings.TrimSpace(header))
			}
			size, err := strconv.ParseInt(fields[2], 10, 64)
			if err != nil || size < 0 {
				return fmt.Errorf("invalid blob size in cat-file --batch header %q", strings.TrimSpace(header))
			}
			if err := consume(e, io.LimitReader(br, size), size); err != nil {
				return fmt.Errorf("reading blob %s (%s): %w", e.path, e.oid, err)
			}
			// Consume trailing newline emitted by `git cat-file --batch`.
			b, err := br.ReadByte()
			if err != nil {
				return fmt.Errorf("reading trailing newline for blob %s: %w", e.path, err)
			}
			if b != '\n' {
				return fmt.Errorf("malformed cat-file --batch delimiter %q after %s", b, e.path)
			}
		}
		return nil
	}()

	waitErr, waitTimedOut := proc.RunBounded(ctx, cmd.Wait)
	if readErr != nil {
		return readErr
	}
	if waitTimedOut {
		return fmt.Errorf("git cat-file --batch did not finish within deadline: %w", ctx.Err())
	}
	if waitErr != nil {
		return fmt.Errorf("git cat-file --batch: %w (%s)", waitErr, strings.TrimSpace(string(stderr.Bytes())))
	}
	return nil
}
