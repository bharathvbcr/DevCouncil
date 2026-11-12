package verify

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/dcverify"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// maxOracleFileBytes bounds any single held-out oracle file hashed into a snapshot.
const maxOracleFileBytes = 32 << 20

// OracleFileDigest binds one repo-relative file in the held-out verification
// closure to its exact pre-turn SHA-256 digest and byte length.
type OracleFileDigest struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
	Size   int64  `json:"size"`
}

// OracleSnapshot is the out-of-sandbox cryptographic commitment to a task's
// held-out specification and test closure, captured before agent execution and
// verified outside the agent sandbox.
//
// Strength is always StrengthProven: a mismatch proves byte-level mutation or
// deletion of a protected oracle file without relying on git status, agent
// claims, or in-sandbox runners.
type OracleSnapshot struct {
	Strength string             `json:"strength"`
	Files    []OracleFileDigest `json:"files"`
}

// CaptureOracleSnapshot hashes each repo-relative path in oraclePaths through
// os.OpenRoot(root), refusing path traversal, symlinks, non-regular files, and
// oversized blobs. Duplicate paths are deduplicated deterministically.
func CaptureOracleSnapshot(root string, oraclePaths []string) (OracleSnapshot, error) {
	if root == "" {
		return OracleSnapshot{}, errors.New("oracle snapshot requires a non-empty repository root")
	}
	rootFS, err := os.OpenRoot(root)
	if err != nil {
		return OracleSnapshot{}, fmt.Errorf("open repository root: %w", err)
	}
	defer rootFS.Close()

	seen := make(map[string]struct{}, len(oraclePaths))
	sorted := make([]string, 0, len(oraclePaths))
	for _, p := range oraclePaths {
		norm := strings.ReplaceAll(strings.TrimSpace(p), "\\", "/")
		norm = strings.TrimPrefix(norm, "./")
		if norm == "" {
			continue
		}
		if err := safefile.ValidRepoPath(norm); err != nil {
			return OracleSnapshot{}, fmt.Errorf("invalid oracle path %q: %w", p, err)
		}
		if _, ok := seen[norm]; !ok {
			seen[norm] = struct{}{}
			sorted = append(sorted, norm)
		}
	}
	sort.Strings(sorted)

	digests := make([]OracleFileDigest, 0, len(sorted))
	for _, rel := range sorted {
		digest, size, err := hashRootRegularFile(rootFS, rel)
		if err != nil {
			return OracleSnapshot{}, err
		}
		digests = append(digests, OracleFileDigest{
			Path:   rel,
			SHA256: digest,
			Size:   size,
		})
	}
	return OracleSnapshot{
		Strength: dcverify.StrengthProven,
		Files:    digests,
	}, nil
}

// VerifyOracleSnapshot compares the post-turn filesystem against snap outside
// the agent sandbox and emits hard-safety blocking `oracle_tampering` gaps on
// any byte modification, deletion, symlink swap, or unreadable oracle file.
func VerifyOracleSnapshot(taskID, root string, snap OracleSnapshot) []Gap {
	if len(snap.Files) == 0 {
		return nil
	}
	rootFS, err := os.OpenRoot(root)
	if err != nil {
		return []Gap{{
			ID:             StableGapID(taskID, "ORACLE-ROOT"),
			Severity:       "critical",
			GapType:        "oracle_tampering",
			TaskID:         taskID,
			Description:    "Held-out oracle snapshot could not open repository root: " + err.Error(),
			Evidence:       []string{err.Error()},
			RecommendedFix: "Restore readable repository root and unmodified oracle files, then re-verify.",
			Blocking:       true,
		}}
	}
	defer rootFS.Close()

	var gaps []Gap
	for _, want := range snap.Files {
		gotHash, gotSize, err := hashRootRegularFile(rootFS, want.Path)
		if err != nil {
			p := want.Path
			gaps = append(gaps, Gap{
				ID:             StableGapID(taskID, "ORACLE-"+want.Path),
				Severity:       "critical",
				GapType:        "oracle_tampering",
				TaskID:         taskID,
				Description:    "Held-out oracle file " + want.Path + " was removed, replaced by a non-regular file/symlink, or became unreadable: " + err.Error(),
				Evidence:       []string{want.Path + ": expected sha256=" + want.SHA256, err.Error()},
				RecommendedFix: "Revert unauthorized changes to " + want.Path + " so it matches the held-out oracle digest.",
				Blocking:       true,
				File:           &p,
			})
			continue
		}
		if gotHash != want.SHA256 || gotSize != want.Size {
			p := want.Path
			gaps = append(gaps, Gap{
				ID:          StableGapID(taskID, "ORACLE-"+want.Path),
				Severity:    "critical",
				GapType:     "oracle_tampering",
				TaskID:      taskID,
				Description: "Held-out oracle file " + want.Path + " was modified during task execution.",
				Evidence: []string{
					fmt.Sprintf("%s: expected sha256=%s (%d bytes), got sha256=%s (%d bytes)", want.Path, want.SHA256, want.Size, gotHash, gotSize),
				},
				RecommendedFix: "Revert unauthorized edits to held-out oracle file " + want.Path + ", then re-verify.",
				Blocking:       true,
				File:           &p,
			})
		}
	}
	return gaps
}

func hashRootRegularFile(rootFS *os.Root, rel string) (string, int64, error) {
	info, err := rootFS.Lstat(rel)
	if err != nil {
		return "", 0, fmt.Errorf("%s: %w", rel, err)
	}
	if info.Mode()&fs.ModeSymlink != 0 {
		return "", 0, fmt.Errorf("%s: refusing symlink in oracle snapshot", rel)
	}
	if !info.Mode().IsRegular() {
		return "", 0, fmt.Errorf("%s: not a regular file", rel)
	}
	if info.Size() > maxOracleFileBytes {
		return "", 0, fmt.Errorf("%s: exceeds %d-byte oracle cap", rel, maxOracleFileBytes)
	}
	f, err := rootFS.Open(rel)
	if err != nil {
		return "", 0, fmt.Errorf("%s: %w", rel, err)
	}
	defer f.Close()

	st, err := f.Stat()
	if err != nil {
		return "", 0, fmt.Errorf("%s: %w", rel, err)
	}
	if !st.Mode().IsRegular() {
		return "", 0, fmt.Errorf("%s: opened descriptor is not a regular file", rel)
	}
	h := sha256.New()
	n, err := io.Copy(h, io.LimitReader(f, maxOracleFileBytes+1))
	if err != nil {
		return "", 0, fmt.Errorf("%s: %w", rel, err)
	}
	if n > maxOracleFileBytes {
		return "", 0, fmt.Errorf("%s: exceeds %d-byte oracle cap", rel, maxOracleFileBytes)
	}
	return hex.EncodeToString(h.Sum(nil)), n, nil
}

// ComputeGoTestClosure resolves the repo-relative Go test and helper files in
// the dependency closure of pkgPatterns via `go list -deps -test -json`,
// excluding plannedEditablePaths so an OracleSnapshot can lock test helpers
// alongside `*_test.go` files while leaving planned implementation files writable.
func ComputeGoTestClosure(ctx context.Context, root string, pkgPatterns []string, plannedEditablePaths []string) ([]string, error) {
	if len(pkgPatterns) == 0 {
		return nil, errors.New("at least one Go package pattern is required")
	}
	absRoot, err := filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	if realRoot, err := filepath.EvalSymlinks(absRoot); err == nil {
		absRoot = realRoot
	}
	cctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()

	args := append([]string{"list", "-deps", "-test", "-json"}, pkgPatterns...)
	cmd := exec.CommandContext(cctx, "go", args...)
	proc.ConfigureGroup(cmd)
	cmd.Dir = absRoot
	cmd.Env = append(os.Environ(), "GOWORK=off")
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	cmd.WaitDelay = commandWaitDelay
	runErr, abandoned := proc.RunBounded(cctx, cmd.Run)
	if abandoned {
		return nil, errors.New("go list -deps -test timed out")
	}
	if runErr != nil {
		return nil, fmt.Errorf("go list -deps -test: %w (%s)", runErr, strings.TrimSpace(stderr.String()))
	}

	editable := make(map[string]struct{}, len(plannedEditablePaths))
	for _, p := range plannedEditablePaths {
		norm := strings.TrimPrefix(strings.ReplaceAll(p, "\\", "/"), "./")
		editable[norm] = struct{}{}
	}

	type goListPkg struct {
		Dir          string   `json:"Dir"`
		Standard     bool     `json:"Standard"`
		GoFiles      []string `json:"GoFiles"`
		CgoFiles     []string `json:"CgoFiles"`
		TestGoFiles  []string `json:"TestGoFiles"`
		XTestGoFiles []string `json:"XTestGoFiles"`
	}
	seen := map[string]struct{}{}
	dec := json.NewDecoder(&stdout)
	for dec.More() {
		var pkg goListPkg
		if err := dec.Decode(&pkg); err != nil {
			return nil, fmt.Errorf("decode go list json: %w", err)
		}
		if pkg.Standard || pkg.Dir == "" {
			continue
		}
		pkgDir := pkg.Dir
		if realDir, err := filepath.EvalSymlinks(pkgDir); err == nil {
			pkgDir = realDir
		}
		relDir, err := filepath.Rel(absRoot, pkgDir)
		if err != nil || strings.HasPrefix(relDir, "..") {
			continue
		}
		var names []string
		names = append(names, pkg.GoFiles...)
		names = append(names, pkg.CgoFiles...)
		names = append(names, pkg.TestGoFiles...)
		names = append(names, pkg.XTestGoFiles...)
		for _, name := range names {
			var relPath string
			if filepath.IsAbs(name) {
				realName := name
				if eval, err := filepath.EvalSymlinks(name); err == nil {
					realName = eval
				}
				relAbs, err := filepath.Rel(absRoot, realName)
				if err != nil || relAbs == "." || strings.HasPrefix(relAbs, "..") {
					// Synthetic `go list -test` generated `_testmain.go` in GOCACHE outside repo root.
					continue
				}
				relPath = relAbs
			} else {
				relPath = filepath.Join(relDir, name)
			}
			rel := filepath.ToSlash(relPath)
			if _, isEditable := editable[rel]; isEditable {
				continue
			}
			if safefile.ValidRepoPath(rel) == nil {
				seen[rel] = struct{}{}
			}
		}
	}
	out := make([]string, 0, len(seen))
	for rel := range seen {
		out = append(out, rel)
	}
	sort.Strings(out)
	return out, nil
}

// RunSandboxedCommandWithEnv binds RunSandboxedVerificationCommand to CommandTimeout
// for use as Input.RunCommand when SandboxSeatbeltRO or SandboxBwrapRO is selected.
func RunSandboxedCommandWithEnv(ctx context.Context, root string, envPolicy EnvPolicy) func(string) CommandOutcome {
	return func(command string) CommandOutcome {
		return RunSandboxedVerificationCommand(ctx, root, command, CommandTimeout, envPolicy)
	}
}

func resolveCanonicalSandboxPaths(rawPath string) (declaredAbs, realAbs string, err error) {
	if strings.TrimSpace(rawPath) == "" {
		return "", "", errors.New("sandbox path must not be empty")
	}
	for i := 0; i < len(rawPath); i++ {
		if rawPath[i] < 0x20 || rawPath[i] == 0x7f {
			return "", "", fmt.Errorf("sandbox path %q contains unsafe control byte 0x%02x", rawPath, rawPath[i])
		}
	}
	abs, err := filepath.Abs(rawPath)
	if err != nil {
		return "", "", err
	}
	declaredAbs = filepath.Clean(abs)
	realAbs = declaredAbs

	// Resolve symlinks on declaredAbs or its deepest existing ancestor so
	// firmlinks/symlinks (e.g. /var/... -> /private/var/... or /tmp -> /private/tmp)
	// canonicalize consistently even before leaf entries exist.
	cur := declaredAbs
	var tail []string
	for {
		if realCur, evalErr := filepath.EvalSymlinks(cur); evalErr == nil {
			for i := len(tail) - 1; i >= 0; i-- {
				realCur = filepath.Join(realCur, tail[i])
			}
			realAbs = filepath.Clean(realCur)
			break
		}
		parent := filepath.Dir(cur)
		if parent == cur {
			break
		}
		tail = append(tail, filepath.Base(cur))
		cur = parent
	}
	for _, p := range []string{declaredAbs, realAbs} {
		for i := 0; i < len(p); i++ {
			if p[i] < 0x20 || p[i] == 0x7f {
				return "", "", fmt.Errorf("canonical sandbox path %q contains unsafe control byte 0x%02x", p, p[i])
			}
		}
	}
	return declaredAbs, realAbs, nil
}

func resolveSeatbeltDenyPaths(rawRoot string) ([]string, error) {
	declaredAbs, realAbs, err := resolveCanonicalSandboxPaths(rawRoot)
	if err != nil {
		return nil, err
	}
	paths := []string{declaredAbs}
	if realAbs != declaredAbs {
		paths = append(paths, realAbs)
	}
	for _, p := range paths {
		if strings.ContainsAny(p, "\"\\\n\r") {
			return nil, errors.New("repository path contains characters unsafe for Seatbelt subpath literal")
		}
	}
	return paths, nil
}

func pathWithinOrEqual(candidate, parent string) bool {
	c := filepath.Clean(candidate)
	p := filepath.Clean(parent)
	if c == p {
		return true
	}
	if p == string(filepath.Separator) {
		return strings.HasPrefix(c, string(filepath.Separator))
	}
	return strings.HasPrefix(c, p+string(filepath.Separator))
}

// BuildLinuxBwrapReadOnlyArgs constructs the deterministic Linux bubblewrap (`bwrap`)
// argv for executing command in a read-only mount namespace over rawRoot with an
// isolated writable scratchTmp directory.
//
// Mount ordering invariant:
//  1. `--ro-bind / /` mounts the entire host filesystem read-only.
//  2. `--dev /dev` and `--proc /proc` provide minimal virtual filesystems.
//  3. `--bind <realTmp> <realTmp>` mounts ONLY the isolated scratch TMPDIR writable.
//  4. `--ro-bind <realRoot> <realRoot>` is re-asserted AFTER `--bind <realTmp> <realTmp>`
//     as defense-in-depth so the repository root remains strictly read-only.
//  5. Overlapping repository and scratchTmp paths (or `/` as either) fail closed.
func BuildLinuxBwrapReadOnlyArgs(rawRoot, scratchTmp, command string) (execDir, realTmp string, args []string, err error) {
	declaredRoot, realRoot, err := resolveCanonicalSandboxPaths(rawRoot)
	if err != nil {
		return "", "", nil, fmt.Errorf("invalid repository root: %w", err)
	}
	declaredTmp, canonTmp, err := resolveCanonicalSandboxPaths(scratchTmp)
	if err != nil {
		return "", "", nil, fmt.Errorf("invalid scratch tmpdir: %w", err)
	}
	sep := string(filepath.Separator)
	for _, p := range []string{declaredRoot, realRoot, declaredTmp, canonTmp} {
		if p == sep {
			return "", "", nil, errors.New("refusing filesystem root \"/\" as repository or scratch path")
		}
	}
	for _, r := range []string{declaredRoot, realRoot} {
		for _, t := range []string{declaredTmp, canonTmp} {
			if pathWithinOrEqual(t, r) || pathWithinOrEqual(r, t) {
				return "", "", nil, fmt.Errorf("scratch tmpdir %q overlaps repository root %q", scratchTmp, rawRoot)
			}
		}
	}

	args = []string{
		"--ro-bind", "/", "/",
		"--dev", "/dev",
		"--proc", "/proc",
		"--bind", canonTmp, canonTmp,
		"--ro-bind", realRoot, realRoot,
		"--chdir", realRoot,
		"--die-with-parent",
		"--",
		"/bin/sh", "-c", command,
	}
	return realRoot, canonTmp, args, nil
}

// ProbeLinuxBwrapWithBin executes a minimal read-only namespace probe using bwrapBin
// to confirm both that the binary is executable and that user/mount namespaces are
// permitted on the host (e.g. not blocked by AppArmor/seccomp/sysctl).
func ProbeLinuxBwrapWithBin(ctx context.Context, bwrapBin string) error {
	if strings.TrimSpace(bwrapBin) == "" {
		return errors.New("bwrap binary path is empty")
	}
	info, err := os.Stat(bwrapBin)
	if err != nil {
		return fmt.Errorf("stat bwrap %q: %w", bwrapBin, err)
	}
	if info.IsDir() {
		return fmt.Errorf("bwrap path %q is a directory", bwrapBin)
	}
	pctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(
		pctx,
		bwrapBin,
		"--ro-bind", "/", "/",
		"--dev", "/dev",
		"--proc", "/proc",
		"--die-with-parent",
		"--",
		"/bin/sh", "-c", ":",
	)
	proc.ConfigureGroup(cmd)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	cmd.WaitDelay = commandWaitDelay
	runErr, abandoned := proc.RunBounded(pctx, cmd.Run)
	if abandoned {
		return errors.New("bwrap probe timed out")
	}
	if runErr != nil {
		detail := strings.TrimSpace(stderr.String())
		if detail != "" {
			return fmt.Errorf("bwrap probe failed: %w (%s)", runErr, detail)
		}
		return fmt.Errorf("bwrap probe failed: %w", runErr)
	}
	return nil
}

func createSafeSandboxTempDir(disallowRoots []string, pattern string) (string, error) {
	var canonicalDisallowed []string
	for _, raw := range disallowRoots {
		if strings.TrimSpace(raw) == "" {
			continue
		}
		if decl, real, err := resolveCanonicalSandboxPaths(raw); err == nil {
			canonicalDisallowed = append(canonicalDisallowed, decl)
			if real != decl {
				canonicalDisallowed = append(canonicalDisallowed, real)
			}
		}
	}
	baseInsideDisallowed := func(baseDir string) bool {
		decl, real, err := resolveCanonicalSandboxPaths(baseDir)
		if err != nil {
			return true
		}
		for _, c := range []string{decl, real} {
			for _, d := range canonicalDisallowed {
				if pathWithinOrEqual(c, d) {
					return true
				}
			}
		}
		return false
	}
	createdOverlapsDisallowed := func(createdDir string) bool {
		decl, real, err := resolveCanonicalSandboxPaths(createdDir)
		if err != nil {
			return true
		}
		for _, c := range []string{decl, real} {
			for _, d := range canonicalDisallowed {
				if pathWithinOrEqual(c, d) || pathWithinOrEqual(d, c) {
					return true
				}
			}
		}
		return false
	}

	for _, baseDir := range []string{os.TempDir(), "/tmp", "/var/tmp"} {
		if baseInsideDisallowed(baseDir) {
			continue
		}
		dir, err := os.MkdirTemp(baseDir, pattern)
		if err != nil {
			continue
		}
		if createdOverlapsDisallowed(dir) {
			_ = os.RemoveAll(dir)
			continue
		}
		return dir, nil
	}
	return "", errors.New("cannot locate host temporary directory outside repository root")
}

// RunLinuxBwrapVerificationCommandWithBin runs command inside Linux bubblewrap (`bwrap`)
// using bwrapBin (or resolving "bwrap" on PATH when bwrapBin is empty). If bwrap is
// missing or blocked from creating a read-only mount namespace, it fails closed
// (`Skipped: true, ExitCode: -1`) rather than executing unsandboxed on the host.
func RunLinuxBwrapVerificationCommandWithBin(ctx context.Context, bwrapBin, root, command string, timeout time.Duration, envPolicy EnvPolicy) CommandOutcome {
	resolvedBin := strings.TrimSpace(bwrapBin)
	if resolvedBin == "" {
		found, err := exec.LookPath("bwrap")
		if err != nil {
			return CommandOutcome{
				ExitCode:           -1,
				Skipped:            true,
				SandboxUnavailable: true,
				Reason:             "bwrap binary not available: " + err.Error(),
			}
		}
		resolvedBin = found
	}
	if err := ProbeLinuxBwrapWithBin(ctx, resolvedBin); err != nil {
		return CommandOutcome{
			ExitCode:           -1,
			Skipped:            true,
			SandboxUnavailable: true,
			Reason:             "linux bwrap sandbox unavailable or blocked: " + err.Error(),
		}
	}

	if timeout <= 0 {
		timeout = CommandTimeout
	}
	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	scratchTmp, err := createSafeSandboxTempDir([]string{root}, "dc-verify-sbx-*")
	if err != nil {
		return CommandOutcome{ExitCode: -1, Summary: "Failed to create sandbox tmpdir: " + err.Error(), Stderr: err.Error()}
	}
	defer os.RemoveAll(scratchTmp)

	execDir, realTmp, bwrapArgs, err := BuildLinuxBwrapReadOnlyArgs(root, scratchTmp, command)
	if err != nil {
		return CommandOutcome{
			ExitCode: -1,
			Summary:  "Failed to configure bwrap sandbox: " + err.Error(),
			Stderr:   err.Error(),
		}
	}
	return execSandboxedProcess(ctx, cctx, timeout, execDir, realTmp, true, denySliceForRoot(root), envPolicy, resolvedBin, bwrapArgs...)
}

func denySliceForRoot(root string) []string {
	decl, real, err := resolveCanonicalSandboxPaths(root)
	if err != nil {
		return nil
	}
	if real != decl {
		return []string{decl, real}
	}
	return []string{decl}
}

// RunSandboxedVerificationCommand executes a verification command inside an
// OS-enforced read-only repository sandbox (`sandbox-exec` on macOS, `bwrap` on
// Linux) so a test or build hook cannot overwrite repository files, `.git`,
// `.devcouncil`, or held-out oracles while tests run. When no OS read-only
// sandbox is operational, it fails closed with Skipped=true rather than silently
// running unsandboxed.
func RunSandboxedVerificationCommand(ctx context.Context, root, command string, timeout time.Duration, envPolicy EnvPolicy) CommandOutcome {
	switch runtime.GOOS {
	case "darwin":
		const sandboxBin = "/usr/bin/sandbox-exec"
		if _, err := os.Stat(sandboxBin); err != nil {
			return CommandOutcome{
				ExitCode:           -1,
				Skipped:            true,
				SandboxUnavailable: true,
				Reason:             "sandbox-exec binary not available: " + err.Error(),
			}
		}
		denyPaths, err := resolveSeatbeltDenyPaths(root)
		if err != nil {
			return CommandOutcome{
				ExitCode: -1,
				Summary:  "Failed to run command: " + err.Error(),
				Stderr:   err.Error(),
			}
		}
		execDir := denyPaths[len(denyPaths)-1]
		// Seatbelt profile: allow read/process execution and writes to scratch/cache
		// directories (`TMPDIR`, `/tmp`, `/private/tmp`, `/private/var/folders`),
		// while denying every file-write* inside both declared and canonical root paths.
		var sb strings.Builder
		sb.WriteString("(version 1)\n(allow default)\n")
		for _, dp := range denyPaths {
			fmt.Fprintf(&sb, "(deny file-write* (subpath %q))\n", dp)
		}
		profile := sb.String()

		if timeout <= 0 {
			timeout = CommandTimeout
		}
		cctx, cancel := context.WithTimeout(ctx, timeout)
		defer cancel()

		scratchTmp, err := createSafeSandboxTempDir(denyPaths, "dc-verify-sbx-*")
		if err != nil {
			return CommandOutcome{ExitCode: -1, Summary: "Failed to create sandbox tmpdir: " + err.Error(), Stderr: err.Error()}
		}
		defer os.RemoveAll(scratchTmp)

		return execSandboxedProcess(ctx, cctx, timeout, execDir, scratchTmp, false, denyPaths, envPolicy, sandboxBin, "-p", profile, "/bin/sh", "-c", command)
	case "linux":
		return RunLinuxBwrapVerificationCommandWithBin(ctx, "", root, command, timeout, envPolicy)
	default:
		return CommandOutcome{
			ExitCode:           -1,
			Skipped:            true,
			SandboxUnavailable: true,
			Reason:             "read-only OS verification sandbox requires darwin sandbox-exec or linux bwrap (got " + runtime.GOOS + ")",
		}
	}
}

func execSandboxedProcess(
	ctx, cctx context.Context,
	timeout time.Duration,
	execDir, scratchTmp string,
	isolateAllCaches bool,
	denyPaths []string,
	envPolicy EnvPolicy,
	bin string,
	args ...string,
) CommandOutcome {
	cmd := exec.CommandContext(cctx, bin, args...)
	proc.ConfigureGroup(cmd)
	cmd.Dir = execDir
	xdgCache := filepath.Join(scratchTmp, "xdg-cache")
	goCache := filepath.Join(scratchTmp, "go-cache")
	if isolateAllCaches {
		_ = os.MkdirAll(xdgCache, 0o700)
		_ = os.MkdirAll(goCache, 0o700)
	}

	pathBlocked := func(val string) bool {
		if val == "" {
			return false
		}
		decl, real, err := resolveCanonicalSandboxPaths(val)
		if err != nil {
			return false
		}
		for _, c := range []string{decl, real} {
			for _, dp := range denyPaths {
				if pathWithinOrEqual(c, dp) {
					return true
				}
			}
		}
		return false
	}

	baseEnv := envPolicy.Environ(os.Environ())
	filteredEnv := make([]string, 0, len(baseEnv)+4)
	replacedGoCache := false
	for _, kv := range baseEnv {
		if strings.HasPrefix(kv, "TMPDIR=") {
			continue
		}
		if isolateAllCaches && strings.HasPrefix(kv, "XDG_CACHE_HOME=") {
			continue
		}
		if strings.HasPrefix(kv, "GOCACHE=") {
			val := strings.TrimPrefix(kv, "GOCACHE=")
			if isolateAllCaches && !pathWithinOrEqual(val, scratchTmp) {
				replacedGoCache = true
				continue
			}
			if !isolateAllCaches && pathBlocked(val) {
				_ = os.MkdirAll(goCache, 0o700)
				replacedGoCache = true
				continue
			}
		}
		filteredEnv = append(filteredEnv, kv)
	}
	filteredEnv = append(filteredEnv, "TMPDIR="+scratchTmp)
	if isolateAllCaches {
		filteredEnv = append(filteredEnv, "XDG_CACHE_HOME="+xdgCache)
		if replacedGoCache || os.Getenv("GOCACHE") == "" {
			filteredEnv = append(filteredEnv, "GOCACHE="+goCache)
		}
	} else if replacedGoCache {
		filteredEnv = append(filteredEnv, "GOCACHE="+goCache)
	}
	cmd.Env = filteredEnv

	out := &proc.CappedBuffer{Limit: commandOutputLimit}
	cmd.Stdout = out
	cmd.Stderr = out
	cmd.WaitDelay = commandWaitDelay

	runErr, abandoned := proc.RunBounded(cctx, cmd.Run)
	if abandoned || cctx.Err() != nil {
		reason := stopReason(ctx, cctx, timeout)
		return CommandOutcome{
			ExitCode: -1,
			TimedOut: true,
			Reason:   reason,
			Summary:  "stopped: " + reason,
		}
	}
	text := out.String()
	summary := truncate(text, 2000)
	if runErr != nil {
		var ee *exec.ExitError
		if errors.As(runErr, &ee) {
			return CommandOutcome{
				ExitCode:       ee.ExitCode(),
				Summary:        summary,
				Stdout:         text,
				Stderr:         text,
				OutputCaptured: true,
			}
		}
		return CommandOutcome{
			ExitCode: -1,
			Summary:  "Failed to run command: " + runErr.Error(),
			Stdout:   text,
			Stderr:   runErr.Error(),
		}
	}
	return CommandOutcome{
		ExitCode:       0,
		Summary:        summary,
		Stdout:         text,
		OutputCaptured: true,
	}
}

// DevMapConeEvidence summarizes a DevMap symbol-impact cone check over changed
// symbols. Because static call graphs do not prove the absence of reflection,
// dynamic dispatch, or build-script edges, DevMap cone checks are explicitly
// classified at StrengthDerived (never StrengthProven), and incomplete walks or
// partial parses fail closed into DegradedReasons rather than passing as clean.
type DevMapConeEvidence struct {
	WalkIncomplete    bool
	PartialParseFiles []string
	Truncated         bool
	OutOfConeTouches  []string
}

// DevMapConeVerdict is the result of evaluating a DevMapConeEvidence envelope.
type DevMapConeVerdict struct {
	Strength        string
	Gaps            []Gap
	DegradedReasons []string
}

// VerifyDevMapCone evaluates DevMap symbol-cone evidence with fail-closed
// handling of incomplete walks, partial parses, and truncated traversals.
func VerifyDevMapCone(taskID string, ev DevMapConeEvidence) DevMapConeVerdict {
	verdict := DevMapConeVerdict{Strength: dcverify.StrengthDerived}
	if ev.WalkIncomplete {
		verdict.DegradedReasons = append(verdict.DegradedReasons,
			"devmap symbol-cone traversal reported walk_incomplete=true; incomplete graph cannot prove scope containment")
	}
	if ev.Truncated {
		verdict.DegradedReasons = append(verdict.DegradedReasons,
			"devmap symbol-cone traversal reported truncated=true; capped graph sample cannot prove scope containment")
	}
	if len(ev.PartialParseFiles) > 0 {
		verdict.DegradedReasons = append(verdict.DegradedReasons,
			"devmap reported partial_parse on "+itoa(len(ev.PartialParseFiles))+" file(s): "+strings.Join(ev.PartialParseFiles, ", "))
	}
	for _, sym := range ev.OutOfConeTouches {
		verdict.Gaps = append(verdict.Gaps, Gap{
			ID:             StableGapID(taskID, "CONE-"+sym),
			Severity:       "medium",
			GapType:        "architecture_drift",
			TaskID:         taskID,
			Description:    "Changed symbol `" + sym + "` lies outside the planned DevMap dependency cone (strength: derived).",
			Evidence:       []string{"symbol: " + sym, "strength: " + dcverify.StrengthDerived},
			RecommendedFix: "Verify whether touching `" + sym + "` was intended; revert out-of-cone edits or ask the task owner to widen planned scope.",
			Blocking:       false,
		})
	}
	return verdict
}
