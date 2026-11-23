package testsupport

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// NoCargoEnv selects prebuilt-binary mode. A caller that has already built
// dcstore, dcverify, dcgrep and devmap — a gate script making one gated cargo
// call before `go test` — sets it to 1, and every store-backed helper in this
// package (DCStore, DCVerify, DCGrep, Devmap and what they feed) then uses
// those binaries and makes zero cargo calls. A binary that is missing, or not
// strictly newer than what it was compiled from, fails the test; the helpers
// never fall back to building, because a silent build is what queues behind the
// cargo slot gate once per test process and outlasts the go test timeout. Any
// value other than "1" (or unset or empty) fails the test rather than being
// read as "off". Tests of this package that call cargo by design clear the mode
// (see TestMain), so "zero cargo calls" is a claim about the helpers.
//
// Without an override the binary is <target>/debug/<name>, where <target> is
// CARGO_TARGET_DIR when set and the workspace's target/ otherwise: where a plain
// `cargo build -p <crate> --bin <name>` leaves it. DEVMAP_BIN, when set in this
// mode, is checked like the others; without the mode it keeps its old meaning
// (used or refused, no freshness check).
//
// The binary is handed to callers as a copy at the stable content-addressed
// path cargoBin has always used, never as the path cargo owns.
const NoCargoEnv = "DEVCOUNCIL_TEST_NO_CARGO"

// prebuiltEnv names the per-binary override. A set override is used or refused,
// never replaced by a build, and must be an absolute path. Setting one without
// NoCargoEnv affects only that binary.
var prebuiltEnv = map[string]string{
	"dcstore":  "DCSTORE_BIN",
	"dcverify": "DCVERIFY_BIN",
	"dcgrep":   "DCGREP_BIN",
	"devmap":   "DEVMAP_BIN",
}

// noCargoMode reports whether NoCargoEnv asks for prebuilt-only. A value that is
// neither "1" nor empty is refused: "true" silently meaning "off" would build.
func noCargoMode(t testing.TB) bool {
	t.Helper()
	switch v := os.Getenv(NoCargoEnv); v {
	case "":
		return false
	case "1":
		return true
	default:
		t.Fatalf("%s=%q is not understood: set it to 1 to use prebuilt binaries only, or unset it", NoCargoEnv, v)
		return false
	}
}

// prebuiltBinary returns a stable copy of the prebuilt binary, or ok=false when
// neither NoCargoEnv nor the binary's override asks for one. Every other outcome
// is a failure — never a skip, even under AllowSkipEnv, because a stale binary
// is not a missing toolchain.
//
// Freshness is judged against this workspace, not the binary's real origin: an
// explicit override, or a shared CARGO_TARGET_DIR written by another checkout,
// is trusted as named, and only has to be newer than the sources its own
// dep-info lists and than this workspace's manifests. What it was compiled from
// is read from the dep-info cargo leaves beside it (<name>.d): every listed
// path must exist and be older than the binary. A missing or unparseable
// dep-info is refused as unverifiable.
//
// Entries inside git metadata (a .git component) are skipped: they are a build
// script's rerun triggers for embedded version info, not compiled source, and
// in a linked worktree .git is a file. Manifests are checked only inside the
// workspace. The trade-off: a path dependency outside the workspace keeps its
// listed sources checked but not its Cargo.toml. If every entry is skipped as git
// metadata the binary is refused as unverifiable.
//
// A successful verdict is cached for the life of the process, keyed by the
// binary's path, mtime and size: this matches cargoBin's once-per-process build,
// so a source edited later in the same process is not seen. A refusal is never
// cached.
func prebuiltBinary(t testing.TB, crates, crate, binary string) (path string, ok bool) {
	t.Helper()
	noCargo := noCargoMode(t)
	override := strings.TrimSpace(os.Getenv(prebuiltEnv[binary]))
	if override == "" && !noCargo {
		return "", false
	}
	source := prebuiltEnv[binary] + "=" + override
	switch {
	case override == "":
		target := os.Getenv("CARGO_TARGET_DIR")
		if target == "" {
			target = filepath.Join(crates, "target")
		}
		override = filepath.Join(target, "debug", binary)
		source = NoCargoEnv + "=1 (looked for " + override + ")"
	case !filepath.IsAbs(override):
		t.Fatalf("cannot use a prebuilt %s: %s is a relative path, and a test's working directory is its "+
			"package directory, so it would name something other than what the caller meant; use an absolute path",
			binary, source)
	}
	info, err := os.Stat(override)
	switch {
	case err != nil:
		t.Fatalf("cannot use a prebuilt %s: %v [%s]. This mode never runs cargo: build it first "+
			"(`cargo build -p %s --bin %s`) or unset the variable.", binary, err, source, crate, binary)
	case info.IsDir() || info.Mode()&0o111 == 0:
		t.Fatalf("cannot use a prebuilt %s: %s is not an executable file [%s]", binary, override, source)
	}
	key := fmt.Sprintf("%s|%d|%d|%s", override, info.ModTime().UnixNano(), info.Size(), crates)
	prebuiltMu.Lock()
	cached, done := prebuiltDone[key]
	prebuiltMu.Unlock()
	if done {
		return cached, true
	}
	if stale, err := staleSource(crates, crate, binary, override, info.ModTime()); err != nil {
		t.Fatalf("cannot use a prebuilt %s: %v [%s]", binary, err, source)
	} else if stale != "" {
		t.Fatalf("refusing the prebuilt %s: %s (%s) is not newer than %s. It may predate a schema or "+
			"protocol change, and this mode never runs cargo: rebuild it with `cargo build -p %s --bin %s`. [%s]",
			binary, override, info.ModTime().Format(time.RFC3339), stale, crate, binary, source)
	}
	data, err := readStableArtifact(override)
	if err != nil {
		t.Fatalf("cannot use a prebuilt %s: %v [%s]", binary, err, source)
	}
	stable, err := publishArtifact(filepath.Join(crates, "target"), binary, data)
	if err != nil {
		t.Fatalf("cannot use a prebuilt %s: publishing a stable copy: %v [%s]", binary, err, source)
	}
	prebuiltMu.Lock()
	prebuiltDone[key] = stable
	prebuiltMu.Unlock()
	return stable, true
}

// Only successes are remembered: a refusal ends its test, and a cached one would
// follow the next test that fixed the cause.
var (
	prebuiltMu   sync.Mutex
	prebuiltDone = map[string]string{}
)

// staleSource returns a source the binary is not strictly newer than, "" when it
// is newer than all of them, and an error when that could not be established.
// The sources are the paths in the dep-info beside the binary, which must all
// exist, plus the workspace Cargo.toml, Cargo.lock and the crate's Cargo.toml,
// which dep-info does not list and which may be absent (a missing manifest is
// not a source of newer bytes). A crate directory may be a symlink (Manvi's
// crates/ link into DevCouncil's rust/); stat follows it.
func staleSource(crates, crate, binary, path string, built time.Time) (string, error) {
	depInfo := filepath.Join(filepath.Dir(path), binary+".d")
	raw, err := os.ReadFile(depInfo)
	if err != nil {
		return "", fmt.Errorf("no dep-info to check it against (%v): without what it was compiled from, "+
			"whether it is current cannot be told", err)
	}
	// The verdict is cached for the life of the process (see prebuiltBinary), the
	// same once-per-process rule as cargoBin's build: a source edited after this
	// check, in the same process, is not seen.
	listed, err := parseDepInfo(string(raw))
	if err != nil {
		return "", fmt.Errorf("dep-info %s is unusable (%v), so whether the binary is current cannot be told", depInfo, err)
	}
	var sources []string
	for _, src := range listed {
		// An entry inside git metadata is a build script's rerun trigger for
		// embedded version info (devmap-cli's build.rs lists ../../.git/HEAD and
		// .git/index), not compiled source. In a linked worktree .git is a file,
		// so the path cannot even be opened, and .git/index changes on most git
		// operations, which would make the binary look stale constantly.
		if inGitMetadata(src) {
			continue
		}
		sources = append(sources, filepath.Clean(src))
		info, err := os.Stat(src)
		if err != nil {
			return "", fmt.Errorf("dep-info %s lists %s, which cannot be read (%v): the binary was built from "+
				"a source that is gone, so it cannot be shown current", depInfo, src, err)
		}
		if !built.After(info.ModTime()) {
			return src, nil
		}
	}
	if len(sources) == 0 {
		return "", fmt.Errorf("dep-info %s lists no compiled source once git metadata is set aside "+
			"(a checkout under a path with a .git component looks like this), so the binary cannot be shown current", depInfo)
	}
	// A crate whose source is listed also contributes its manifest — features and
	// edition live there and dep-info does not list it — so each listed path's
	// nearest enclosing Cargo.toml is checked too, once per directory.
	manifests := []string{
		filepath.Join(crates, "Cargo.toml"), filepath.Join(crates, "Cargo.lock"),
		filepath.Join(crates, crate, "Cargo.toml"),
	}
	seen := map[string]bool{}
	for _, src := range sources {
		// Only manifests inside the workspace are dependency manifests: a
		// source outside it (a registry crate, a path in the home directory)
		// must not make a stray Cargo.toml further up decide the verdict.
		for dir := filepath.Dir(src); within(crates, dir) && !seen[dir]; dir = filepath.Dir(dir) {
			seen[dir] = true
			manifest := filepath.Join(dir, "Cargo.toml")
			if _, err := os.Stat(manifest); err == nil {
				manifests = append(manifests, manifest)
				break
			}
			if dir == filepath.Dir(dir) {
				break
			}
		}
	}
	for _, manifest := range manifests {
		info, err := os.Stat(manifest)
		switch {
		case errors.Is(err, os.ErrNotExist):
			continue
		case err != nil:
			return "", fmt.Errorf("cannot read %s: %v", manifest, err)
		case !built.After(info.ModTime()):
			return manifest, nil
		}
	}
	return "", nil
}

// inGitMetadata reports whether a path has a .git component.
func inGitMetadata(path string) bool {
	for _, part := range strings.Split(filepath.Clean(path), string(filepath.Separator)) {
		if part == ".git" {
			return true
		}
	}
	return false
}

// within reports whether dir is root or lies below it, lexically.
func within(root, dir string) bool {
	rel, err := filepath.Rel(filepath.Clean(root), dir)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}

// parseDepInfo reads the Makefile-syntax dependency line cargo writes beside a
// binary: `<target>: <dep> <dep> ...`, spaces in a name escaped with a
// backslash. rustc may add one empty `<dep>:` rule per dependency after the
// first line; those carry no information and are ignored. An empty dependency
// list is an error: a binary compiled from nothing is not a real one.
func parseDepInfo(text string) ([]string, error) {
	line, _, _ := strings.Cut(text, "\n")
	// A line ending in an odd number of backslashes continues onto the next
	// line, whose dependencies this parser does not read: refuse rather than
	// accept a list it knows is cut short.
	if trimmed := strings.TrimRight(line, "\\"); (len(line)-len(trimmed))%2 == 1 {
		return nil, errors.New("the dependency line ends in a backslash continuation, which is not supported")
	}
	// The target path may itself hold escaped spaces, so the separator is the
	// first ": " that is not escaped.
	sep := -1
	for i := 0; i+1 < len(line); i++ {
		if line[i] == '\\' {
			i++
			continue
		}
		if line[i] == ':' && line[i+1] == ' ' {
			sep = i
			break
		}
	}
	if sep < 0 {
		return nil, errors.New("no \"target: dependencies\" line")
	}
	var deps []string
	var cur strings.Builder
	flush := func() {
		if cur.Len() > 0 {
			deps = append(deps, cur.String())
			cur.Reset()
		}
	}
	rest := line[sep+2:]
	for i := 0; i < len(rest); i++ {
		switch c := rest[i]; {
		case c == '\\' && i+1 < len(rest):
			i++
			cur.WriteByte(rest[i])
		case c == ' ' || c == '\t' || c == '\r':
			flush()
		default:
			cur.WriteByte(c)
		}
	}
	flush()
	for _, dep := range deps {
		// A backslash that survives unescaping is a literal one in a path; the
		// escaping rules for it differ between make dialects, so a path holding
		// one is not trusted to name the file it was meant to.
		if strings.Contains(dep, "\\") {
			return nil, fmt.Errorf("path %q holds a literal backslash, which is not supported", dep)
		}
	}
	if len(deps) == 0 {
		return nil, errors.New("it lists no dependencies")
	}
	return deps, nil
}
