package devcouncil

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
)

const defaultRegistryPoolCap = 16

// RegistryPool caches per-repository *Registry instances for a global MCP server
// so tool calls carrying repo_path or root can route across repositories without
// restarting the server. Evicted store clients are retired rather than closed
// mid-call and are closed together when Close runs after in-flight calls drain.
type RegistryPool struct {
	mu         sync.Mutex
	maxEntries int
	build      func(root string) (*Registry, error)
	entries    map[string]*Registry
	order      []string // oldest first, most-recently-used last
	retired    []*store.Client
}

// NewRegistryPool builds a bounded LRU pool of per-repo registries.
func NewRegistryPool(maxEntries int, build func(root string) (*Registry, error)) *RegistryPool {
	if maxEntries <= 0 {
		maxEntries = defaultRegistryPoolCap
	}
	return &RegistryPool{
		maxEntries: maxEntries,
		build:      build,
		entries:    make(map[string]*Registry),
	}
}

// Get returns a cached *Registry for root, building one on first access.
func (p *RegistryPool) Get(root string) (*Registry, error) {
	if p == nil || p.build == nil {
		return nil, fmt.Errorf("no repository resolver configured")
	}
	key := canonicalRepoKey(root)

	p.mu.Lock()
	if reg, ok := p.entries[key]; ok {
		p.touchLocked(key)
		p.mu.Unlock()
		return reg, nil
	}
	p.mu.Unlock()

	reg, err := p.build(root)
	if err != nil {
		return nil, err
	}
	if reg == nil {
		return nil, fmt.Errorf("repository %q produced a nil registry", root)
	}

	p.mu.Lock()
	defer p.mu.Unlock()
	if existing, ok := p.entries[key]; ok {
		// Another goroutine raced and cached one first; retire the duplicate
		// store client so nothing leaks.
		if reg.Store != nil && reg.Store != existing.Store {
			p.retired = append(p.retired, reg.Store)
		}
		p.touchLocked(key)
		return existing, nil
	}
	if len(p.order) >= p.maxEntries {
		oldest := p.order[0]
		p.order = p.order[1:]
		if evicted, ok := p.entries[oldest]; ok {
			delete(p.entries, oldest)
			if evicted != nil && evicted.Store != nil {
				p.retired = append(p.retired, evicted.Store)
			}
		}
	}
	p.entries[key] = reg
	p.order = append(p.order, key)
	return reg, nil
}

// Close closes every store client owned by active and retired pooled registries.
// Call after in-flight requests have drained.
func (p *RegistryPool) Close() {
	if p == nil {
		return
	}
	p.mu.Lock()
	clients := make([]*store.Client, 0, len(p.entries)+len(p.retired))
	seen := make(map[*store.Client]struct{}, len(p.entries)+len(p.retired))
	add := func(c *store.Client) {
		if c == nil {
			return
		}
		if _, dup := seen[c]; dup {
			return
		}
		seen[c] = struct{}{}
		clients = append(clients, c)
	}
	for _, reg := range p.entries {
		if reg != nil {
			add(reg.currentStore())
		}
	}
	for _, c := range p.retired {
		add(c)
	}
	p.entries = make(map[string]*Registry)
	p.order = nil
	p.retired = nil
	p.mu.Unlock()

	for _, c := range clients {
		c.Close()
	}
}

func (p *RegistryPool) touchLocked(key string) {
	for i, k := range p.order {
		if k == key {
			p.order = append(p.order[:i], p.order[i+1:]...)
			p.order = append(p.order, key)
			return
		}
	}
	p.order = append(p.order, key)
}

func canonicalRepoKey(dir string) string {
	clean := filepath.Clean(dir)
	if resolved, err := filepath.EvalSymlinks(clean); err == nil {
		return resolved
	}
	return clean
}

func sameRepoDir(a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	ca, cb := filepath.Clean(a), filepath.Clean(b)
	if ca == cb {
		return true
	}
	return canonicalRepoKey(ca) == canonicalRepoKey(cb)
}

func validateRepoRootArg(tool, field, raw string) (string, error) {
	if strings.TrimSpace(raw) == "" {
		return "", fmt.Errorf("%s: %s is empty; omit it to use the server's default repository root", tool, field)
	}
	if strings.Contains(raw, "$") {
		return "", fmt.Errorf("%s: %s %q contains an unexpanded shell variable; pass an absolute repository path", tool, field, raw)
	}
	if !filepath.IsAbs(raw) {
		return "", fmt.Errorf("%s: %s %q is relative; it must be an absolute path to the repository", tool, field, raw)
	}
	info, err := os.Stat(raw)
	if err != nil {
		return "", fmt.Errorf("%s: %s %q cannot be used: %v", tool, field, raw, err)
	}
	if !info.IsDir() {
		return "", fmt.Errorf("%s: %s %q is not a directory", tool, field, raw)
	}
	return filepath.Clean(raw), nil
}
