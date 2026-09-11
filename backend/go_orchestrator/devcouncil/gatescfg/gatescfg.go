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

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/flags"
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
	HookGate           string `json:"hook_gate"`
	WriteGate          bool   `json:"write_gate"`
	ConfigPath         string `json:"config_path"`
	ConfigPresent      bool   `json:"config_present"`
}

// Normalize maps config spellings onto off|advisory|enforce.
// Empty, false, 0, none, and off are off. Unknown values are refused by
// ParseMode; Normalize treats them as off so a typo cannot silently enforce.
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
		return ModeOff, OriginInvalid
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

// Load reads `.devcouncil/config.yaml` under root. Missing file → off, origin default.
func Load(root string) Snapshot {
	path := filepath.Join(root, ".devcouncil", "config.yaml")
	snap := Snapshot{
		VerificationMode:   ModeOff,
		VerificationOrigin: OriginDefault,
		HookGate:           ModeOff,
		WriteGate:          false,
		ConfigPath:         path,
	}
	f, err := os.Open(path)
	if err != nil {
		return snap
	}
	defer func() { _ = f.Close() }()
	snap.ConfigPresent = true
	values, err := flags.ParseConfig(f)
	if err != nil {
		snap.VerificationOrigin = OriginInvalid
		return snap
	}
	if _, scalar := values["gates"]; scalar {
		// `gates: [` and `gates: off` are scalars, not a mapping with mode.
		// Treat them as unreadable rather than as "no gates.mode, default off".
		snap.VerificationOrigin = OriginInvalid
		return snap
	}
	snap.VerificationMode, snap.VerificationOrigin = Normalize(values["gates.mode"])
	if hook := strings.TrimSpace(strings.ToLower(values["execution.hook_gate.mode"])); hook != "" {
		if hook == "contain" {
			snap.HookGate = "contain"
		} else {
			mode, _ := Normalize(hook)
			snap.HookGate = mode
		}
	}
	wg := strings.TrimSpace(strings.ToLower(values["integrations.cursor.write_gate"]))
	snap.WriteGate = wg == "true" || wg == "1" || wg == "yes"
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
		body := "gates:\n  mode: " + canonical + "\nexecution:\n  hook_gate:\n    mode: off\n"
		return writeAtomicFile(path, []byte(body), 0o644)
	}
	updated, err := patchYAMLKey(string(data), "gates", "mode", canonical)
	if err != nil {
		return err
	}
	return writeAtomicFile(path, []byte(updated), 0o644)
}

// SetHookGate writes execution.hook_gate.mode (off or contain).
func SetHookGate(path, mode string) error {
	s := strings.TrimSpace(strings.ToLower(mode))
	if s != ModeOff && s != "contain" {
		return fmt.Errorf("unknown hook-gate mode %q (want off or contain)", mode)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		if !os.IsNotExist(err) {
			return err
		}
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return err
		}
		body := "gates:\n  mode: off\nexecution:\n  hook_gate:\n    mode: " + s + "\n"
		return writeAtomicFile(path, []byte(body), 0o644)
	}
	updated, err := patchNestedYAML(string(data), []string{"execution", "hook_gate"}, "mode", s)
	if err != nil {
		return err
	}
	return writeAtomicFile(path, []byte(updated), 0o644)
}

func patchYAMLKey(src, section, key, value string) (string, error) {
	return patchNestedYAML(src, []string{section}, key, value)
}

func writeAtomicFile(path string, data []byte, perm os.FileMode) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	f, err := os.CreateTemp(dir, ".cfg-*.tmp")
	if err != nil {
		return err
	}
	tmp := f.Name()
	ok := false
	defer func() {
		if !ok {
			_ = os.Remove(tmp)
		}
	}()
	if _, err := f.Write(data); err != nil {
		_ = f.Close()
		return err
	}
	if err := f.Chmod(perm); err != nil {
		_ = f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		_ = f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmp, path); err != nil {
		_ = os.Remove(path)
		if err := os.Rename(tmp, path); err != nil {
			return err
		}
	}
	ok = true
	return nil
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
