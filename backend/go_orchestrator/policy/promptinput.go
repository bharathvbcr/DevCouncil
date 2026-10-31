package policy

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/fnmatch"
)

// PromptInputPatterns name the files whose contents a later turn reads as
// instructions or executes as configuration: the project instruction files an
// agent loads into its next prompt (AGENTS.md, CLAUDE.md, GEMINI.md) and the MCP
// server declarations whose commands the harness spawns (mcp.json, .mcp.json).
// An agent that can write them can write its own next system prompt, or the
// command a later turn runs, from inside a task scoped to something else.
//
// Not part of SecretPathPatterns, and the difference is the design. That list
// also refuses reads (EvaluateRead, ClassifyRead, the command and diff read
// rungs), and these files must stay readable: they are how an agent learns the
// project's rules. Not part of RestrictedPathPatterns either: that rung sits in
// the hard-rules block, which the yolo posture switches off. This set is
// write-only and is judged by EvaluateFileChange outside that block, so it holds
// under every posture. It reports as RuleRestrictedPath: the rule set is a
// contract shared with other repositories, and "a protected repository path" is
// what these are.
//
// What holds under yolo, precisely: every write the gate models. That is a
// write tool, a shell redirection, and the arguments of the writers the command
// gate reads (tee, sed -i, cp, mv, rm, install, ln, rsync, dd, patch, perl -i,
// truncate, touch, git mv/rm/restore/checkout with a path). It also holds for a
// path that reaches a prompt input by another name: a symlink or a directory
// link, a case or Unicode spelling, a Windows trailing-dot, trailing-space or
// ::$DATA spelling, a hard link to a root-level prompt input, and the target of
// a prompt input that is itself a symlink. What yolo does NOT hold: a program
// the command gate does not model (an interpreter, `sh -c`, tar, unzip, a git
// subcommand that rewrites the tree such as reset --hard, merge or am). The
// strict and dev postures add the unmodelled-program operand rung, which refuses
// such a program handed one of these paths. Hard-linking a nested prompt input
// and writing through the link elsewhere is a known gap.
//
// The Internal argument of EvaluateFileChange exempts the host's own writes. It
// is a host-trust assertion, not a property of the path: whatever sets it must
// never derive it, or the path it applies to, from model output.
//
// Entries are file names, not globs, and match the last path component at any
// depth, case-folded. Any depth because the tools that load these files do not
// stop at the root: Manvi reads the working directory's file, which is a
// subdirectory when the agent is launched in one, Claude Code, Codex and Gemini
// CLI all load nested instruction files, and plugin discovery reads any mcp.json
// under plugins/. The cost is that an agent cannot edit a nested fixture of the
// same name; a person can.
var PromptInputPatterns = []string{
	"AGENTS.md",
	"CLAUDE.md",
	"GEMINI.md",
	"mcp.json",
	".mcp.json",
}

// PromptInputDirs are the skill roots: the directories whose files a harness
// serves to the model as instructions on request (a skill body), so a write there
// is a write to a later prompt just as one to AGENTS.md is. They are the skill
// layout roots DevCouncil's skills scaffold installs into (skills/scaffold.go
// Options.Destinations), matched at any depth as the names above are, as a run of
// path components.
//
// Deliberately only the skill roots, not whole agent-config trees such as
// .claude/** or .agents/**: the yolo posture is specified to leave
// .claude/settings.json writable (gate_test.go
// TestYoloPostureAlsoDisablesHardRules), and widening this list to settings and
// hooks overturns that contract, which is the owner's call, not a side effect.
var PromptInputDirs = []string{".claude/skills", ".cursor/skills", ".agents/skills"}

// PromptInputManifestRoots and PromptInputManifestNames are the plugin
// declarations Manvi discovers (manvi/mcp/plugin.go DiscoverPlugins, called with
// .devcouncil/plugins, .mcp/plugins and plugins): a plugin.json or
// openplugin.json anywhere under one of those roots names a command the harness
// spawns, exactly as mcp.json does. (mcp.json itself is in PromptInputPatterns.)
// Root-anchored, as the discovery is. .devcouncil/plugins is also covered by
// RestrictedPathPatterns, which stops running when the hard rules do.
var (
	PromptInputManifestRoots = []string{"plugins", ".mcp/plugins", ".devcouncil/plugins"}
	PromptInputManifestNames = []string{"plugin.json", "openplugin.json"}
)

// The folded forms are computed once: IsPromptInput runs several times per
// write evaluation, and folding allocates.
var (
	promptNamesFolded         = foldAllPrompt(PromptInputPatterns)
	promptDirsFolded          = foldAllPrompt(PromptInputDirs)
	promptManifestRootsFolded = foldAllPrompt(PromptInputManifestRoots)
	promptManifestNamesFolded = foldAllPrompt(PromptInputManifestNames)
)

func foldAllPrompt(list []string) []string {
	out := make([]string, len(list))
	for i, s := range list {
		out[i] = fnmatch.Fold(s)
	}
	return out
}

// foldPrompt is fnmatch.Fold with an allocation-free path for ASCII, which is
// nearly every path asked about.
func foldPrompt(s string) string {
	if !isASCII(s) {
		return fnmatch.Fold(s)
	}
	return strings.ToLower(s)
}

func isASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] >= 0x80 {
			return false
		}
	}
	return true
}

// IsPromptInput reports whether a repository-relative path is a prompt input:
// one of PromptInputPatterns at any depth, a path inside (or naming) one of
// PromptInputDirs at any depth, or a plugin manifest under a
// PromptInputManifestRoots entry. Case-folded, and tolerant of the Windows
// spellings that name the same file (see winFold).
func IsPromptInput(path string) bool {
	cleaned := strings.TrimPrefix(normalizeSlashes(strings.TrimSpace(path)), "./")
	folded := winFold(foldPrompt(cleaned))
	base := folded[strings.LastIndexByte(folded, '/')+1:]
	for _, name := range promptNamesFolded {
		if base == name {
			return true
		}
	}
	for _, dir := range promptDirsFolded {
		if containsComponents(folded, dir) {
			return true
		}
	}
	for _, root := range promptManifestRootsFolded {
		if len(folded) > len(root) && folded[len(root)] == '/' && strings.HasPrefix(folded, root) {
			for _, name := range promptManifestNamesFolded {
				if base == name {
					return true
				}
			}
		}
	}
	return false
}

// containsComponents reports whether run, itself a "/"-joined list of
// components, occurs in path as a whole run of its components.
func containsComponents(path, run string) bool {
	for from := 0; from < len(path); {
		i := strings.Index(path[from:], run)
		if i < 0 {
			return false
		}
		i += from
		end := i + len(run)
		if (i == 0 || path[i-1] == '/') && (end == len(path) || path[end] == '/') {
			return true
		}
		from = i + 1
	}
	return false
}

// winFold maps the spellings Windows resolves to the same file onto the plain
// one: a component's trailing dots and spaces are dropped by the Win32 path
// layer, and "name::$DATA" is the file's default stream. A path with neither is
// returned as it came, without allocating. Applied to a folded path.
func winFold(p string) string {
	if !strings.ContainsAny(p, ":") && !strings.HasSuffix(p, ".") && !strings.HasSuffix(p, " ") &&
		!strings.Contains(p, "./") && !strings.Contains(p, " /") {
		return p
	}
	parts := strings.Split(p, "/")
	for i, part := range parts {
		parts[i] = winComponent(part)
	}
	return strings.Join(parts, "/")
}

func winComponent(part string) string {
	for i := 0; i < 4; i++ {
		next := strings.TrimSuffix(strings.TrimSuffix(part, "::$data"), ":$data")
		if trimmed := strings.TrimRight(next, ". "); trimmed != "" {
			next = trimmed
		}
		if next == part {
			return part
		}
		part = next
	}
	return part
}

// lexicalRepoRel is raw's repository-relative spelling before any symbolic link
// is followed, and whether it names a place inside the root at all. NormalizeRepoPath
// answers for the file the kernel would open; a prompt input is also whatever is
// *called* AGENTS.md, because a repository may keep its instruction file elsewhere
// and link it in, and the clients load it by that name.
func lexicalRepoRel(root, raw string) (string, bool) {
	cleaned, settled := settlePath(normalizeSlashes(raw))
	if !settled {
		return "", false
	}
	if filepath.IsAbs(cleaned) {
		rel, err := filepath.Rel(filepath.Clean(root), cleaned)
		if err != nil || rel == ".." || strings.HasPrefix(rel, "../") {
			resolved, rerr := filepath.EvalSymlinks(root)
			if rerr != nil {
				return "", false
			}
			if rel, err = filepath.Rel(resolved, cleaned); err != nil {
				return "", false
			}
		}
		cleaned = filepath.ToSlash(rel)
	}
	cleaned = strings.TrimPrefix(cleaned, "./")
	if cleaned == "." || cleaned == ".." || strings.HasPrefix(cleaned, "../") {
		return "", false
	}
	return cleaned, true
}

// promptInputReason says why a write to path is a write to a prompt input, or ""
// when it is not. The questions, cheapest first:
//
//   - the spelling the caller used, lexically (before any link is followed);
//   - the file the kernel would open (normalized), and the name the filesystem
//     stores it under, for a path that exists (disk);
//   - whether the existing file is the same file as a root-level prompt input.
//
// The last two only make sense for a path inside the root: with the hard rules
// off the operator has asked for escapes, and a file of the same name out there
// is not this repository's prompt input.
func (g FileGate) promptInputReason(path, normalized string, outside bool, disk func() (string, bool)) string {
	const direct = "Project instruction, skill and MCP declaration files cannot be modified by an agent: " +
		"they become the next turn's prompt or the commands it runs."
	if lexical, ok := lexicalRepoRel(g.Root, path); ok && IsPromptInput(lexical) {
		return direct
	}
	if outside {
		return ""
	}
	if IsPromptInput(normalized) {
		return direct
	}
	// The stored spelling matters only for a name the text fold could miss, and an
	// ASCII path differs from what the filesystem stores only by case, which the
	// fold already equates. Skipping the directory walk for it keeps the common
	// write (and every yolo write, which would otherwise be the walk's only
	// caller) at its old price. The identity check below does not depend on
	// spelling at all.
	if !isASCII(normalized) {
		if stored, differs := disk(); differs && IsPromptInput(stored) {
			return "Path is another name for `" + stored + "`, a project instruction, skill or MCP declaration file, " +
				"which an agent cannot modify."
		}
	}
	if reachesRootPromptInput(g.Root, normalized) {
		return "Path is the same file as a root-level project instruction or MCP declaration file " +
			"(a hard link to it, or what it links to), which an agent cannot modify."
	}
	return ""
}

// lexicalPromptInput is IsPromptInput of a command operand's lexical spelling.
func lexicalPromptInput(root, operand string) bool {
	if root == "" {
		return IsPromptInput(operand)
	}
	lexical, ok := lexicalRepoRel(root, operand)
	return ok && IsPromptInput(lexical)
}

// reachesRootPromptInput reports whether the existing file at normalized (a
// resolved repository-relative path) is the very file one of the root-level
// prompt inputs is: a hard link to it, or the target of one that is itself a
// symbolic link. Identity, not spelling, so no name the agent chooses changes
// the answer. Nested prompt inputs are not compared (that would cost a tree
// walk per write): a hard link to docs/AGENTS.md written through elsewhere is
// the documented gap.
func reachesRootPromptInput(root, normalized string) bool {
	target, err := os.Stat(filepath.Join(root, filepath.FromSlash(normalized)))
	if err != nil || !target.Mode().IsRegular() {
		return false
	}
	for _, name := range PromptInputPatterns {
		if input, err := os.Stat(filepath.Join(root, name)); err == nil && os.SameFile(target, input) {
			return true
		}
	}
	return false
}
