package integrate

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// Target names what Uninstall removes.
type Target string

// TargetHooks removes every DevCouncil lifecycle-hook registration this
// repository ever wrote. DevCouncil host hooks are retired (Phase 7); the task
// gates live on the MCP surface instead.
const TargetHooks Target = "hooks"

// UninstallTargets is the set Uninstall accepts.
var UninstallTargets = []Target{TargetHooks}

// UninstallOptions for Uninstall.
type UninstallOptions struct {
	Root   string
	Target Target
	// Mode apply removes; dry-run and check only report.
	Mode Mode
}

// maxHostConfigBytes bounds a host config read. These files are hand-sized
// JSON; anything larger is not a config we should be rewriting.
const maxHostConfigBytes = 1 << 20

// devcouncilHookMarkers identify a hook entry as DevCouncil's. A host config is
// shared with other tools, so only entries naming DevCouncil's own hook
// dispatch are removed.
var devcouncilHookMarkers = []string{
	"dev hook",
	"devcouncil hook",
	"dev_hook",
}

// Uninstall removes host wiring for one target.
func Uninstall(opts UninstallOptions) (*Receipt, error) {
	root, err := filepath.Abs(opts.Root)
	if err != nil {
		return nil, err
	}
	target := opts.Target
	if target == "" {
		target = TargetHooks
	}
	if target != TargetHooks {
		return nil, fmt.Errorf("unknown uninstall target %q (known: %s)", target, joinTargets(UninstallTargets))
	}
	mode := opts.Mode
	if mode == "" {
		mode = ModeApply
	}

	receipt := &Receipt{Host: "uninstall:" + string(target), Mode: string(mode), Files: map[string]string{}}

	// Files DevCouncil owns outright: the whole file is hook registration.
	for _, rel := range []string{
		filepath.Join(".cursor", "hooks.json"),
		filepath.Join(".grok", "hooks", "devcouncil.json"),
	} {
		if err := removeOwnedFile(filepath.Join(root, rel), mode, receipt, toSlash(rel)); err != nil {
			return receipt, err
		}
	}

	// Files shared with other tools: strip only DevCouncil's own entries.
	for _, rel := range []string{
		filepath.Join(".claude", "settings.json"),
		filepath.Join(".claude", "settings.local.json"),
	} {
		if err := stripClaudeHooks(filepath.Join(root, rel), mode, receipt, toSlash(rel)); err != nil {
			return receipt, err
		}
	}

	receipt.Notes = append(receipt.Notes,
		"DevCouncil lifecycle hooks are retired; task gates live on the DevCouncil MCP tools.",
		"A host that already loaded a hook config keeps it until the window or session is reloaded.",
	)
	return receipt, nil
}

func removeOwnedFile(path string, mode Mode, receipt *Receipt, rel string) error {
	info, err := os.Lstat(path)
	if os.IsNotExist(err) {
		receipt.Files[rel] = "missing"
		return nil
	}
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() {
		// A symlink or directory here is not something this wrote; refuse
		// rather than following it out of the project.
		receipt.Files[rel] = "skipped_not_regular"
		return nil
	}
	if mode != ModeApply {
		receipt.Files[rel] = "would_remove"
		return nil
	}
	if err := os.Remove(path); err != nil {
		return err
	}
	receipt.Files[rel] = "removed"
	return nil
}

func stripClaudeHooks(path string, mode Mode, receipt *Receipt, rel string) error {
	data, err := readBoundedFile(path)
	if os.IsNotExist(err) {
		receipt.Files[rel] = "missing"
		return nil
	}
	if err != nil {
		return err
	}
	var settings map[string]any
	if err := json.Unmarshal(data, &settings); err != nil {
		return fmt.Errorf("%s: refusing to rewrite unparsable JSON: %w", path, err)
	}
	hooks, ok := settings["hooks"].(map[string]any)
	if !ok || len(hooks) == 0 {
		receipt.Files[rel] = "unchanged"
		return nil
	}

	removed := 0
	for event, raw := range hooks {
		groups, ok := raw.([]any)
		if !ok {
			continue
		}
		kept := make([]any, 0, len(groups))
		for _, rawGroup := range groups {
			group, ok := rawGroup.(map[string]any)
			if !ok {
				kept = append(kept, rawGroup)
				continue
			}
			entries, ok := group["hooks"].([]any)
			if !ok {
				kept = append(kept, rawGroup)
				continue
			}
			keptEntries := make([]any, 0, len(entries))
			for _, rawEntry := range entries {
				if isDevCouncilHookEntry(rawEntry) {
					removed++
					continue
				}
				keptEntries = append(keptEntries, rawEntry)
			}
			if len(keptEntries) == 0 {
				continue
			}
			group["hooks"] = keptEntries
			kept = append(kept, group)
		}
		if len(kept) == 0 {
			delete(hooks, event)
			continue
		}
		hooks[event] = kept
	}
	if removed == 0 {
		receipt.Files[rel] = "unchanged"
		return nil
	}
	if len(hooks) == 0 {
		delete(settings, "hooks")
	}
	if mode != ModeApply {
		receipt.Files[rel] = "would_clean"
		return nil
	}
	if err := writeFileAtomic(path, mustJSON(settings)); err != nil {
		return err
	}
	receipt.Files[rel] = "cleaned"
	return nil
}

func isDevCouncilHookEntry(raw any) bool {
	entry, ok := raw.(map[string]any)
	if !ok {
		return false
	}
	command, ok := entry["command"].(string)
	if !ok {
		return false
	}
	lowered := strings.ToLower(command)
	for _, marker := range devcouncilHookMarkers {
		if strings.Contains(lowered, marker) {
			return true
		}
	}
	return false
}

func readBoundedFile(path string) ([]byte, error) {
	info, err := os.Stat(path)
	if err != nil {
		return nil, err
	}
	if info.Size() > maxHostConfigBytes {
		return nil, fmt.Errorf("%s: %d bytes exceeds the %d-byte host config bound", path, info.Size(), maxHostConfigBytes)
	}
	return os.ReadFile(path)
}

func writeFileAtomic(path string, content []byte) error {
	return safefile.WriteAtomic(path, content, 0o644)
}

func joinTargets(targets []Target) string {
	names := make([]string, 0, len(targets))
	for _, t := range targets {
		names = append(names, string(t))
	}
	return strings.Join(names, ", ")
}

func toSlash(rel string) string { return filepath.ToSlash(rel) }
