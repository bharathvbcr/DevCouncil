package skills

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"path/filepath"
	"strings"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/devmap"
)

// This package owns DevCouncil's domain-skill *library*: which skills ship and
// what they say. Installing them is `devmap skills install`'s job. A second Go
// installer used to write the same `.devcouncil-skills.json` receipt and lock
// as the Rust one, with bounds and refusals kept identical by hand; the
// library now goes to DevMap over `--library-stdin`, so the receipt has one
// writer. Its contracts — refusing unowned edits, all-or-nothing batches, the
// lock — are tested in `rust/devmap-cli/src/skills.rs`.

// Skill is one embedded domain skill.
type Skill struct {
	Name    string
	Content []byte // full SKILL.md bytes (frontmatter + body)
}

// Library is the embed.FS of domain skills.
type Library struct {
	FS fs.FS
}

// Load reads every *.md skill from the library FS (skipping README).
//
// A skill whose frontmatter cannot be read is an error, not a fallback to the
// file name: the name is what an agent host is told to look for, so guessing it
// installs working-looking guidance under a name nothing will ask for.
func (lib Library) Load() ([]Skill, error) {
	entries, err := fs.ReadDir(lib.FS, ".")
	if err != nil {
		return nil, err
	}
	var out []Skill
	for _, e := range entries {
		name := e.Name()
		if e.IsDir() || !strings.HasSuffix(name, ".md") || strings.EqualFold(name, "README.md") {
			continue
		}
		data, err := fs.ReadFile(lib.FS, name)
		if err != nil {
			return nil, err
		}
		skillName, err := frontmatterName(data)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", name, err)
		}
		out = append(out, Skill{Name: skillName, Content: data})
	}
	return out, nil
}

// frontmatterName reads `name:` out of the leading YAML block.
//
// LF is assumed rather than tolerated: the repository's `.gitattributes` sets
// `* -text`, so these bytes are identical on every checkout including Windows.
func frontmatterName(data []byte) (string, error) {
	text := string(data)
	if !strings.HasPrefix(text, "---\n") {
		return "", errors.New("skill has no YAML frontmatter")
	}
	rest := text[4:]
	end := strings.Index(rest, "\n---\n")
	if end < 0 {
		return "", errors.New("skill frontmatter is not terminated")
	}
	for _, line := range strings.Split(rest[:end], "\n") {
		value, ok := strings.CutPrefix(strings.TrimSpace(line), "name:")
		if !ok {
			continue
		}
		if value = strings.TrimSpace(value); value != "" {
			return value, nil
		}
		return "", errors.New("skill frontmatter has an empty name")
	}
	return "", errors.New("skill frontmatter has no name")
}

// Options control scaffold.
type Options struct {
	Root   string
	Skills []Skill
	// Destinations are skill layout roots relative to Root. Empty means
	// DevMap's defaults: `.claude/skills`, `.cursor/skills`, `.agents/skills`.
	Destinations []string
	DryRun       bool
	CheckOnly    bool
	// DevmapBin is an explicit devmap, used or refused; empty resolves
	// DEVMAP_BIN, then PATH outside the repository.
	DevmapBin string
}

// Result is what was (or would be) written. Files names only the paths that
// differ from what is on disk, so an empty Files is the "already installed"
// answer and a check that merely ran cannot be mistaken for one that passed.
type Result struct {
	Files []string
}

// Scaffold installs skills under each destination as <name>/SKILL.md through
// `devmap skills install --library-stdin`.
func Scaffold(opts Options) (*Result, error) {
	root, err := filepath.Abs(opts.Root)
	if err != nil {
		return nil, err
	}
	if len(opts.Skills) == 0 {
		return nil, errors.New("no skills selected")
	}
	// Both are read-only and report the same files; a check also answers
	// whether anything differs, so it wins when both are asked for.
	mode := devmap.InstallApply
	switch {
	case opts.CheckOnly:
		mode = devmap.InstallCheck
	case opts.DryRun:
		mode = devmap.InstallDryRun
	}
	binary, err := devmap.ResolveOutside(opts.DevmapBin, root)
	if err != nil {
		return nil, fmt.Errorf("%w; skills are installed by `devmap skills install`", err)
	}
	library := make([]devmap.Skill, 0, len(opts.Skills))
	for _, skill := range opts.Skills {
		library = append(library, devmap.Skill{Name: skill.Name, Content: string(skill.Content)})
	}
	report, err := devmap.New(binary, root).InstallSkills(context.Background(), opts.Destinations, library, mode)
	if err != nil {
		return nil, err
	}
	paths := report.Differing
	if mode == devmap.InstallApply {
		paths = report.Written
	}
	files := make([]string, 0, len(paths))
	for _, p := range paths {
		files = append(files, devmap.RelativeTo(root, p))
	}
	return &Result{Files: files}, nil
}
