package integrate

import (
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
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
	HookEntries map[string]int    `json:"hook_entries,omitempty"` // managed entries found before cleanup
	Backups     map[string]string `json:"backups,omitempty"`      // original relative path -> recoverable backup
	Host        string            `json:"host"`
	Mode        string            `json:"mode"`
	Files       map[string]string `json:"files"` // rel path -> action (wrote|unchanged|would_write|missing)
	Spawned     []string          `json:"spawned,omitempty"`
	Notes       []string          `json:"notes,omitempty"`
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

## Product stack

DevCouncil is **components and modules** (` + "`devmap`" + `, ` + "`dcstore`" + `, ` + "`dcverify`" + `, ` + "`dcgrep`" + `, and this host). **Manvi** wraps them into a coding-agent harness. Host apps such as **GitPulse** use Manvi for policy, workbench and agent hosting, and DevCouncil components for code intelligence and related analysis. Take only the modules an app needs; update them independently.

## Tasks and gates are opt-in

DevCouncil host hooks are retired. Legacy ` + "`execution.hook_gate.mode`" + ` and
` + "`integrations.cursor.write_gate`" + ` settings do not enforce host operations, so
**interactive Shell and Write need no task lease**. Do not claim "Shell is gated", and do not check out a task in order to
run a command. Use the ` + "`devcouncil_*`" + ` MCP tools when you actually want task state,
scope or verification — and prefer them over guessing it. A host that uses MCP policy
must honor its results. Retired hooks do not enforce old containment settings;
` + "`--write-gate`" + ` is refused rather than claiming to install enforcement.

Engineering skills live under ` + "`.cursor/skills/`" + ` and ` + "`.claude/skills/`" + ` (` + "`devcouncil skills scaffold`" + `).
`

// Run configures one host.
func Run(opts Options) (*Receipt, error) {
	if opts.WriteGate {
		return nil, fmt.Errorf("--write-gate is unavailable: DevCouncil lifecycle hooks are retired; use MCP policy and verification explicitly")
	}

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
	// Two different questions, and conflating them is what let repository
	// content be executed. `devmap` is the name written into the host config,
	// which the *host* resolves later on its own PATH. `runnable` is the
	// program this process is willing to spawn, and PATH discovery there
	// refuses any candidate whose canonical location is inside the repository.
	// An explicit --devmap-bin stays an operator's choice, used or refused.
	devmap := opts.DevmapBin
	runnable := opts.DevmapBin
	if devmap == "" {
		devmap = "devmap"
		discovered, err := proc.LookPathOutside("devmap", root)
		if err != nil {
			runnable = ""
		} else {
			devmap = discovered
			runnable = discovered
		}
	}

	receipt := &Receipt{Host: host, Mode: string(mode), Files: map[string]string{}}

	// Every host config below is repository content: a clone owns `.mcp.json`,
	// `.cursor/` and `.codex/`, and a tracked symlink there would otherwise
	// aim a read or a write outside the checkout. Reads and writes go through
	// this root and the package's own no-follow helpers, which is the
	// containment `integrate uninstall` has always used.
	repo, err := os.OpenRoot(root)
	if err != nil {
		return receipt, err
	}
	defer func() { _ = repo.Close() }()

	switch host {
	case "cursor":
		if err := integrateCursor(repo, root, selfBin, devmap, mode, receipt); err != nil {
			return receipt, err
		}
	case "claude":
		if err := integrateClaude(repo, root, selfBin, devmap, mode, receipt); err != nil {
			return receipt, err
		}
	case "codex":
		if err := integrateCodex(repo, mode, receipt); err != nil {
			return receipt, err
		}
	default:
		receipt.Notes = append(receipt.Notes, fmt.Sprintf("host %q: Phase 4 writes a stub receipt; full adapter ports follow", host))
	}

	// Compose DevMap assets rather than reimplementing them.
	if mode == ModeApply || mode == ModeDryRun {
		if runnable == "" {
			receipt.Notes = append(receipt.Notes,
				"devmap assets skipped: no devmap outside the repository was found on PATH. "+
					"Repository build outputs are not run implicitly; pass --devmap-bin to choose one.")
			return receipt, nil
		}
		args := []string{"integrate", host}
		if mode == ModeDryRun {
			args = append(args, "--dry-run")
		}
		out, err := runDevmap(runnable, args, root)
		receipt.Spawned = append(receipt.Spawned, "devmap "+strings.Join(args, " "))
		if err != nil {
			receipt.Notes = append(receipt.Notes, fmt.Sprintf("devmap integrate: %v (%s)", err, truncate(out, 400)))
		}
		skillArgs := append([]string{"skills", "install"}, skillInstallArgs(root, host)...)
		if mode == ModeDryRun {
			skillArgs = append(skillArgs, "--dry-run")
		}
		sout, serr := runDevmap(runnable, skillArgs, root)
		receipt.Spawned = append(receipt.Spawned, "devmap "+strings.Join(skillArgs, " "))
		if serr != nil {
			receipt.Notes = append(receipt.Notes, fmt.Sprintf("devmap skills install: %v (%s)", serr, truncate(sout, 400)))
		}
	}
	return receipt, nil
}

// devmapAssetBudget bounds one composed DevMap invocation. A skill install on
// a cold cache is the slow case; anything past this is a hang, not work.
const devmapAssetBudget = 5 * time.Minute

// runDevmap spawns one composed DevMap command under the shared process bound,
// so a child that never exits cannot hold `integrate` open forever.
func runDevmap(binary string, args []string, dir string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), devmapAssetBudget)
	defer cancel()
	cmd := exec.CommandContext(ctx, binary, args...)
	proc.ConfigureGroup(cmd)
	cmd.Dir = dir
	cmd.WaitDelay = 2 * time.Second
	var out []byte
	runErr, timedOut := proc.RunBoundedWithCleanup(ctx, func() error {
		var err error
		out, err = cmd.CombinedOutput()
		return err
	})
	if timedOut {
		// The abandoned goroutine may still be writing `out`; do not read it.
		return "", fmt.Errorf("timed out after %s", devmapAssetBudget)
	}
	return string(out), runErr
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

func integrateCursor(repo *os.Root, root, selfBin, devmap string, mode Mode, receipt *Receipt) error {
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

	if err := planWrite(repo, ".cursor/mcp.json", mustJSON(mcp), mode, receipt); err != nil {
		return err
	}
	if err := planWrite(repo, ".cursor/rules/devcouncil.mdc", []byte(cursorRule), mode, receipt); err != nil {
		return err
	}
	// No .cursor/hooks.json: DevCouncil lifecycle hooks are retired, and an
	// empty stub is still a hook config the host parses and dispatches from.
	// `devcouncil integrate uninstall --target hooks` removes any left over.
	receipt.Notes = append(receipt.Notes, "hooks not written: DevCouncil host hooks are retired (MCP-first)")
	return nil
}

func integrateClaude(repo *os.Root, root, selfBin, devmap string, mode Mode, receipt *Receipt) error {
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
	return planWrite(repo, ".mcp.json", mustJSON(mcp), mode, receipt)
}

func integrateCodex(repo *os.Root, mode Mode, receipt *Receipt) error {
	body := []byte("# Managed by devcouncil integrate codex\n")
	return planWrite(repo, ".codex/config.toml", body, mode, receipt)
}

// planWrite inspects and rewrites one host config named *relative to the
// repository root*.
//
// `rel` is never joined onto an absolute path here. It is resolved inside
// `repo` by readHookFile and writeRooted, which refuse a symlink at any
// component and bound the read at maxHostConfigBytes. Both are the helpers
// `integrate uninstall` uses: repository content chooses these filenames, so
// the read must not follow a link out of the checkout (which would copy an
// outside file's contents into the merged result) and the write must not
// land outside it.
func planWrite(repo *os.Root, rel string, content []byte, mode Mode, receipt *Receipt) error {
	existing, _, err := readHookFile(repo, rel)
	exists := err == nil
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("%s: %w", rel, err)
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
		if dir := path.Dir(rel); dir != "." {
			if err := repo.MkdirAll(dir, 0o755); err != nil {
				return fmt.Errorf("%s: %w", rel, err)
			}
		}
		// Refuse JSON-with-comments for mcp.json: encoding/json cannot round-trip it.
		if strings.HasSuffix(rel, "mcp.json") && exists {
			if err := refuseJSONComments(existing, rel); err != nil {
				return err
			}
			merged, err := mergeMCPJSON(existing, content)
			if err != nil {
				return err
			}
			content = merged
		}
		if err := writeRooted(repo, rel, content, 0o644); err != nil {
			return fmt.Errorf("%s: %w", rel, err)
		}
		receipt.Files[rel] = "wrote"
		return nil
	}
	return fmt.Errorf("unknown mode %q", mode)
}

// writeRooted publishes `content` at `rel` inside `repo` atomically, through
// the same exclusive no-follow create the uninstall path uses. The rename is
// root-relative, so neither the temporary nor the final name can be redirected
// by a link planted between the check and the write.
func writeRooted(repo *os.Root, rel string, content []byte, perm os.FileMode) error {
	tmp := rel + ".devcouncil-tmp-" + rand.Text()
	if err := writeHookExclusive(repo, tmp, content, perm); err != nil {
		return err
	}
	if err := repo.Rename(tmp, rel); err != nil {
		return errors.Join(err, repo.Remove(tmp))
	}
	return nil
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
