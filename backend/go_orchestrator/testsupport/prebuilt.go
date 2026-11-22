package testsupport

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// NoCargoEnv selects prebuilt-binary mode. A caller that has already built
// dcstore, dcverify and dcgrep — a gate script making one gated cargo call
// before `go test` — sets it to 1, and every helper in this package then uses
// those binaries and makes zero cargo calls. A binary that is missing or older
// than the workspace sources fails the test; the helpers never fall back to
// building, because a silent build is what queues behind the cargo slot gate
// once per test process and outlasts the go test timeout.
//
// Without a per-binary override the binary is <target>/debug/<name>, where
// <target> is CARGO_TARGET_DIR when set and the workspace's target/ otherwise:
// where a plain `cargo build -p dc-store --bin dcstore` leaves it.
const NoCargoEnv = "DEVCOUNCIL_TEST_NO_CARGO"

// prebuiltEnv names the per-binary override. Like DEVMAP_BIN, a set override is
// used or refused, never replaced by a build. Setting one without NoCargoEnv
// affects only that binary.
var prebuiltEnv = map[string]string{
	"dcstore":  "DCSTORE_BIN",
	"dcverify": "DCVERIFY_BIN",
	"dcgrep":   "DCGREP_BIN",
	"devmap":   "DEVMAP_BIN",
}

// maxSourceFiles bounds the freshness walk: a workspace with more files than
// this is refused as unverifiable rather than walked without limit.
const maxSourceFiles = 200_000

// prebuiltBinary returns the prebuilt binary for the named binary, or ok=false
// when neither NoCargoEnv nor the binary's override asks for one. Every other
// outcome is a failure: a binary that is absent, not executable, or older than
// the sources it was built from is refused (never skipped, even under
// AllowSkipEnv: a stale binary is not a missing toolchain).
func prebuiltBinary(t testing.TB, crates, binary string) (path string, ok bool) {
	t.Helper()
	override := strings.TrimSpace(os.Getenv(prebuiltEnv[binary]))
	noCargo := os.Getenv(NoCargoEnv) == "1"
	if override == "" && !noCargo {
		return "", false
	}
	source := prebuiltEnv[binary] + "=" + override
	if override == "" {
		target := os.Getenv("CARGO_TARGET_DIR")
		if target == "" {
			target = filepath.Join(crates, "target")
		}
		override = filepath.Join(target, "debug", binary)
		source = NoCargoEnv + "=1 (looked for " + override + ")"
	}
	info, err := os.Stat(override)
	switch {
	case err != nil:
		t.Fatalf("cannot use a prebuilt %s: %v [%s]. This mode never runs cargo: build it first "+
			"(`cargo build -p <crate> --bin %s`) or unset the variable.", binary, err, source, binary)
	case info.IsDir() || info.Mode()&0o111 == 0:
		t.Fatalf("cannot use a prebuilt %s: %s is not an executable file [%s]", binary, override, source)
	}
	if newer, newerAt, err := newestSource(crates); err != nil {
		t.Fatalf("cannot use a prebuilt %s: could not check it against the sources under %s: %v [%s]",
			binary, crates, err, source)
	} else if newerAt.After(info.ModTime()) {
		t.Fatalf("refusing the prebuilt %s: %s (%s) is older than %s (%s). It may predate a schema or "+
			"protocol change, and this mode never runs cargo: rebuild it with `cargo build -p <crate> --bin %s`. [%s]",
			binary, override, info.ModTime().Format(time.RFC3339), newer, newerAt.Format(time.RFC3339), binary, source)
	}
	return override, true
}

var (
	sourceMu    sync.Mutex
	sourceCache = map[string]newestResult{}
)

type newestResult struct {
	path string
	at   time.Time
	err  error
}

// newestSource returns the most recently modified file that cargo would read to
// build a workspace binary: everything under the workspace except target/, VCS
// and dependency directories, and the tests/ and benches/ that no binary
// contains. A crate directory may be a symlink (Manvi's crates/ are links into
// DevCouncil's rust/), so each top-level entry is resolved before walking.
// Computed once per workspace per process: sources do not change mid-run.
func newestSource(crates string) (string, time.Time, error) {
	sourceMu.Lock()
	defer sourceMu.Unlock()
	if r, done := sourceCache[crates]; done {
		return r.path, r.at, r.err
	}
	path, at, err := scanSources(crates)
	sourceCache[crates] = newestResult{path, at, err}
	return path, at, err
}

func scanSources(crates string) (newest string, newestAt time.Time, err error) {
	files := 0
	skipDir := func(name string) bool {
		switch name {
		case "target", ".git", "node_modules", "tests", "benches":
			return true
		}
		return false
	}
	visit := func(path string) error {
		files++
		if files > maxSourceFiles {
			return fmt.Errorf("more than %d files; the freshness check is capped, so this is unverified", maxSourceFiles)
		}
		info, err := os.Stat(path)
		if err != nil {
			return err
		}
		if info.ModTime().After(newestAt) {
			newest, newestAt = path, info.ModTime()
		}
		return nil
	}
	walk := func(root string) error {
		return filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if d.IsDir() {
				if path != root && skipDir(d.Name()) {
					return filepath.SkipDir
				}
				return nil
			}
			return visit(path)
		})
	}
	entries, err := os.ReadDir(crates)
	if err != nil {
		return "", time.Time{}, err
	}
	for _, e := range entries {
		if skipDir(e.Name()) {
			continue
		}
		resolved, err := filepath.EvalSymlinks(filepath.Join(crates, e.Name()))
		if err != nil {
			return "", time.Time{}, err
		}
		info, err := os.Stat(resolved)
		if err != nil {
			return "", time.Time{}, err
		}
		if info.IsDir() {
			err = walk(resolved)
		} else {
			err = visit(resolved)
		}
		if err != nil {
			return "", time.Time{}, err
		}
	}
	if newest == "" {
		return "", time.Time{}, errors.New("no source files found")
	}
	return newest, newestAt, nil
}
