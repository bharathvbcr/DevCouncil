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
	"slices"
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

// Hosts this command will configure.
//
// Matches the Rust `integrate::Host` set exactly. Two lists that disagree is
// how `devcouncil integrate <host> --apply` came to succeed for a name
// `devmap integrate <host>` then refused.
var Hosts = []string{
	"cursor", "claude", "codex", "opencode", "warp", "antigravity",
}

// Names this command used to accept, and what to do instead.
//
// Kept as data rather than deleted outright: a user who has run
// `integrate gemini` before deserves to be told where it went, not that their
// spelling is invalid. Removing a host from *installation* also says nothing
// about *cleanup* — `Uninstall` still knows `.gemini/settings.json`, because a
// registration already on disk must stay removable after the adapter that
// wrote it is gone.
var retiredHosts = map[string]string{
	"gemini": "Gemini CLI was replaced upstream by Antigravity; " +
		"use `integrate antigravity`. Existing `.gemini/settings.json` hooks are " +
		"still removed by `dev hook disable --client gemini`.",
	"aider": "Aider exposes no MCP server, so there is nothing for `integrate` " +
		"to write. Run Aider against the repository directly; it reaches the " +
		"index through the `devmap` CLI rather than MCP.",
}

// Options for Integrate.
type Options struct {
	Root      string
	Host      string
	Mode      Mode
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

DevCouncil host hooks are retired, so
**interactive Shell and Write need no task lease**. Do not claim "Shell is gated", and do not check out a task in order to
run a command. Use the ` + "`devcouncil_*`" + ` MCP tools when you actually want task state,
scope or verification — and prefer them over guessing it. A host that uses MCP policy
must honor its results. The ` + "`--write-gate`" + ` flag has been removed rather than
kept as a refusal, and ` + "`integrations.<host>.write_gate`" + ` is no longer read: both
named a pre-tool-use write gate that only the retired hooks ever installed. A copy
left in an existing ` + "`config.yaml`" + ` is inert.

Engineering skills live under ` + "`.cursor/skills/`" + ` and ` + "`.claude/skills/`" + ` (` + "`devcouncil skills scaffold`" + `).
`

// Run configures one host.
func Run(opts Options) (*Receipt, error) {
	root, err := filepath.Abs(opts.Root)
	if err != nil {
		return nil, err
	}
	host := strings.ToLower(opts.Host)
	// Validate before anything is written or spawned. Until this existed the
	// unknown-host path fell through to a note on an otherwise ordinary
	// receipt, so `integrate banana --apply` exited 0, reported success, and
	// spawned `devmap integrate banana` on the way. A host this command cannot
	// configure is a refusal, not a receipt.
	if err := checkHost(host); err != nil {
		return nil, err
	}
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
		// Every remaining name in `Hosts` writes a server document. An
		// unsupported one never reaches here — `checkHost` refused it before
		// anything was opened — so a stub receipt would now be unreachable
		// rather than merely unhelpful.
		doc, known := hostMcpDocs[host]
		if !known {
			return receipt, fmt.Errorf("host %q is advertised but has no adapter", host)
		}
		if err := integrateServerDoc(repo, doc, root, mode, receipt); err != nil {
			return receipt, err
		}
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

// checkHost accepts only a host with an adapter, and explains the rest.
func checkHost(host string) error {
	if host == "" {
		return fmt.Errorf("no host given; expected one of: %s", strings.Join(Hosts, ", "))
	}
	if slices.Contains(Hosts, host) {
		return nil
	}
	if reason, retired := retiredHosts[host]; retired {
		return fmt.Errorf("host %q is no longer configured here: %s", host, reason)
	}
	return fmt.Errorf("unsupported host %q; expected one of: %s", host, strings.Join(Hosts, ", "))
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

	if err := planWrite(repo, ".cursor/mcp.json", mustJSON(mcp), mode, receipt, mergeMCPJSON); err != nil {
		return err
	}
	if err := planWrite(repo, ".cursor/rules/devcouncil.mdc", []byte(cursorRule), mode, receipt, nil); err != nil {
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
	return planWrite(repo, ".mcp.json", mustJSON(mcp), mode, receipt, mergeMCPJSON)
}

func integrateCodex(repo *os.Root, mode Mode, receipt *Receipt) error {
	body := []byte("# Managed by devcouncil integrate codex\n")
	return planWrite(repo, ".codex/config.toml", body, mode, receipt, nil)
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
// hostMcpDoc says where one host keeps its server list and how that document
// is shaped.
//
// Three hosts, three shapes: Antigravity nests servers under `mcpServers`,
// OpenCode under `mcp`, and Warp's file *is* the server map. Declaring the
// shape once is what lets these share `planWrite` instead of becoming three
// near-copies. The Rust integrator holds the same table for the `devmap`
// entry it owns in these files; `TestHostDocumentsMatchTheRustIntegrator`
// pins the two together.
type hostMcpDoc struct {
	rel string
	// container is the key the server map lives under. Empty means the
	// document root is the map itself.
	container string
	// preamble is established when the file is created and never overwritten.
	preamble map[string]any
	// argvForm says entries name the program as one argv array rather than a
	// command plus a separate args list.
	argvForm bool
}

// hostMcpDocs are the hosts whose server document this package writes.
//
// Cursor and Claude are absent on purpose: their files are handled by the
// adapters above, which also write a rule and pick up `.claude/mcp.json`.
var hostMcpDocs = map[string]hostMcpDoc{
	"antigravity": {rel: ".agents/mcp_config.json", container: "mcpServers"},
	"opencode": {
		rel:       "opencode.json",
		container: "mcp",
		// OpenCode validates against this schema. A file created without it
		// loses editor completion for every other key in the user's config.
		preamble: map[string]any{"$schema": "https://opencode.ai/config.json"},
		argvForm: true,
	},
	"warp": {rel: ".devcouncil/integrations/warp-mcp.json"},
}

// integrateServerDoc registers the DevCouncil MCP server in one host's project
// document.
//
// Every other key survives. `opencode.json` sits at the repository root and is
// a *user's* configuration that happens to hold a server list — losing an
// unrelated key there is a bug even when the server entry comes out right.
func integrateServerDoc(repo *os.Root, doc hostMcpDoc, root string, mode Mode, receipt *Receipt) error {
	generated := map[string]any{}
	for key, value := range doc.preamble {
		generated[key] = value
	}
	entry := devcouncilEntry(doc, root)
	if doc.container == "" {
		generated[mcpServerName] = entry
	} else {
		generated[doc.container] = map[string]any{mcpServerName: entry}
	}
	return planWrite(repo, doc.rel, mustJSON(generated), mode, receipt, mergeServerMap(doc))
}

// mcpServerName is the key DevCouncil's own server is registered under.
const mcpServerName = "devcouncil"

// devcouncilEntry is the server entry in this host's spelling.
func devcouncilEntry(doc hostMcpDoc, root string) map[string]any {
	if doc.argvForm {
		return map[string]any{
			"type":        "local",
			"command":     []any{"devcouncil", "mcp-server"},
			"environment": map[string]any{"DEVCOUNCIL_PROJECT_ROOT": root},
			"enabled":     true,
			"timeout":     10000,
		}
	}
	entry := map[string]any{
		"command": "devcouncil",
		"args":    []any{"mcp-server"},
		"env":     map[string]any{"DEVCOUNCIL_PROJECT_ROOT": root},
	}
	if doc.container == "mcpServers" {
		// Antigravity resolves relative paths against its own working
		// directory, not the project's.
		entry["cwd"] = root
	}
	return entry
}

// mergeServerMap folds our entry into the host's document, container and all.
func mergeServerMap(doc hostMcpDoc) mergeFunc {
	return func(existing, generated []byte) ([]byte, error) {
		var cur, gen map[string]any
		if err := json.Unmarshal(existing, &cur); err != nil {
			// Refuse rather than replace: an unparseable config is far more
			// likely to be a file worth keeping than one worth overwriting.
			return nil, fmt.Errorf("not JSON (%w); refuse to overwrite a host config this command cannot read", err)
		}
		if err := json.Unmarshal(generated, &gen); err != nil {
			return nil, err
		}
		if cur == nil {
			cur = map[string]any{}
		}
		for key, value := range gen {
			if key == doc.container {
				continue
			}
			// Preamble keys are established, never overwritten: the user may
			// have pinned a different schema revision on purpose.
			if _, present := cur[key]; !present {
				cur[key] = value
			}
		}
		if doc.container == "" {
			cur[mcpServerName] = gen[mcpServerName]
			return mustJSON(cur), nil
		}
		servers, ok := cur[doc.container].(map[string]any)
		if !ok {
			if cur[doc.container] != nil {
				return nil, fmt.Errorf("%q is not a JSON object", doc.container)
			}
			servers = map[string]any{}
			cur[doc.container] = servers
		}
		genServers, _ := gen[doc.container].(map[string]any)
		for name, value := range genServers {
			servers[name] = value
		}
		return mustJSON(cur), nil
	}
}

// mergeFunc folds the generated document into what is already on disk.
//
// A nil mergeFunc means the file is ours outright and is replaced.
type mergeFunc func(existing, generated []byte) ([]byte, error)

func planWrite(repo *os.Root, rel string, content []byte, mode Mode, receipt *Receipt, merge mergeFunc) error {
	existing, _, err := readHookFile(repo, rel)
	exists := err == nil
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("%s: %w", rel, err)
	}
	// Resolve what would actually land, once, so check and apply cannot
	// disagree about whether this file needs writing.
	want := content
	if merge != nil && exists {
		// encoding/json cannot round-trip comments, so a config carrying them
		// would silently lose them in the merge.
		if err := refuseJSONComments(existing, rel); err != nil {
			return err
		}
		merged, mergeErr := merge(existing, content)
		if mergeErr != nil {
			return fmt.Errorf("%s: %w", rel, mergeErr)
		}
		want = merged
	}
	switch mode {
	case ModeCheck:
		if !exists {
			receipt.Files[rel] = "missing"
			return nil
		}
		// Compared against the merged result, not the generated document: a
		// file that already holds our entry beside three of someone else's is
		// current, and reporting it as drift would demand a rewrite that
		// changes nothing.
		if string(existing) == string(want) {
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
		if err := writeRooted(repo, rel, want, 0o644); err != nil {
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
