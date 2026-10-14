package components

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
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
	Origin
}

// Origin is where an installed binary was built from. A remote install names
// the public repository and the ref it was pinned to ("tag v0.2.4" or
// "rev <commit>"), so two receipts can say whether they hold the same build.
// A checkout install names the checkout and no ref: it builds the working
// tree as it stands, uncommitted edits included, which no commit names.
// Receipts written before these fields existed read back with both empty.
type Origin struct {
	Source string `json:"source,omitempty"`
	Ref    string `json:"ref,omitempty"`
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

// LoadState reads the receipt. A missing receipt is an empty state. A receipt
// that exists but cannot be read or parsed is an error: every caller goes on
// to SaveState, and saving an empty stand-in over a corrupt file would erase
// the record of every other component. The returned State is still safe to
// read (empty) so a caller that only reports can show it beside the error.
func LoadState(prefix string) (State, error) {
	st := emptyState(prefix)
	data, err := os.ReadFile(statePath(prefix))
	switch {
	case err == nil:
		var parsed State
		if err := json.Unmarshal(data, &parsed); err != nil {
			st.Disabled = mergeDisabled(prefix, nil)
			return st, fmt.Errorf("components receipt %s is corrupt (%w); fix or remove it, nothing was changed", statePath(prefix), err)
		}
		if parsed.Installed != nil {
			st.Installed = parsed.Installed
		}
		st.Disabled = parsed.Disabled
	case !errors.Is(err, fs.ErrNotExist):
		return st, fmt.Errorf("components receipt %s is unreadable: %w", statePath(prefix), err)
	}
	st.Disabled = mergeDisabled(prefix, st.Disabled)
	st.Prefix = prefix
	st.Version = stateVersion
	return st, nil
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
	if err := safefile.WriteAtomic(statePath(st.Prefix), data, 0o644); err != nil {
		return err
	}
	body := strings.Join(st.Disabled, "\n")
	if body != "" {
		body += "\n"
	}
	return safefile.WriteAtomic(disabledTextPath(st.Prefix), []byte(body), 0o644)
}

func (st *State) Record(id, binary string, from Origin) {
	if st.Installed == nil {
		st.Installed = map[string]InstallRec{}
	}
	st.Installed[id] = InstallRec{Binary: binary, At: time.Now().UTC().Format(time.RFC3339), Origin: from}
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
