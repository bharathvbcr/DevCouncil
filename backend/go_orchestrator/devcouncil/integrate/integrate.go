package integrate

import (
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// Mode is apply / check / dry-run.
type Mode string

const (
	ModeApply  Mode = "apply"
	ModeCheck  Mode = "check"
	ModeDryRun Mode = "dry-run"
)

// Hosts supported by Phase 4.
var Hosts = []string{
	"cursor", "claude", "codex", "gemini", "opencode", "warp", "aider", "antigravity",
}

// Options for Integrate.
type Options struct {
	Root      string
	Host      string
	Mode      Mode
	WriteGate bool
	DevmapBin string
	SelfBin   string // path to this devcouncil binary
}

// Receipt records what was written or would be written.
type Receipt struct {
	Host    string            `json:"host"`
	Mode    string            `json:"mode"`
	Files   map[string]string `json:"files"` // rel path -> action (wrote|unchanged|would_write|missing)
	Spawned []string          `json:"spawned,omitempty"`
	Notes   []string          `json:"notes,omitempty"`
}

// cursorRule is deliberately split in two registers.
//
// Code navigation stays directive: an agent that guesses where a symbol lives
// reads files it did not need and still answers from the wrong one, so DevMap
// remains the instruction. The DevCouncil *task loop* is advisory, because
// nothing enforces it — host hooks are retired and the gates are off — and a
// rule that tells an agent it must check out a lease that no gate requires
// produces exactly one behaviour: an agent that stops to ask for a lease before
// running `ls`.
const cursorRule = `---
description: DevCouncil navigation, and the task loop when you want it
alwaysApply: true
---

# DevCouncil

Navigate with DevMap before reading or grepping: ` + "`devmap_explore`" + `, ` + "`devmap_search`" + `, ` + "`devmap_impact`" + `, ` + "`devmap_trace`" + `, ` + "`devmap_neighbors`" + `, ` + "`devmap_dead_symbols`" + `, ` + "`devmap_affected_tests`" + ` — or the matching ` + "`devmap`" + ` CLI commands. Run impact analysis before editing a symbol, and read ` + "`truncated`" + ` / ` + "`total`" + ` on every envelope before treating a list as complete. When DevMap cannot answer, record a gap in ` + "`.devcouncil/codeintel/sessions/gaps.jsonl`" + ` rather than switching to another index.

` + "`.devcouncil/repo_map.json`" + ` is the file-level index (subsystems, entry_points, critical_files).

## Tasks and gates are opt-in

Nothing here gates you. DevCouncil host hooks are retired, ` + "`execution.hook_gate.mode`" + ` is
off and ` + "`integrations.cursor.write_gate`" + ` is false, so **interactive Shell and Write need
no task lease**. Do not claim "Shell is gated", and do not check out a task in order to
run a command. Use the ` + "`devcouncil_*`" + ` MCP tools when you actually want task state,
scope or verification — and prefer them over guessing it. A lease matters only where
containment is switched on deliberately (` + "`devcouncil integrate … --write-gate`" + ` or
` + "`execution.hook_gate.mode: contain`" + `).

Engineering skills live under ` + "`.cursor/skills/`" + ` and ` + "`.claude/skills/`" + ` (` + "`devcouncil skills scaffold`" + `).
`

// Run configures one host.
func Run(opts Options) (*Receipt, error) {
	root, err := filepath.Abs(opts.Root)
	if err != nil {
		return nil, err
	}
	host := strings.ToLower(opts.Host)
	mode := opts.Mode
	if mode == "" {
		mode = ModeCheck
	}
	selfBin := opts.SelfBin
	if selfBin == "" {
		selfBin, _ = os.Executable()
	}
	devmap := opts.DevmapBin
	if devmap == "" {
		devmap, _ = exec.LookPath("devmap")
		if devmap == "" {
			devmap = "devmap"
		}
	}

	receipt := &Receipt{Host: host, Mode: string(mode), Files: map[string]string{}}

	switch host {
	case "cursor":
		if err := integrateCursor(root, selfBin, devmap, mode, opts.WriteGate, receipt); err != nil {
			return receipt, err
		}
	case "claude":
		if err := integrateClaude(root, selfBin, devmap, mode, receipt); err != nil {
			return receipt, err
		}
	case "codex":
		if err := integrateCodex(root, mode, receipt); err != nil {
			return receipt, err
		}
	default:
		receipt.Notes = append(receipt.Notes, fmt.Sprintf("host %q: Phase 4 writes a stub receipt; full adapter ports follow", host))
	}

	// Compose DevMap assets rather than reimplementing them.
	if mode == ModeApply || mode == ModeDryRun {
		args := []string{"integrate", host}
		if mode == ModeDryRun {
			args = append(args, "--dry-run")
		}
		cmd := exec.Command(devmap, args...)
		cmd.Dir = root
		out, err := cmd.CombinedOutput()
		receipt.Spawned = append(receipt.Spawned, "devmap "+strings.Join(args, " "))
		if err != nil {
			receipt.Notes = append(receipt.Notes, fmt.Sprintf("devmap integrate: %v (%s)", err, truncate(string(out), 400)))
		}
		skillArgs := append([]string{"skills", "install"}, skillInstallArgs(root, host)...)
		if mode == ModeDryRun {
			skillArgs = append(skillArgs, "--dry-run")
		}
		scmd := exec.Command(devmap, skillArgs...)
		scmd.Dir = root
		sout, serr := scmd.CombinedOutput()
		receipt.Spawned = append(receipt.Spawned, "devmap "+strings.Join(skillArgs, " "))
		if serr != nil {
			receipt.Notes = append(receipt.Notes, fmt.Sprintf("devmap skills install: %v (%s)", serr, truncate(string(sout), 400)))
		}
	}
	return receipt, nil
}

// hostSkillDirs maps a host to the skill layout it reads. A host absent here
// gets DevMap's own defaults (`.claude/skills`, `.cursor/skills`,
// `.agents/skills`) rather than a guess.
var hostSkillDirs = map[string][]string{
	"cursor":      {".cursor/skills"},
	"claude":      {".claude/skills"},
	"antigravity": {".agents/skills"},
}

// skillInstallArgs names the repository and, where the host has one, its skill
// directory.
//
// `--destination` is a layout root *relative to the project*, not a project
// root: passing the absolute repository path there — which is what this did —
// is refused by name, and `integrate --apply` then reported success with a note
// nobody reads while installing no skills at all.
func skillInstallArgs(root, host string) []string {
	args := []string{"--project-root", root}
	for _, dir := range hostSkillDirs[host] {
		args = append(args, "--destination", dir)
	}
	return args
}

func integrateCursor(root, selfBin, devmap string, mode Mode, writeGate bool, receipt *Receipt) error {
	mcpPath := filepath.Join(root, ".cursor", "mcp.json")
	rulePath := filepath.Join(root, ".cursor", "rules", "devcouncil.mdc")

	mcp := map[string]any{
		"mcpServers": map[string]any{
			"devcouncil": map[string]any{
				"type":    "stdio",
				"command": selfBin,
				"args":    []string{"mcp"},
				"env": map[string]string{
					"DEVCOUNCIL_PROJECT_ROOT": root,
				},
			},
			"devmap": map[string]any{
				"type":    "stdio",
				"command": devmap,
				"args":    []string{"mcp"},
			},
		},
	}
	_ = writeGate

	if err := planWrite(mcpPath, mustJSON(mcp), mode, receipt, ".cursor/mcp.json"); err != nil {
		return err
	}
	if err := planWrite(rulePath, []byte(cursorRule), mode, receipt, ".cursor/rules/devcouncil.mdc"); err != nil {
		return err
	}
	// No .cursor/hooks.json: DevCouncil lifecycle hooks are retired, and an
	// empty stub is still a hook config the host parses and dispatches from.
	// `devcouncil integrate uninstall --target hooks` removes any left over.
	receipt.Notes = append(receipt.Notes, "hooks not written: DevCouncil host hooks are retired (MCP-first)")
	return nil
}

func integrateClaude(root, selfBin, devmap string, mode Mode, receipt *Receipt) error {
	mcpPath := filepath.Join(root, ".mcp.json")
	mcp := map[string]any{
		"mcpServers": map[string]any{
			"devcouncil": map[string]any{
				"command": selfBin,
				"args":    []string{"mcp"},
				"env":     map[string]string{"DEVCOUNCIL_PROJECT_ROOT": root},
			},
			"devmap": map[string]any{
				"command": devmap,
				"args":    []string{"mcp"},
			},
		},
	}
	return planWrite(mcpPath, mustJSON(mcp), mode, receipt, ".mcp.json")
}

func integrateCodex(root string, mode Mode, receipt *Receipt) error {
	cfg := filepath.Join(root, ".codex", "config.toml")
	body := []byte("# Managed by devcouncil integrate codex\n")
	return planWrite(cfg, body, mode, receipt, ".codex/config.toml")
}

func planWrite(path string, content []byte, mode Mode, receipt *Receipt, rel string) error {
	existing, err := os.ReadFile(path)
	exists := err == nil
	if err != nil && !os.IsNotExist(err) {
		return err
	}
	switch mode {
	case ModeCheck:
		if !exists {
			receipt.Files[rel] = "missing"
			return nil
		}
		if string(existing) == string(content) {
			receipt.Files[rel] = "unchanged"
		} else {
			receipt.Files[rel] = "drift"
		}
		return nil
	case ModeDryRun:
		receipt.Files[rel] = "would_write"
		return nil
	case ModeApply:
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		// Refuse JSON-with-comments for mcp.json: encoding/json cannot round-trip it.
		if strings.HasSuffix(rel, "mcp.json") && exists {
			if err := refuseJSONComments(existing, path); err != nil {
				return err
			}
			merged, err := mergeMCPJSON(existing, content)
			if err != nil {
				return err
			}
			content = merged
		}
		tmp := path + ".tmp"
		if err := os.WriteFile(tmp, content, 0o644); err != nil {
			return err
		}
		if err := os.Rename(tmp, path); err != nil {
			return err
		}
		receipt.Files[rel] = "wrote"
		return nil
	}
	return fmt.Errorf("unknown mode %q", mode)
}

func refuseJSONComments(data []byte, path string) error {
	trimmed := strings.TrimSpace(string(data))
	if strings.Contains(trimmed, "//") || strings.Contains(trimmed, "/*") {
		// Heuristic: real JSON strings can contain //; try parse first.
		var probe any
		if err := json.Unmarshal(data, &probe); err != nil {
			return fmt.Errorf("%s: refusing to rewrite JSON-with-comments (parse failed: %v)", path, err)
		}
	}
	return nil
}

func mergeMCPJSON(existing, generated []byte) ([]byte, error) {
	var cur, gen map[string]any
	if err := json.Unmarshal(existing, &cur); err != nil {
		return nil, fmt.Errorf("existing mcp.json: %w", err)
	}
	if err := json.Unmarshal(generated, &gen); err != nil {
		return nil, err
	}
	curServers, _ := cur["mcpServers"].(map[string]any)
	if curServers == nil {
		curServers = map[string]any{}
		cur["mcpServers"] = curServers
	}
	genServers, _ := gen["mcpServers"].(map[string]any)
	for k, v := range genServers {
		curServers[k] = v
	}
	return mustJSON(cur), nil
}

func mustJSON(v any) []byte {
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return []byte("{}\n")
	}
	return append(b, '\n')
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}
