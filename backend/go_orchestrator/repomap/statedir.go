package repomap

import (
	"os"
	"path/filepath"
)

// The state-directory rule is DevMap's, and its owner is
// rust/devmap-extract/src/paths.rs (`resolve_state_dir`). This is a port of that
// one function, not a second definition: the rule is a pure function of the
// environment and of which directories exist — the Rust docs say so precisely so
// that a host with no shared config loader can answer identically — and the
// constants below are the ones that file defines. When the rule changes there,
// it changes here; TestResolveStateDirPrecedence pins the four rungs.
//
// Hard-coding `.devcouncil/graph/code_graph.json` instead is the failure the
// rule exists to prevent: a repository that has migrated to `.devmap/`, or that
// sets $DEVMAP_HOME, would be read from a directory the producer no longer
// writes, and the gate would consult a stale graph — or none, reported as an
// index that was never built.
const (
	// DevmapHomeEnv overrides the state directory. Absolute is used as given;
	// relative is joined onto the repository root.
	DevmapHomeEnv = "DEVMAP_HOME"
	// StandaloneStateDir is what a repository with neither directory gets.
	StandaloneStateDir = ".devmap"
	// LegacyStateDir is the directory DevMap shares with DevCouncil when a
	// repository already has one.
	LegacyStateDir = ".devcouncil"
	// CodeGraphRelPath is the symbol-level graph, relative to the state dir.
	CodeGraphRelPath = "graph/code_graph.json"
)

// ResolveStateDir is the precedence itself, with the environment value and the
// filesystem passed in so it can be tested without process-wide state:
//
//  1. home, when non-empty — an empty value is a broken expansion far more
//     often than a request for the repository root, so it counts as unset;
//  2. <root>/.devmap when it is a directory;
//  3. <root>/.devcouncil when it is a directory;
//  4. <root>/.devmap.
func ResolveStateDir(root, home string, isDir func(string) bool) string {
	if home != "" {
		if filepath.IsAbs(home) {
			return home
		}
		return filepath.Join(root, home)
	}
	if standalone := filepath.Join(root, StandaloneStateDir); isDir(standalone) {
		return standalone
	}
	if legacy := filepath.Join(root, LegacyStateDir); isDir(legacy) {
		return legacy
	}
	return filepath.Join(root, StandaloneStateDir)
}

// StateDir resolves the state directory for root from $DEVMAP_HOME and the
// filesystem.
func StateDir(root string) string {
	return ResolveStateDir(root, os.Getenv(DevmapHomeEnv), func(p string) bool {
		info, err := os.Stat(p)
		return err == nil && info.IsDir()
	})
}

// CodeGraphPath is where DevMap writes root's code graph.
func CodeGraphPath(root string) string {
	return filepath.Join(StateDir(root), filepath.FromSlash(CodeGraphRelPath))
}
