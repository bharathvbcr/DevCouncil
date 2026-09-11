// Package components is the install catalog for DevCouncil binaries.
//
// Scripts under scripts/ remain the bootstrap (no host binary yet). The
// `devcouncil install` CLI uses this catalog after the host exists, and the
// two must agree on names.
package components

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// Kind is how a component is built.
type Kind string

const (
	KindGo   Kind = "go"
	KindRust Kind = "rust"
)

// Component is one independently installable binary.
type Component struct {
	ID          string `json:"id"`
	Binary      string `json:"binary"`
	Kind        Kind   `json:"kind"`
	Package     string `json:"package"`
	Description string `json:"description"`
	Standalone  bool   `json:"standalone"`
}

// Catalog is the ordered list. Presets resolve to subsets of these IDs.
var Catalog = []Component{
	{
		ID:          "host",
		Binary:      "devcouncil",
		Kind:        KindGo,
		Package:     "./cmd/devcouncil",
		Description: "Go host: MCP, integrate, skills, verify, map forwarding (also installed as `dev`)",
	},
	{
		ID:          "devmap",
		Binary:      "devmap",
		Kind:        KindRust,
		Package:     "devmap-cli",
		Description: "Code-intelligence graph CLI — installable on its own",
		Standalone:  true,
	},
	{
		ID:          "dcstore",
		Binary:      "dcstore",
		Kind:        KindRust,
		Package:     "dc-store",
		Description: "Task and lease store",
	},
	{
		ID:          "dcverify",
		Binary:      "dcverify",
		Kind:        KindRust,
		Package:     "dc-verify",
		Description: "Deterministic verifier",
	},
	{
		ID:          "dcgrep",
		Binary:      "dcgrep",
		Kind:        KindRust,
		Package:     "dc-grep",
		Description: "Ignore-aware repository search",
	},
}

// Preset expands a named bundle. Preset names win over component IDs when both
// could match (devmap is both).
var Presets = map[string][]string{
	"all":       {"host", "devmap", "dcstore", "dcverify", "dcgrep"},
	"analysis":  {"devmap", "dcstore", "dcverify", "dcgrep"},
	"codeintel": {"devmap"},
	"devmap":    {"devmap"},
	"host":      {"host"},
}

// Lookup returns a catalog entry by ID.
func Lookup(id string) (Component, bool) {
	for _, c := range Catalog {
		if c.ID == id {
			return c, true
		}
	}
	return Component{}, false
}

// Resolve turns positional names (presets and/or IDs) into a de-duplicated
// component list. Empty means the `all` preset.
func Resolve(names []string) ([]Component, error) {
	if len(names) == 0 {
		names = []string{"all"}
	}
	seen := map[string]bool{}
	var out []Component
	for _, raw := range names {
		n := strings.TrimSpace(strings.ToLower(raw))
		if n == "" {
			continue
		}
		if ids, ok := Presets[n]; ok {
			for _, id := range ids {
				if seen[id] {
					continue
				}
				c, ok := Lookup(id)
				if !ok {
					return nil, fmt.Errorf("internal: preset %q names unknown component %q", n, id)
				}
				seen[id] = true
				out = append(out, c)
			}
			continue
		}
		c, ok := Lookup(n)
		if !ok {
			return nil, fmt.Errorf("unknown component or preset %q (known: %s)", raw, KnownNames())
		}
		if seen[c.ID] {
			continue
		}
		seen[c.ID] = true
		out = append(out, c)
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("no components selected")
	}
	return out, nil
}

// KnownNames is the help string for errors.
func KnownNames() string {
	names := make([]string, 0, len(Catalog)+len(Presets))
	for _, c := range Catalog {
		names = append(names, c.ID)
	}
	for p := range Presets {
		if _, isComponent := Lookup(p); isComponent {
			continue
		}
		names = append(names, p)
	}
	sort.Strings(names)
	return strings.Join(names, ", ")
}

// IncludesHost reports whether the selection contains the Go host.
func IncludesHost(cs []Component) bool {
	for _, c := range cs {
		if c.ID == "host" {
			return true
		}
	}
	return false
}

// RustIDs returns rust component IDs in catalog order.
func RustIDs(cs []Component) []string {
	var ids []string
	for _, c := range cs {
		if c.Kind == KindRust {
			ids = append(ids, c.ID)
		}
	}
	return ids
}

// FindSourceRoot locates a DevCouncil checkout. Empty string means none.
func FindSourceRoot() string {
	if v := strings.TrimSpace(os.Getenv("DEVCOUNCIL_ROOT")); v != "" && looksLikeCheckout(v) {
		return v
	}
	if v := strings.TrimSpace(os.Getenv("GITPULSE_DEVCOUNCIL_ROOT")); v != "" && looksLikeCheckout(v) {
		return v
	}
	wd, err := os.Getwd()
	if err != nil {
		return ""
	}
	for dir := wd; ; {
		if looksLikeCheckout(dir) {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			break
		}
		dir = parent
	}
	return ""
}

func looksLikeCheckout(root string) bool {
	_, err1 := os.Stat(filepath.Join(root, "rust", "devmap-cli", "Cargo.toml"))
	_, err2 := os.Stat(filepath.Join(root, "backend", "go_orchestrator", "go.mod"))
	return err1 == nil && err2 == nil
}

// DefaultPrefix is ~/.local, matching the install scripts.
func DefaultPrefix() string {
	if v := strings.TrimSpace(os.Getenv("PREFIX")); v != "" {
		return v
	}
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return filepath.Join(".", ".local")
	}
	return filepath.Join(home, ".local")
}

const (
	PublicGit     = "https://github.com/bharathvbcr/DevCouncil.git"
	HostGoInstall = "github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/cmd/devcouncil@latest"
)
