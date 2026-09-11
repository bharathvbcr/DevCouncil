package skills

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
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
		skillName := strings.TrimSuffix(name, ".md")
		// Prefer frontmatter name when present.
		if n := frontmatterName(data); n != "" {
			skillName = n
		}
		out = append(out, Skill{Name: skillName, Content: data})
	}
	return out, nil
}

func frontmatterName(data []byte) string {
	text := string(data)
	if !strings.HasPrefix(text, "---\n") {
		return ""
	}
	rest := text[4:]
	end := strings.Index(rest, "\n---\n")
	if end < 0 {
		return ""
	}
	for _, line := range strings.Split(rest[:end], "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "name:") {
			return strings.TrimSpace(strings.TrimPrefix(line, "name:"))
		}
	}
	return ""
}

// Options control scaffold.
type Options struct {
	Root         string
	Skills       []Skill
	Destinations []string
	DryRun       bool
	CheckOnly    bool
}

// Result is what was (or would be) written.
type Result struct {
	Files   []string
	Receipt map[string]string
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

	receiptRel := ".devcouncil-skills.json"
	if opts.DryRun || opts.CheckOnly || len(rendered) == 0 {
		files := make([]string, 0, len(rendered))
		for k := range rendered {
			files = append(files, k)
		}
		return &Result{Files: files}, nil
	}

	lockPath := filepath.Join(root, ".devcouncil-skills.lock")
	if err := acquireLock(lockPath, LockWait); err != nil {
		return nil, err
	}
	defer os.Remove(lockPath)

	hashes, err := loadReceipt(root, receiptRel)
	if err != nil {
		return nil, err
	}
	var written []string
	for key, content := range rendered {
		target, err := scaffoldPath(root, key)
		if err != nil {
			return nil, err
		}
		current, err := readScaffoldFile(target, SkillFileLimit)
		if err != nil {
			return nil, err
		}
		if current != nil && !bytesEqual(current, content) {
			if hashes[key] != sha256Hex(current) {
				return nil, fmt.Errorf("%s: refusing to overwrite unowned or locally modified skill file", key)
			}
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return nil, err
		}
		if err := atomicWrite(target, content); err != nil {
			return nil, err
		}
		hashes[key] = sha256Hex(content)
		written = append(written, key)
	}
	if len(hashes) > ReceiptEntries {
		return nil, fmt.Errorf("%s: invalid or oversized skill installation receipt", receiptRel)
	}
	doc := map[string]any{"schema": 1, "files": hashes}
	raw, err := json.Marshal(doc)
	if err != nil {
		return nil, err
	}
	if len(raw) > ReceiptByteLimit {
		return nil, fmt.Errorf("%s: receipt exceeds byte limit", receiptRel)
	}
	receiptPath := filepath.Join(root, receiptRel)
	if err := atomicWrite(receiptPath, prettyReceipt(hashes)); err != nil {
		return nil, err
	}
	return &Result{Files: written, Receipt: hashes}, nil
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

func atomicWrite(path string, content []byte) error {
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, ".skill-*.tmp")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	defer os.Remove(tmpName)
	if _, err := tmp.Write(content); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmpName, path)
}

func sha256Hex(b []byte) string {
	sum := sha256.Sum256(b)
	return hex.EncodeToString(sum[:])
}

func bytesEqual(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
