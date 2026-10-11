// Package gatescfg loads and updates DevCouncil verification-gate settings.
//
// `gates.mode` in `.devcouncil/config.yaml` is not a harness flag, so
// flags.LoadConfig skips it. Assist docs say verification is opt-in and
// defaults to off; this package is the owner that actually reads the key.
package gatescfg

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

const (
	ModeOff      = "off"
	ModeAdvisory = "advisory"
	ModeEnforce  = "enforce"

	OriginDefault = "default"
	OriginConfig  = "config"
	OriginInvalid = "invalid"
)

// Snapshot is what `devcouncil gate status` reports.
type Snapshot struct {
	VerificationMode   string `json:"verification_mode"`
	VerificationOrigin string `json:"verification_origin"`
	ConfigPath         string `json:"config_path"`
	ConfigPresent      bool   `json:"config_present"`
	// Error says why the origin is invalid; empty otherwise.
	Error string `json:"error,omitempty"`
}

// Normalize maps config spellings onto off|advisory|enforce.
// Empty, false, 0, none, and off are off. An unknown spelling is a bad config,
// and a bad config fails closed: it resolves to enforce with origin invalid, so
// a typo such as `enforced` cannot switch the gate off.
func Normalize(raw string) (mode, origin string) {
	s := strings.TrimSpace(strings.ToLower(raw))
	switch s {
	case "":
		return ModeOff, OriginDefault
	case "off", "false", "0", "no", "none":
		return ModeOff, OriginConfig
	case "advisory", "warn":
		return ModeAdvisory, OriginConfig
	case "enforce", "true", "1", "yes":
		return ModeEnforce, OriginConfig
	default:
		return ModeEnforce, OriginInvalid
	}
}

// ParseMode accepts only the three canonical names (plus documented aliases).
func ParseMode(raw string) (string, error) {
	s := strings.TrimSpace(strings.ToLower(raw))
	switch s {
	case ModeOff, "false", "0", "no", "none":
		return ModeOff, nil
	case ModeAdvisory, "warn":
		return ModeAdvisory, nil
	case ModeEnforce, "true", "1", "yes":
		return ModeEnforce, nil
	default:
		return "", fmt.Errorf("unknown gate mode %q (want off, advisory, or enforce)", raw)
	}
}

// failClosed marks snap as a bad config: enforce, with the reason.
func failClosed(snap Snapshot, reason string) Snapshot {
	snap.VerificationMode = ModeEnforce
	snap.VerificationOrigin = OriginInvalid
	snap.Error = reason
	return snap
}

// Load reads `.devcouncil/config.yaml` under root, uncached. A missing file is
// off, origin default, and is the only way to reach the default. A file that is
// present but cannot be opened or parsed, or that holds a scalar `gates` or an
// unknown mode, is enforce with origin invalid and the reason in Error.
func Load(root string) Snapshot {
	path := filepath.Join(root, ".devcouncil", "config.yaml")
	snap := Snapshot{
		VerificationMode:   ModeOff,
		VerificationOrigin: OriginDefault,
		ConfigPath:         path,
	}
	f, err := os.Open(path)
	if err != nil {
		if os.IsNotExist(err) {
			return snap
		}
		snap.ConfigPresent = true
		return failClosed(snap, fmt.Sprintf("%s: %v", path, err))
	}
	defer func() { _ = f.Close() }()
	snap.ConfigPresent = true
	values, err := flags.ParseConfig(f)
	if err != nil {
		return failClosed(snap, fmt.Sprintf("%s: %v", path, err))
	}
	if _, scalar := values["gates"]; scalar {
		// `gates: [` and `gates: off` are scalars, not a mapping with mode.
		return failClosed(snap, path+": gates must be a mapping with a mode key, not a scalar")
	}
	snap.VerificationMode, snap.VerificationOrigin = Normalize(values["gates.mode"])
	if snap.VerificationOrigin == OriginInvalid {
		snap.Error = fmt.Sprintf("%s: unknown gate mode %q (want off, advisory, or enforce)", path, values["gates.mode"])
	}
	// Neither `execution.hook_gate.mode` nor `integrations.<host>.write_gate`
	// is read. Both named pre-tool-use containment that only DevCouncil's
	// retired lifecycle hooks ever installed, and `contain` was a mode that
	// contained nothing — a status line reading `hook_gate: contain` or
	// `write_gate: true` reported enforcement that did not exist, which is
	// worse than reporting none. Copies left in an existing config.yaml are
	// inert and need no migration.
	//
	// Detail on the removed keys. It named a
	// pre-tool-use write gate that DevCouncil's retired lifecycle hooks used to
	// install; nothing enforces it now. Publishing it in this snapshot let
	// `gate status` print a knob that did nothing, which is the shape of a
	// setting that reads as enforcement without being any. A key left in an
	// existing config.yaml is inert and needs no migration.
	return snap
}

// fileKey identifies one reading of the config file: a different file (inode),
// a rewrite (mtime, size) or a chmod (mode) all change it.
type fileKey struct {
	mtime int64
	ino   uint64
	size  int64
	mode  os.FileMode
}

var (
	resolveMu    sync.Mutex
	resolveCache = map[string]resolved{}
)

type resolved struct {
	key  fileKey
	snap Snapshot
}

// Resolve is Load, read per call: it stats the file and re-reads it only when
// the file changed, so `gate set --mode enforce` reaches a host that is already
// running without a restart, and a long-lived host does not parse the file on
// every tool call. It fails closed exactly as Load does.
func Resolve(root string) Snapshot {
	path := filepath.Join(root, ".devcouncil", "config.yaml")
	info, err := os.Stat(path)
	if err != nil {
		resolveMu.Lock()
		delete(resolveCache, path)
		resolveMu.Unlock()
		return Load(root)
	}
	key := fileKey{mtime: info.ModTime().UnixNano(), ino: inode(info), size: info.Size(), mode: info.Mode()}
	resolveMu.Lock()
	if hit, ok := resolveCache[path]; ok && hit.key == key {
		resolveMu.Unlock()
		return hit.snap
	}
	resolveMu.Unlock()
	snap := Load(root)
	resolveMu.Lock()
	if len(resolveCache) > 64 {
		resolveCache = map[string]resolved{}
	}
	resolveCache[path] = resolved{key: key, snap: snap}
	resolveMu.Unlock()
	return snap
}

// SetVerificationMode writes gates.mode surgically. It does not rewrite the
// rest of the file.
func SetVerificationMode(path, mode string) error {
	canonical, err := ParseMode(mode)
	if err != nil {
		return err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		if !os.IsNotExist(err) {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		// gates.mode only. This used to add `execution.hook_gate.mode: off`, a
		// key Load above names as retired and nothing reads — so every new
		// config was born carrying a setting for a gate that does not exist.
		body := "gates:\n  mode: " + canonical + "\n"
		return safefile.WriteAtomic(path, []byte(body), 0o644)
	}
	updated, err := patchYAMLKey(string(data), "gates", "mode", canonical)
	if err != nil {
		return err
	}
	return safefile.WriteAtomic(path, []byte(updated), 0o644)
}

func patchYAMLKey(src, section, key, value string) (string, error) {
	return patchNestedYAML(src, []string{section}, key, value)
}

func patchNestedYAML(src string, sections []string, key, value string) (string, error) {
	src = strings.ReplaceAll(src, "\r\n", "\n")
	src = strings.ReplaceAll(src, "\r", "\n")
	lines := strings.Split(src, "\n")
	// Dotted single-line form: gates.mode: enforce
	dotted := strings.Join(append(append([]string{}, sections...), key), ".")
	for i, line := range lines {
		trim := strings.TrimSpace(line)
		if strings.HasPrefix(trim, dotted+":") && !strings.HasPrefix(trim, "#") {
			indent := line[:len(line)-len(strings.TrimLeft(line, " \t"))]
			lines[i] = indent + dotted + ": " + value
			return strings.Join(lines, "\n"), nil
		}
	}

	depth := 0
	sectionIndent := -1
	for i, line := range lines {
		if strings.TrimSpace(line) == "" || strings.HasPrefix(strings.TrimSpace(line), "#") {
			continue
		}
		indent := leadingSpaces(line)
		trim := strings.TrimSpace(line)
		if depth < len(sections) {
			want := sections[depth] + ":"
			if trim == want || strings.HasPrefix(trim, want+" ") {
				if depth == 0 || indent > sectionIndent {
					depth++
					sectionIndent = indent
					continue
				}
			}
			if depth > 0 && indent <= sectionIndent && trim != want {
				// Left the current section without finding the key — insert before this line.
				insert := strings.Repeat(" ", sectionIndent+2) + key + ": " + value
				out := make([]string, 0, len(lines)+1)
				out = append(out, lines[:i]...)
				out = append(out, insert)
				out = append(out, lines[i:]...)
				return strings.Join(out, "\n"), nil
			}
			continue
		}
		// Inside the target section.
		wantKey := key + ":"
		if indent == sectionIndent+2 && (trim == wantKey || strings.HasPrefix(trim, wantKey+" ") || strings.HasPrefix(trim, wantKey+"\t")) {
			prefix := line[:len(line)-len(strings.TrimLeft(line, " \t"))]
			lines[i] = prefix + key + ": " + value
			return strings.Join(lines, "\n"), nil
		}
		if indent <= sectionIndent {
			insert := strings.Repeat(" ", sectionIndent+2) + key + ": " + value
			out := make([]string, 0, len(lines)+1)
			out = append(out, lines[:i]...)
			out = append(out, insert)
			out = append(out, lines[i:]...)
			return strings.Join(out, "\n"), nil
		}
	}
	if depth == len(sections) {
		insert := strings.Repeat(" ", sectionIndent+2) + key + ": " + value
		if len(lines) > 0 && lines[len(lines)-1] == "" {
			lines = append(lines[:len(lines)-1], insert, "")
			return strings.Join(lines, "\n"), nil
		}
		return strings.TrimRight(src, "\n") + "\n" + insert + "\n", nil
	}
	// Sections missing — append a nested stub.
	var b strings.Builder
	b.WriteString(strings.TrimRight(src, "\n"))
	b.WriteByte('\n')
	for i, sec := range sections {
		b.WriteString(strings.Repeat("  ", i))
		b.WriteString(sec)
		b.WriteString(":\n")
	}
	b.WriteString(strings.Repeat("  ", len(sections)))
	b.WriteString(key)
	b.WriteString(": ")
	b.WriteString(value)
	b.WriteByte('\n')
	return b.String(), nil
}

func leadingSpaces(s string) int {
	n := 0
	for _, r := range s {
		if r == ' ' {
			n++
			continue
		}
		if r == '\t' {
			n += 2
			continue
		}
		break
	}
	return n
}
