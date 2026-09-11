package components

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"
)

var saveMu sync.Mutex

const stateVersion = 1

// State is the install receipt under $PREFIX/share/devcouncil/components.json.
type State struct {
	Version   int                   `json:"version"`
	Prefix    string                `json:"prefix"`
	Installed map[string]InstallRec `json:"installed"`
	Disabled  []string              `json:"disabled"`
}

// InstallRec records one binary we put on disk.
type InstallRec struct {
	Binary string `json:"binary"`
	At     string `json:"at"`
}

func statePath(prefix string) string {
	return filepath.Join(prefix, "share", "devcouncil", "components.json")
}

func disabledTextPath(prefix string) string {
	return filepath.Join(prefix, "share", "devcouncil", "disabled")
}

func emptyState(prefix string) State {
	return State{
		Version:   stateVersion,
		Prefix:    prefix,
		Installed: map[string]InstallRec{},
	}
}

func LoadState(prefix string) State {
	st := emptyState(prefix)
	data, err := os.ReadFile(statePath(prefix))
	if err == nil {
		var parsed State
		if json.Unmarshal(data, &parsed) != nil {
			// Corrupt receipt: refuse the partial map rather than keep it.
			st.Disabled = mergeDisabled(prefix, nil)
			return st
		}
		if parsed.Installed != nil {
			st.Installed = parsed.Installed
		}
		st.Disabled = parsed.Disabled
	}
	st.Disabled = mergeDisabled(prefix, st.Disabled)
	st.Prefix = prefix
	st.Version = stateVersion
	return st
}

func mergeDisabled(prefix string, jsonIDs []string) []string {
	ids := append([]string{}, jsonIDs...)
	data, err := os.ReadFile(disabledTextPath(prefix))
	if err == nil {
		for _, line := range strings.Split(string(data), "\n") {
			ids = append(ids, strings.TrimSpace(line))
		}
	}
	return sanitizeDisabled(ids)
}

func sanitizeDisabled(ids []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, id := range ids {
		id = strings.TrimSpace(strings.ToLower(id))
		if _, ok := Lookup(id); !ok {
			continue
		}
		if seen[id] {
			continue
		}
		seen[id] = true
		out = append(out, id)
	}
	return out
}

func SaveState(st State) error {
	saveMu.Lock()
	defer saveMu.Unlock()
	dir := filepath.Dir(statePath(st.Prefix))
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	st.Version = stateVersion
	st.Disabled = sanitizeDisabled(st.Disabled)
	if st.Installed == nil {
		st.Installed = map[string]InstallRec{}
	}
	data, err := json.MarshalIndent(st, "", "  ")
	if err != nil {
		return err
	}
	data = append(data, '\n')
	if err := writeAtomicFile(statePath(st.Prefix), data, 0o644); err != nil {
		return err
	}
	body := strings.Join(st.Disabled, "\n")
	if body != "" {
		body += "\n"
	}
	return writeAtomicFile(disabledTextPath(st.Prefix), []byte(body), 0o644)
}

func writeAtomicFile(path string, data []byte, perm os.FileMode) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	f, err := os.CreateTemp(dir, ".tmp-*")
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
		return err
	}
	ok = true
	return nil
}

func (st *State) Record(id, binary string) {
	if st.Installed == nil {
		st.Installed = map[string]InstallRec{}
	}
	st.Installed[id] = InstallRec{Binary: binary, At: time.Now().UTC().Format(time.RFC3339)}
}

func (st *State) Forget(id string) {
	delete(st.Installed, id)
	st.Disabled = slices.DeleteFunc(st.Disabled, func(s string) bool { return s == id })
}

func (st State) IsDisabled(id string) bool {
	return slices.Contains(st.Disabled, id)
}

func (st *State) SetDisabled(id string, disabled bool) {
	if disabled {
		if !slices.Contains(st.Disabled, id) {
			st.Disabled = append(st.Disabled, id)
		}
		st.Disabled = sanitizeDisabled(st.Disabled)
		return
	}
	st.Disabled = slices.DeleteFunc(st.Disabled, func(s string) bool { return s == id })
}
