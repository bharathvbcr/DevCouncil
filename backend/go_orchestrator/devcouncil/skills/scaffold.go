package skills

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/safefile"
)

// Bounds ported from Python skills/registry.py.
const (
	MaxSkills        = 256
	MaxDestinations  = 16
	SkillFileLimit   = 256 * 1024
	BatchByteLimit   = 8 * 1024 * 1024
	ReceiptEntries   = 4096
	ReceiptByteLimit = 1024 * 1024
	LockWait         = 5 * time.Second

	receiptRel = ".devcouncil-skills.json"
)

var (
	DefaultDestinations = []string{".claude/skills", ".cursor/skills", ".agents/skills"}
	skillNameRe         = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,63}$`)
	hashRe              = regexp.MustCompile(`^[a-f0-9]{64}$`)
	reservedNames       = map[string]struct{}{
		"con": {}, "prn": {}, "aux": {}, "nul": {},
		"com1": {}, "com2": {}, "com3": {}, "com4": {}, "com5": {},
		"com6": {}, "com7": {}, "com8": {}, "com9": {},
		"lpt1": {}, "lpt2": {}, "lpt3": {}, "lpt4": {}, "lpt5": {},
		"lpt6": {}, "lpt7": {}, "lpt8": {}, "lpt9": {},
	}
)

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
	Root         string
	Skills       []Skill
	Destinations []string
	DryRun       bool
	CheckOnly    bool
}

// Result is what was (or would be) written. Files names only the paths that
// differ from what is on disk, so an empty Files is the "already installed"
// answer and a check that merely ran cannot be mistaken for one that passed.
type Result struct {
	Files   []string
	Receipt map[string]string
}

// step is one file an install would change, captured before anything is written.
type step struct {
	key      string
	target   string
	content  []byte
	previous []byte
}

// Scaffold writes skills under each destination as <name>/SKILL.md.
func Scaffold(opts Options) (*Result, error) {
	root, err := filepath.Abs(opts.Root)
	if err != nil {
		return nil, err
	}
	info, err := os.Stat(root)
	if err != nil {
		return nil, err
	}
	if !info.IsDir() {
		return nil, fmt.Errorf("%s: skill destination root is not a directory", root)
	}
	dests := opts.Destinations
	if len(dests) == 0 {
		dests = append([]string{}, DefaultDestinations...)
	}
	if len(dests) == 0 || len(dests) > MaxDestinations || len(opts.Skills) > MaxSkills {
		return nil, errors.New("Skill install limit: 1–16 destinations and at most 256 skills")
	}

	rendered := map[string][]byte{}
	names := map[string]struct{}{}
	total := 0
	for _, skill := range opts.Skills {
		if !skillNameRe.MatchString(skill.Name) {
			return nil, fmt.Errorf("Invalid skill name: %q", skill.Name)
		}
		if _, bad := reservedNames[skill.Name]; bad {
			return nil, fmt.Errorf("Invalid skill name: %q", skill.Name)
		}
		if _, dup := names[skill.Name]; dup {
			return nil, fmt.Errorf("Conflicting duplicate skill name: %s", skill.Name)
		}
		names[skill.Name] = struct{}{}
		if len(skill.Content) > SkillFileLimit {
			return nil, fmt.Errorf("%s: skill exceeds byte limit", skill.Name)
		}
		for _, dest := range dests {
			key := filepath.ToSlash(filepath.Join(dest, skill.Name, "SKILL.md"))
			if _, err := scaffoldPath(root, key); err != nil {
				return nil, err
			}
			rendered[key] = skill.Content
			total += len(skill.Content)
		}
	}
	if total > BatchByteLimit {
		return nil, errors.New("Skill install exceeds 8 MiB batch limit")
	}

	// The whole batch is planned before anything is written, so a refusal on
	// the last host cannot leave the first two upgraded. Three agent hosts
	// share this corpus; a half-applied install makes them disagree about what
	// the guidance says, which is worse than not installing at all.
	if opts.DryRun || opts.CheckOnly || len(rendered) == 0 {
		steps, _, err := plan(root, rendered, true)
		if err != nil {
			return nil, err
		}
		return &Result{Files: keysOf(steps)}, nil
	}

	lockPath := filepath.Join(root, ".devcouncil-skills.lock")
	if err := acquireLock(lockPath, LockWait); err != nil {
		return nil, err
	}
	defer os.Remove(lockPath)

	// Re-planned under the lock: the tree read a moment ago was not yet ours.
	steps, hashes, err := plan(root, rendered, false)
	if err != nil {
		return nil, err
	}
	written := make([]string, 0, len(steps))
	for _, s := range steps {
		current, err := readScaffoldFile(s.target, SkillFileLimit)
		if err != nil {
			return nil, err
		}
		if !sameContent(current, s.previous) {
			return nil, fmt.Errorf("%s: modified while installing skills", s.target)
		}
		if err := os.MkdirAll(filepath.Dir(s.target), 0o755); err != nil {
			return nil, err
		}
		if err := safefile.WriteAtomic(s.target, s.content, 0o644); err != nil {
			return nil, err
		}
		written = append(written, s.key)
	}

	receiptPath := filepath.Join(root, receiptRel)
	encoded := prettyReceipt(hashes)
	if len(encoded) > ReceiptByteLimit {
		return nil, fmt.Errorf("%s: receipt exceeds byte limit", receiptRel)
	}
	prior, err := readScaffoldFile(receiptPath, ReceiptByteLimit)
	if err != nil {
		return nil, err
	}
	if !sameContent(prior, encoded) {
		if err := safefile.WriteAtomic(receiptPath, encoded, 0o644); err != nil {
			return nil, err
		}
	}
	return &Result{Files: written, Receipt: hashes}, nil
}

// plan resolves every rendered path and reads what is already there, returning
// only the files that differ. checkOnly reports an unowned file as work to do
// instead of refusing, so `--check` can describe a tree it may not modify.
func plan(root string, rendered map[string][]byte, checkOnly bool) ([]step, map[string]string, error) {
	hashes, err := loadReceipt(root, receiptRel)
	if err != nil {
		return nil, nil, err
	}
	keys := make([]string, 0, len(rendered))
	for key := range rendered {
		keys = append(keys, key)
	}
	sort.Strings(keys)

	steps := make([]step, 0, len(keys))
	for _, key := range keys {
		content := rendered[key]
		target, err := scaffoldPath(root, key)
		if err != nil {
			return nil, nil, err
		}
		current, err := readScaffoldFile(target, SkillFileLimit)
		if err != nil {
			return nil, nil, err
		}
		if !sameContent(current, content) {
			// Wording and shape are kept identical to the Rust installer in
			// rust/devmap-cli/src/skills.rs: two ports of one behaviour whose
			// refusals read differently drift without anything noticing.
			if !checkOnly && current != nil && hashes[key] != sha256Hex(current) {
				return nil, nil, fmt.Errorf("%s: unmanaged or locally modified skill; preserve or move it before installing", target)
			}
			steps = append(steps, step{key: key, target: target, content: content, previous: current})
		}
		hashes[key] = sha256Hex(content)
	}
	if len(hashes) > ReceiptEntries {
		return nil, nil, fmt.Errorf("%s: invalid or oversized skill installation receipt", receiptRel)
	}
	return steps, hashes, nil
}

func keysOf(steps []step) []string {
	out := make([]string, 0, len(steps))
	for _, s := range steps {
		out = append(out, s.key)
	}
	return out
}

func prettyReceipt(hashes map[string]string) []byte {
	doc := map[string]any{"schema": 1, "files": hashes}
	b, _ := json.MarshalIndent(doc, "", "  ")
	return append(b, '\n')
}

func acquireLock(path string, wait time.Duration) error {
	deadline := time.Now().Add(wait)
	for {
		err := os.Mkdir(path, 0o755)
		if err == nil {
			return nil
		}
		if !os.IsExist(err) {
			return err
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("Skill installer is busy: %s", path)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func scaffoldPath(root, relative string) (string, error) {
	if relative == "" || strings.Contains(relative, `\`) || strings.Contains(relative, ":") || strings.HasPrefix(relative, "/") {
		return "", fmt.Errorf("Invalid skill destination: %q", relative)
	}
	parts := strings.Split(relative, "/")
	for _, part := range parts {
		if part == "" || part == "." || part == ".." {
			return "", fmt.Errorf("Invalid skill destination: %q", relative)
		}
	}
	path := root
	for i, part := range parts {
		path = filepath.Join(path, part)
		info, err := os.Lstat(path)
		if err != nil {
			if os.IsNotExist(err) {
				continue
			}
			return "", err
		}
		if info.Mode()&os.ModeSymlink != 0 {
			return "", fmt.Errorf("%s: refusing symlink in skill destination", path)
		}
		if i < len(parts)-1 && !info.IsDir() {
			return "", fmt.Errorf("%s: skill parent is not a directory", path)
		}
	}
	return path, nil
}

func readScaffoldFile(path string, limit int) ([]byte, error) {
	info, err := os.Lstat(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}
	if info.Mode()&os.ModeSymlink != 0 {
		return nil, fmt.Errorf("%s: refusing symlink in skill destination", path)
	}
	if !info.Mode().IsRegular() || info.Size() > int64(limit) {
		return nil, fmt.Errorf("%s: not a regular file within the %d byte limit", path, limit)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	if len(data) > limit {
		return nil, fmt.Errorf("%s: file grew beyond byte limit", path)
	}
	return data, nil
}

func loadReceipt(root, rel string) (map[string]string, error) {
	path := filepath.Join(root, rel)
	raw, err := readScaffoldFile(path, ReceiptByteLimit)
	if err != nil {
		return nil, err
	}
	if raw == nil {
		return map[string]string{}, nil
	}
	var saved struct {
		Schema int               `json:"schema"`
		Files  map[string]string `json:"files"`
	}
	if err := json.Unmarshal(raw, &saved); err != nil {
		return nil, fmt.Errorf("%s: invalid skill installation receipt", rel)
	}
	if saved.Schema != 1 || saved.Files == nil {
		return nil, fmt.Errorf("%s: invalid skill installation receipt", rel)
	}
	if len(saved.Files) > ReceiptEntries {
		return nil, fmt.Errorf("%s: invalid or oversized skill installation receipt", rel)
	}
	for k, v := range saved.Files {
		if !hashRe.MatchString(v) {
			return nil, fmt.Errorf("%s: invalid or oversized skill installation receipt", rel)
		}
		_ = k
	}
	return saved.Files, nil
}

func sha256Hex(b []byte) string {
	sum := sha256.Sum256(b)
	return hex.EncodeToString(sum[:])
}

// sameContent compares two reads of a path, where nil means "no file there".
//
// bytes.Equal alone would call a missing file equal to an empty one, which
// leaves a zero-byte skill uninstalled forever and lets a file created between
// the plan and the write pass as untouched.
func sameContent(a, b []byte) bool {
	return (a == nil) == (b == nil) && bytes.Equal(a, b)
}
