package integrate

import (
	"bytes"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

type Target string

const TargetHooks Target = "hooks"

var UninstallTargets = []Target{TargetHooks}

// UninstallOptions scopes cleanup to one root and optionally one client.
// Check and dry-run are read-only. Apply backs up every changed file.
type UninstallOptions struct {
	Root   string
	Target Target
	Mode   Mode
	Client string
}

const maxHostConfigBytes = 1 << 20
const hookCleanupLock = ".devcouncil-hook-cleanup.lock"
const openCodeHookPlugin = ".devcouncil/integrations/opencode_devcouncil_plugin.mjs"

type hookConfig struct {
	client, path string
	removeEmpty  bool
	root         string
}

var hookConfigs = []hookConfig{
	{"cursor", ".cursor/hooks.json", true, ""},
	{"claude", ".claude/settings.json", false, ""},
	{"claude", ".claude/settings.local.json", false, ""},
	{"codex", ".codex/hooks.json", true, ""},
	{"gemini", ".gemini/settings.json", false, ""},
	{"grok", ".grok/hooks/devcouncil.json", true, ""},
	{"opencode", "opencode.json", false, ""},
	{"opencode", openCodeHookPlugin, true, ""},
}

type hookEdit struct {
	path          string
	before, after []byte // nil after means remove
	info          os.FileInfo
}

// Uninstall plans the complete scope before changing host files. Unknown or
// unreadable input is an error, never a clean receipt. Existing permissions and
// unrelated settings survive. A lock serializes our own writers; identity and
// byte checks detect intervening edits from other programs before each commit.
func Uninstall(opts UninstallOptions) (receipt *Receipt, resultErr error) {
	if opts.Target != "" && opts.Target != TargetHooks {
		return nil, fmt.Errorf("unknown uninstall target %q (known: hooks)", opts.Target)
	}
	mode := opts.Mode
	if mode == "" {
		mode = ModeApply
	}
	if mode != ModeApply && mode != ModeCheck && mode != ModeDryRun {
		return nil, fmt.Errorf("unknown uninstall mode %q", mode)
	}
	client := strings.ToLower(opts.Client)
	if client != "" && client != "all" {
		known := false
		for _, spec := range hookConfigs {
			known = known || spec.client == client
		}
		if !known {
			return nil, fmt.Errorf("unsupported hook client %q (claude, cursor, codex, gemini, grok, opencode, all)", client)
		}
	}
	if strings.TrimSpace(opts.Root) == "" {
		return nil, errors.New("hook cleanup requires a project root")
	}
	rootPath, err := filepath.Abs(opts.Root)
	if err != nil {
		return nil, err
	}
	root, err := os.OpenRoot(rootPath)
	if err != nil {
		return nil, err
	}
	defer func() { resultErr = errors.Join(resultErr, root.Close()) }()
	receipt = &Receipt{Host: "uninstall:hooks", Mode: string(mode), Files: map[string]string{}}
	if mode == ModeApply {
		lock, err := root.OpenFile(hookCleanupLock, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
		if err != nil {
			return receipt, fmt.Errorf("hook cleanup lock: %w; another cleanup may be active; inspect %s before removing a stale lock", err, filepath.Join(rootPath, hookCleanupLock))
		}
		defer func() { resultErr = errors.Join(resultErr, root.Remove(hookCleanupLock)) }()
		if err := lock.Close(); err != nil {
			return receipt, err
		}
	}
	var edits []hookEdit
	for _, spec := range hookConfigs {
		if client != "" && client != "all" && spec.client != client {
			continue
		}
		spec.root = rootPath
		data, info, err := readHookFile(root, spec.path)
		if os.IsNotExist(err) {
			receipt.Files[spec.path] = "missing"
			continue
		}
		if err != nil {
			receipt.Files[spec.path] = "refused"
			return receipt, fmt.Errorf("%s: %w", spec.path, err)
		}
		var after []byte
		var removed int
		if spec.path == openCodeHookPlugin {
			// The exact generated filename plus two source markers establish ownership.
			if bytes.Contains(data, []byte("export const DevCouncilOpenCodeHook")) && bytes.Contains(data, []byte(`spawnSync("devcouncil", args`)) {
				removed = 1
			}
		} else {
			after, removed, err = cleanHookJSON(data, spec)
			if err != nil {
				receipt.Files[spec.path] = "refused"
				return receipt, fmt.Errorf("%s: refusing to rewrite: %w", spec.path, err)
			}
		}
		if removed == 0 {
			receipt.Files[spec.path] = "unchanged"
			continue
		}
		if receipt.HookEntries == nil {
			receipt.HookEntries = map[string]int{}
		}
		receipt.HookEntries[spec.path] = removed
		receipt.Files[spec.path] = "would_clean"
		if after == nil {
			receipt.Files[spec.path] = "would_remove"
		}
		edits = append(edits, hookEdit{spec.path, data, after, info})
	}
	receipt.Notes = append(receipt.Notes,
		"DevCouncil lifecycle hooks are retired. Legacy event commands are compatibility no-ops; MCP policy and host permissions are unchanged.",
		"Scope is the selected root and clients only; global settings, plugin caches and Git map-refresh hooks outside these paths are not scanned.",
		"Reload the host session after cleanup to stop dispatching cached hook registrations.")
	if mode != ModeApply {
		return receipt, nil
	}
	// Revalidate the full plan before backups or writes; then check again before
	// each rename. Multi-file cleanup is not a filesystem transaction: a later
	// I/O failure returns the partial receipt and the backups needed to recover.
	for _, edit := range edits {
		if err := checkHookEdit(root, edit); err != nil {
			return receipt, err
		}
	}
	for _, edit := range edits {
		backup := edit.path + ".devcouncil-backup-" + rand.Text()
		if err := writeHookExclusive(root, backup, edit.before, edit.info.Mode().Perm()); err != nil {
			return receipt, err
		}
		if receipt.Backups == nil {
			receipt.Backups = map[string]string{}
		}
		receipt.Backups[edit.path] = backup
	}
	for _, edit := range edits {
		if err := applyHookEdit(root, edit); err != nil {
			receipt.Files[edit.path] = "refused"
			return receipt, err
		}
		receipt.Files[edit.path] = "cleaned"
		if edit.after == nil {
			receipt.Files[edit.path] = "removed"
		}
	}
	return receipt, nil
}

func readHookFile(root *os.Root, path string) ([]byte, os.FileInfo, error) {
	// Reject both final and ancestor symlinks. Root also prevents escape if a
	// component is swapped after inspection. Only regular files may be read.
	parts := strings.Split(filepath.ToSlash(path), "/")
	for i := range parts {
		info, err := root.Lstat(filepath.Join(parts[:i+1]...))
		if err != nil {
			return nil, nil, err
		}
		if info.Mode()&os.ModeSymlink != 0 {
			return nil, nil, errors.New("symbolic links are not rewritten")
		}
		if i < len(parts)-1 && !info.IsDir() {
			return nil, nil, errors.New("config parent is not a directory")
		}
		if i == len(parts)-1 && !info.Mode().IsRegular() {
			return nil, nil, errors.New("config is not a regular file")
		}
	}
	f, err := safefile.OpenNoFollow(root, path, os.O_RDONLY, 0)
	if err != nil {
		return nil, nil, err
	}
	info, err := f.Stat()
	if err != nil {
		return nil, nil, errors.Join(err, f.Close())
	}
	if !info.Mode().IsRegular() {
		return nil, nil, errors.Join(errors.New("config is not a regular file"), f.Close())
	}
	if info.Size() > maxHostConfigBytes {
		return nil, nil, errors.Join(fmt.Errorf("config exceeds %d-byte bound", maxHostConfigBytes), f.Close())
	}
	data, err := io.ReadAll(io.LimitReader(f, maxHostConfigBytes+1))
	err = errors.Join(err, f.Close())
	if err != nil {
		return nil, nil, err
	}
	if len(data) > maxHostConfigBytes {
		return nil, nil, fmt.Errorf("config exceeds %d-byte bound", maxHostConfigBytes)
	}
	return data, info, nil
}

func checkHookEdit(root *os.Root, edit hookEdit) error {
	data, info, err := readHookFile(root, edit.path)
	if err != nil {
		return fmt.Errorf("%s changed during cleanup: %w", edit.path, err)
	}
	if !os.SameFile(info, edit.info) || !bytes.Equal(data, edit.before) || info.Mode() != edit.info.Mode() {
		return fmt.Errorf("%s changed during cleanup; retry after the other writer finishes", edit.path)
	}
	return nil
}

func writeHookExclusive(root *os.Root, path string, data []byte, perm os.FileMode) error {
	f, err := safefile.OpenNoFollow(root, path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, perm)
	if err != nil {
		return err
	}
	_, err = f.Write(data)
	if err == nil {
		err = f.Chmod(perm)
	}
	if err == nil {
		err = f.Sync()
	}
	err = errors.Join(err, f.Close())
	if err != nil {
		return errors.Join(err, root.Remove(path))
	}
	return nil
}

func applyHookEdit(root *os.Root, edit hookEdit) (err error) {
	if edit.after == nil {
		if err := checkHookEdit(root, edit); err != nil {
			return err
		}
		return root.Remove(edit.path)
	}
	tmp := edit.path + ".devcouncil-tmp-" + rand.Text()
	if err := writeHookExclusive(root, tmp, edit.after, edit.info.Mode().Perm()); err != nil {
		return err
	}
	defer func() {
		if e := root.Remove(tmp); e != nil && !os.IsNotExist(e) {
			err = errors.Join(err, e)
		}
	}()
	if err := checkHookEdit(root, edit); err != nil {
		return err
	}
	return root.Rename(tmp, edit.path)
}

func cleanHookJSON(data []byte, spec hookConfig) ([]byte, int, error) {
	if err := validateHookJSON(data); err != nil {
		return nil, 0, err
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	var settings map[string]any
	if err := decoder.Decode(&settings); err != nil {
		return nil, 0, err
	}
	if settings == nil {
		return nil, 0, errors.New("expected a JSON object")
	}
	if err := validateHookSettings(settings, spec.client); err != nil {
		return nil, 0, err
	}
	removed := 0
	if spec.client == "opencode" {
		if entries, ok := settings["plugin"].([]any); ok {
			kept := make([]any, 0, len(entries))
			for _, entry := range entries {
				if isOpenCodeHookRef(entry, spec.root) {
					removed++
				} else {
					kept = append(kept, entry)
				}
			}
			if removed > 0 {
				if len(kept) == 0 {
					delete(settings, "plugin")
				} else {
					settings["plugin"] = kept
				}
			}
		} else if isOpenCodeHookRef(settings["plugin"], spec.root) {
			delete(settings, "plugin")
			removed++
		}
	} else if hooks, ok := settings["hooks"].(map[string]any); ok {
		for event, raw := range hooks {
			entries, ok := raw.([]any)
			if !ok {
				continue
			}
			for _, rawEntry := range entries {
				entry, ok := rawEntry.(map[string]any)
				if !ok {
					continue
				}
				if children, ok := entry["hooks"].([]any); ok {
					for _, child := range children {
						if err := checkNamedHook(child); err != nil {
							return nil, 0, err
						}
					}
				} else if err := checkNamedHook(entry); err != nil {
					return nil, 0, err
				}
			}
			kept, n := stripHookEntries(entries)
			removed += n
			if n == 0 {
				continue
			}
			if len(kept) == 0 {
				delete(hooks, event)
			} else {
				hooks[event] = kept
			}
		}
		if removed > 0 && len(hooks) == 0 {
			delete(settings, "hooks")
		}
	}
	// Legacy installs wrote a DevCouncil status line beside the hooks.
	// isDevCouncilHookEntry holds this to our own value, so a host's own
	// statusLine is never touched. Counting it as a removal here, before
	// the no-op guard, keeps a file whose only DevCouncil artifact is the
	// status line from being reported unchanged.
	if isDevCouncilHookEntry(settings["statusLine"]) {
		delete(settings, "statusLine")
		removed++
	}
	if removed == 0 {
		return data, 0, nil
	}
	if spec.removeEmpty && (len(settings) == 0 || (len(settings) == 1 && settings["version"] != nil)) {
		return nil, removed, nil
	}
	after, err := json.MarshalIndent(settings, "", "  ")
	if err != nil {
		return nil, 0, err
	}
	return append(after, '\n'), removed, nil
}

// Validate the portions we inspect; unrelated host fields stay opaque. An
// unsupported shape cannot be reported as a successful inspection with no hooks.
func validateHookSettings(settings map[string]any, client string) error {
	if client == "opencode" {
		raw, exists := settings["plugin"]
		if !exists {
			return nil
		}
		if _, ok := raw.(string); ok {
			return nil
		}
		entries, ok := raw.([]any)
		if !ok {
			return errors.New("plugin must be a string or array of strings")
		}
		for _, entry := range entries {
			if _, ok := entry.(string); !ok {
				return errors.New("plugin entries must be strings")
			}
		}
		return nil
	}
	raw, exists := settings["hooks"]
	if !exists {
		return nil
	}
	hooks, ok := raw.(map[string]any)
	if !ok {
		return errors.New("hooks must be an object")
	}
	for _, raw := range hooks {
		entries, ok := raw.([]any)
		if !ok {
			return errors.New("hook events must contain arrays")
		}
		for _, raw := range entries {
			entry, ok := raw.(map[string]any)
			if !ok {
				return errors.New("hook entries must be objects")
			}
			nested, exists := entry["hooks"]
			if !exists {
				continue
			}
			children, ok := nested.([]any)
			if !ok {
				return errors.New("matcher hooks must contain an array")
			}
			for _, child := range children {
				if _, ok := child.(map[string]any); !ok {
					return errors.New("matcher hook entries must be objects")
				}
			}
		}
	}
	return nil
}

// A known managed name with customized wiring needs manual review, not a
// misleading clean receipt or deletion based on a substring in a shell script.
func checkNamedHook(raw any) error {
	entry, ok := raw.(map[string]any)
	if !ok {
		return nil
	}
	name, _ := entry["name"].(string)
	if strings.HasPrefix(name, "devcouncil-") && !isDevCouncilHookEntry(entry) {
		return errors.New("unrecognized DevCouncil-named hook command; review this entry manually (no changes applied)")
	}
	return nil
}

func stripHookEntries(entries []any) ([]any, int) {
	kept := make([]any, 0, len(entries))
	removed := 0
	for _, raw := range entries {
		entry, ok := raw.(map[string]any)
		if !ok {
			kept = append(kept, raw)
			continue
		}
		if nested, ok := entry["hooks"].([]any); ok {
			// Host schemas have one matcher-group level. Do not recurse into unknown
			// extension data or prune an unrelated, already-empty matcher group.
			children := make([]any, 0, len(nested))
			n := 0
			for _, child := range nested {
				if isDevCouncilHookEntry(child) {
					n++
				} else {
					children = append(children, child)
				}
			}
			removed += n
			if n > 0 {
				if len(children) == 0 {
					continue
				}
				entry["hooks"] = children
			}
		} else if isDevCouncilHookEntry(entry) {
			removed++
			continue
		}
		kept = append(kept, raw)
	}
	return kept, removed
}

func isOpenCodeHookRef(raw any, root string) bool {
	ref, ok := raw.(string)
	if !ok {
		return false
	}
	ref = strings.ReplaceAll(ref, "\\", "/")
	if strings.HasPrefix(ref, "file:") {
		parsed, err := url.Parse(ref)
		if err != nil || (parsed.Host != "" && parsed.Host != "localhost") || parsed.RawQuery != "" || parsed.Fragment != "" {
			return false
		}
		ref = parsed.Path
		// Windows file URIs have a leading slash before the drive letter.
		if len(ref) > 3 && ref[0] == '/' && ref[2] == ':' {
			ref = ref[1:]
		}
	}
	clean := filepath.ToSlash(filepath.Clean(ref))
	if clean == openCodeHookPlugin {
		return true
	}
	return root != "" && clean == filepath.ToSlash(filepath.Join(root, openCodeHookPlugin))
}

func isDevCouncilHookEntry(raw any) bool {
	entry, ok := raw.(map[string]any)
	if !ok {
		return false
	}
	if kind, ok := entry["type"].(string); ok && kind != "command" {
		return false
	}
	command, ok := entry["command"].(string)
	if !ok {
		return false
	}
	words, ok := simpleHookWords(command)
	if !ok || len(words) == 0 {
		return false
	}
	executable := strings.ToLower(filepath.Base(strings.ReplaceAll(words[0], "\\", "/")))
	if executable != "dev" && executable != "devcouncil" && executable != "dev.exe" && executable != "devcouncil.exe" {
		return false
	}
	if len(words) > 1 {
		return words[1] == "hook"
	}
	args, ok := entry["args"].([]any)
	if !ok || len(args) == 0 {
		return false
	}
	return args[0] == "hook"
}

// Parse only a simple direct invocation, including shell-quoted paths. Refuse
// compound commands, substitutions and redirects rather than delete a foreign
// command that merely mentions DevCouncil. This parser never executes input.
func simpleHookWords(command string) ([]string, bool) {
	var words []string
	var word strings.Builder
	quote := byte(0)
	started := false
	for i := 0; i < len(command); i++ {
		c := command[i]
		if c == 0 || c == '\n' || c == '\r' {
			return nil, false
		}
		if quote != 0 {
			if c == quote {
				quote = 0
			} else {
				if quote == '"' && (c == '$' || c == '`') {
					return nil, false
				}
				word.WriteByte(c)
			}
			continue
		}
		switch c {
		case '\'', '"':
			quote = c
			started = true
		case ';', '&', '|', '<', '>', '`', '$', '(', ')':
			return nil, false
		case ' ', '\t':
			if started {
				words = append(words, word.String())
				word.Reset()
				started = false
			}
		default:
			word.WriteByte(c)
			started = true
		}
	}
	if quote != 0 {
		return nil, false
	}
	if started {
		words = append(words, word.String())
	}
	return words, true
}

// Reject duplicate keys and excessive nesting before the normal decoder can
// silently overwrite data. UseNumber also preserves integers beyond 2^53.
func validateHookJSON(data []byte) error {
	d := json.NewDecoder(bytes.NewReader(data))
	d.UseNumber()
	if err := validateHookValue(d, 0); err != nil {
		return err
	}
	if _, err := d.Token(); err != io.EOF {
		if err == nil {
			return errors.New("multiple JSON values")
		}
		return err
	}
	return nil
}
func validateHookValue(d *json.Decoder, depth int) error {
	if depth > 64 {
		return errors.New("JSON nesting exceeds 64 levels")
	}
	token, err := d.Token()
	if err != nil {
		return err
	}
	delim, ok := token.(json.Delim)
	if !ok {
		return nil
	}
	switch delim {
	case '{':
		keys := map[string]bool{}
		for d.More() {
			token, err := d.Token()
			if err != nil {
				return err
			}
			key, ok := token.(string)
			if !ok {
				return errors.New("non-string JSON key")
			}
			if keys[key] {
				return errors.New("duplicate JSON key")
			}
			keys[key] = true
			if err := validateHookValue(d, depth+1); err != nil {
				return err
			}
		}
	case '[':
		for d.More() {
			if err := validateHookValue(d, depth+1); err != nil {
				return err
			}
		}
	default:
		return errors.New("unexpected JSON delimiter")
	}
	_, err = d.Token()
	return err
}
