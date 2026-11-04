// Package agents is the Antigravity agent library DevCouncil embeds.
//
// Installing it is `devmap agents install`'s job. This package only knows which
// files ship and what they say. The primary and the always-on rule are copied
// from agents/antigravity; the three roles are rendered by
// scripts/sync-agent-defs.mjs. A second copy under `.agents/` is not source:
// that tree holds machine-specific MCP paths and is gitignored.
package agents

import (
	"embed"
	"fmt"
	"io/fs"
	"path"
	"slices"
	"strings"
)

//go:embed library
var raw embed.FS

// File is one installable Markdown document.
type File struct {
	Name    string
	Content []byte
}

// Set is the library `devcouncil integrate antigravity` hands to DevMap.
type Set struct {
	Agents []File
	Rules  []File
}

// Load reads the embedded library and refuses a set that is not the four
// agents plus the one rule. A short library would install a session that
// cannot delegate, and that is not a successful install of a smaller one.
func Load() (Set, error) {
	sub, err := fs.Sub(raw, "library")
	if err != nil {
		return Set{}, err
	}
	var set Set
	err = fs.WalkDir(sub, ".", func(name string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if entry.IsDir() {
			if name != "." && name != "rules" {
				return fmt.Errorf("%s: unexpected directory in the agent library", name)
			}
			return nil
		}
		if !strings.HasSuffix(name, ".md") {
			return fmt.Errorf("%s: agent library file is not markdown", name)
		}
		data, err := fs.ReadFile(sub, name)
		if err != nil {
			return err
		}
		file := File{Name: strings.TrimSuffix(path.Base(name), ".md"), Content: data}
		switch path.Dir(name) {
		case ".":
			set.Agents = append(set.Agents, file)
		case "rules":
			set.Rules = append(set.Rules, file)
		default:
			return fmt.Errorf("%s: unexpected agent library path", name)
		}
		return nil
	})
	if err != nil {
		return Set{}, err
	}
	slices.SortFunc(set.Agents, func(a, b File) int { return strings.Compare(a.Name, b.Name) })
	slices.SortFunc(set.Rules, func(a, b File) int { return strings.Compare(a.Name, b.Name) })
	wantAgents := []string{"devcouncil", "devcouncil-implementer", "devcouncil-reviewer", "devcouncil-verifier"}
	gotAgents := names(set.Agents)
	if !slices.Equal(gotAgents, wantAgents) {
		return Set{}, fmt.Errorf("agent library agents = %v, want %v", gotAgents, wantAgents)
	}
	if len(set.Rules) != 1 || set.Rules[0].Name != "devcouncil" {
		return Set{}, fmt.Errorf("agent library rules = %v, want [devcouncil]", names(set.Rules))
	}
	for _, file := range set.Agents {
		declared, err := frontmatter(file.Content, "name")
		if err != nil {
			return Set{}, fmt.Errorf("%s: %w", file.Name, err)
		}
		if declared != file.Name {
			return Set{}, fmt.Errorf("%s: frontmatter name %q", file.Name, declared)
		}
	}
	trigger, err := frontmatter(set.Rules[0].Content, "trigger")
	if err != nil {
		return Set{}, fmt.Errorf("rule: %w", err)
	}
	if trigger != "always_on" {
		return Set{}, fmt.Errorf("rule trigger %q, want always_on", trigger)
	}
	return set, nil
}

func names(files []File) []string {
	out := make([]string, len(files))
	for i, file := range files {
		out[i] = file.Name
	}
	return out
}

// frontmatter reads one top-level key from the leading YAML block.
func frontmatter(data []byte, key string) (string, error) {
	text := string(data)
	rest, ok := strings.CutPrefix(text, "---\n")
	if !ok {
		return "", fmt.Errorf("must begin with '---' YAML frontmatter")
	}
	front, _, ok := strings.Cut(rest, "\n---\n")
	if !ok {
		return "", fmt.Errorf("frontmatter is never closed")
	}
	prefix := key + ":"
	for _, line := range strings.Split(front, "\n") {
		if strings.HasPrefix(line, " ") || strings.HasPrefix(line, "\t") || strings.HasPrefix(line, "#") {
			continue
		}
		value, found := strings.CutPrefix(line, prefix)
		if !found {
			continue
		}
		value = strings.TrimSpace(value)
		value = strings.Trim(value, `"`)
		if value == "" {
			return "", fmt.Errorf("frontmatter %s is empty", key)
		}
		return value, nil
	}
	return "", fmt.Errorf("frontmatter has no %s", key)
}
