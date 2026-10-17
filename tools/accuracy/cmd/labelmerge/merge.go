package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// fileName is a label-store file: one language, one optional round.
var fileName = regexp.MustCompile(`^labels-([a-z][a-z0-9_]*)(?:-r([0-9]+))?\.json$`)

// identityHash is the shape of a private corpus's `symbol_hash`: `h:` and
// twelve lowercase hex digits. It mirrors `is_identity_hash` in
// rust/devmap-analyze/src/record_identity.rs, which the runner applies; a test
// holds the two together.
var identityHash = regexp.MustCompile(`^h:[0-9a-f]{12}$`)

// evidenceCategories mirrors `EVIDENCE_CATEGORIES` in record_identity.rs
// (checked by a test); a private corpus's dead label carries one.
var evidenceCategories = []string{
	"rg_declaration_only",
	"direct_reference",
	"framework_hook",
	"runtime_import_by_name",
	"spot_check_upstream_shadow",
	"spot_check_comment_only",
}

// sections are the runner's label arrays, in output order.
var sections = []string{"edges", "recall", "oracle", "dead"}

// spec says which fields a section's entries must carry and how two entries
// are recognised as the same site.
type spec struct {
	strings []string
	bools   []string
	arrays  []string
	key     []string
}

var specs = map[string]spec{
	"edges":  {strings: []string{"language", "caller", "callee"}, bools: []string{"correct"}, key: []string{"language", "caller", "callee"}},
	"recall": {strings: []string{"language", "construct", "caller", "name", "truth"}, key: []string{"language", "caller", "name"}},
	"oracle": {strings: []string{"language"}, arrays: []string{"covered_callers", "uses"}, key: []string{"language"}},
	"dead":   {strings: []string{"language", "id"}, bools: []string{"truly_dead"}, key: []string{"language", "id"}},
}

// row is one label entry as a canonical string, so two copies compare equal
// exactly when every field (known or not) is equal.
type row struct {
	key   string
	canon string
	obj   map[string]any
}

// source is one input file's rows for one site.
type source struct {
	file string
	rows []row
}

// Options controls a merge.
type Options struct {
	// Private requires every dead label to carry a valid `symbol_hash` and an
	// evidence `category`, as the runner does for any corpus but DevCouncil's.
	Private bool
}

// Merge reads every labels-<lang>[-rN].json in dir and returns the merged
// runner input, plus a per-language count of the entries it holds.
//
// It refuses rather than choosing: a site two files label differently is an
// error naming both files, never last-wins; a malformed file, a file whose
// rows are in another language than its name says, and an empty directory are
// errors too.
func Merge(dir string, opts Options) ([]byte, map[string]int, error) {
	files, err := inputFiles(dir)
	if err != nil {
		return nil, nil, err
	}
	// section -> site key -> the files that label it.
	bySite := map[string]map[string][]source{}
	for _, s := range sections {
		bySite[s] = map[string][]source{}
	}
	for _, f := range files {
		raw, err := readBounded(filepath.Join(dir, f.name), f.name)
		if err != nil {
			return nil, nil, err
		}
		perSection, err := parseFile(f.name, f.language, raw, opts)
		if err != nil {
			return nil, nil, err
		}
		for _, s := range sections {
			grouped := map[string][]row{}
			for _, r := range perSection[s] {
				grouped[r.key] = append(grouped[r.key], r)
			}
			for key, rows := range grouped {
				bySite[s][key] = append(bySite[s][key], source{file: f.name, rows: rows})
			}
		}
	}

	out := map[string][]map[string]any{}
	counts := map[string]int{}
	for _, s := range sections {
		keys := make([]string, 0, len(bySite[s]))
		for k := range bySite[s] {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		out[s] = []map[string]any{}
		for _, key := range keys {
			sources := bySite[s][key]
			for _, other := range sources[1:] {
				if !sameRows(sources[0].rows, other.rows) {
					return nil, nil, fmt.Errorf("%s: %s site %s is labelled differently in %s and %s; resolve it in the label store",
						dir, s, describe(key), sources[0].file, other.file)
				}
			}
			for _, r := range sources[0].rows {
				out[s] = append(out[s], r.obj)
				counts[fmt.Sprint(r.obj["language"])]++
			}
		}
	}
	total := 0
	for _, n := range counts {
		total += n
	}
	if total == 0 {
		return nil, nil, fmt.Errorf("%s: %d label file(s) hold no labels at all; refusing to write an empty labels file", dir, len(files))
	}
	encoded, err := encode(out)
	if err != nil {
		return nil, nil, err
	}
	return encoded, counts, nil
}

type inputFile struct {
	name     string
	language string
	round    int
}

// inputFiles lists dir's label files in a fixed order (language, round, name).
// A `labels-*.json` that does not follow the naming is an error: a misnamed
// round must not be skipped silently.
func inputFiles(dir string) ([]inputFile, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	var files []inputFile
	for _, e := range entries {
		name := e.Name()
		lower := strings.ToLower(name)
		if e.IsDir() || !strings.HasPrefix(lower, "labels-") || !strings.HasSuffix(lower, ".json") {
			continue
		}
		// A case variant (`Labels-go.json`, `labels-go.JSON`) is a label file
		// someone meant to include; on a case-insensitive filesystem it is
		// also the same file as the lower-case spelling. Never skip it.
		if name != lower {
			return nil, fmt.Errorf("%s: %s is a case variant of a label file name; rename it to %s", dir, name, lower)
		}
		m := fileName.FindStringSubmatch(name)
		if m == nil {
			return nil, fmt.Errorf("%s: %s does not match labels-<lang>[-rN].json", dir, name)
		}
		round := 0
		if m[2] != "" {
			if _, err := fmt.Sscanf(m[2], "%d", &round); err != nil {
				return nil, fmt.Errorf("%s: %s: bad round %q", dir, name, m[2])
			}
		}
		files = append(files, inputFile{name: name, language: m[1], round: round})
	}
	if len(files) == 0 {
		return nil, fmt.Errorf("%s: no labels-<lang>[-rN].json files to merge", dir)
	}
	sort.Slice(files, func(i, j int) bool {
		a, b := files[i], files[j]
		if a.language != b.language {
			return a.language < b.language
		}
		if a.round != b.round {
			return a.round < b.round
		}
		return a.name < b.name
	})
	return files, nil
}

// parseFile decodes one label file into canonical rows per section.
func parseFile(name, language string, raw []byte, opts Options) (map[string][]row, error) {
	if err := noDuplicateKeys(raw); err != nil {
		return nil, fmt.Errorf("%s: %w", name, err)
	}
	var top map[string]json.RawMessage
	if err := decodeStrict(raw, &top); err != nil {
		return nil, fmt.Errorf("%s: not a JSON object of label arrays: %w", name, err)
	}
	if top == nil {
		return nil, fmt.Errorf("%s: is JSON null, not an object of label arrays", name)
	}
	for k := range top {
		if _, ok := specs[k]; !ok {
			return nil, fmt.Errorf("%s: unknown top-level key %q (the runner reads edges, recall, oracle, dead)", name, k)
		}
	}
	out := map[string][]row{}
	for _, s := range sections {
		msg, ok := top[s]
		if !ok {
			continue
		}
		var entries []map[string]any
		if err := decodeStrict(msg, &entries); err != nil {
			return nil, fmt.Errorf("%s: %q is not an array of objects: %w", name, s, err)
		}
		if entries == nil {
			return nil, fmt.Errorf("%s: %q is null, not an array", name, s)
		}
		seen := map[string]int{}
		for i, obj := range entries {
			r, err := checkEntry(s, obj)
			if err != nil {
				return nil, fmt.Errorf("%s: %s[%d]: %w", name, s, i, err)
			}
			if got := obj["language"]; got != language {
				return nil, fmt.Errorf("%s: %s[%d] is labelled language %v, but the file is named for %s", name, s, i, got, language)
			}
			if s == "dead" {
				if err := checkDead(obj, opts); err != nil {
					return nil, fmt.Errorf("%s: dead[%d]: %w", name, i, err)
				}
			}
			// One file says one thing about a site. Two recall rows may share
			// (caller, name) - two call sites of one name - but not the truth.
			site := r.key
			if s == "recall" {
				site += obj["truth"].(string)
			}
			if first, dup := seen[site]; dup {
				kind := "conflicts with"
				if out[s][first].canon == r.canon {
					kind = "duplicates"
				}
				return nil, fmt.Errorf("%s: %s[%d] %s %s[%d] (same site %s)", name, s, i, kind, s, first, describe(r.key))
			}
			seen[site] = len(out[s])
			out[s] = append(out[s], r)
		}
	}
	return out, nil
}

// checkEntry verifies an entry's required fields and builds its row.
func checkEntry(section string, obj map[string]any) (row, error) {
	sp := specs[section]
	for _, f := range sp.strings {
		if _, ok := obj[f].(string); !ok {
			return row{}, fmt.Errorf("has no string %q", f)
		}
	}
	for _, f := range sp.bools {
		if _, ok := obj[f].(bool); !ok {
			return row{}, fmt.Errorf("has no bool %q", f)
		}
	}
	for _, f := range sp.arrays {
		if _, ok := obj[f].([]any); !ok {
			return row{}, fmt.Errorf("has no array %q", f)
		}
	}
	canon, err := encodeCompact(obj)
	if err != nil {
		return row{}, err
	}
	parts := make([]string, len(sp.key))
	for i, f := range sp.key {
		parts[i] = obj[f].(string)
	}
	key, err := encodeCompact(parts)
	if err != nil {
		return row{}, err
	}
	return row{key: key, canon: canon, obj: obj}, nil
}

// checkDead applies the runner's identity rules to a dead label: a present
// `symbol_hash` must have the hash shape; with Private it and a known
// evidence `category` are required.
func checkDead(obj map[string]any, opts Options) error {
	hash, hasHash := obj["symbol_hash"]
	if hasHash {
		s, ok := hash.(string)
		if !ok || !identityHash.MatchString(s) {
			return fmt.Errorf("`symbol_hash` is not `h:` and twelve lowercase hex digits")
		}
	}
	if !opts.Private {
		return nil
	}
	if !hasHash {
		return fmt.Errorf("`symbol_hash` is missing (required for a private corpus)")
	}
	cat, _ := obj["category"].(string)
	for _, c := range evidenceCategories {
		if cat == c {
			return nil
		}
	}
	return fmt.Errorf("`category` is missing or is not an evidence category (required for a private corpus)")
}

func sameRows(a, b []row) bool {
	if len(a) != len(b) {
		return false
	}
	ca, cb := canons(a), canons(b)
	for i := range ca {
		if ca[i] != cb[i] {
			return false
		}
	}
	return true
}

func canons(rows []row) []string {
	out := make([]string, len(rows))
	for i, r := range rows {
		out[i] = r.canon
	}
	sort.Strings(out)
	return out
}

// describe renders a site key (a JSON array of strings) for an error.
func describe(key string) string {
	var parts []string
	if err := json.Unmarshal([]byte(key), &parts); err != nil {
		return key
	}
	return strings.Join(parts, " | ")
}

// maxInputBytes bounds one label file; a variable so a test can lower it.
var maxInputBytes int64 = 64 << 20

// readBounded reads a label file of at most maxInputBytes.
func readBounded(path, name string) ([]byte, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	raw, err := io.ReadAll(io.LimitReader(f, maxInputBytes+1))
	if err != nil {
		return nil, fmt.Errorf("%s: %w", name, err)
	}
	if int64(len(raw)) > maxInputBytes {
		return nil, fmt.Errorf("%s: larger than the %d MiB limit for one label file", name, maxInputBytes>>20)
	}
	return raw, nil
}

// noDuplicateKeys walks raw's JSON and refuses an object that repeats a key:
// encoding/json keeps the last one silently, so the file would say two things
// about one field and the merge would pick one without telling.
func noDuplicateKeys(raw []byte) error {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	return walkKeys(dec, 0)
}

// maxDepth bounds the nesting walkKeys follows, so a hostile file cannot
// exhaust the stack.
const maxDepth = 256

func walkKeys(dec *json.Decoder, depth int) error {
	if depth > maxDepth {
		return fmt.Errorf("JSON nested deeper than %d levels", maxDepth)
	}
	tok, err := dec.Token()
	if err != nil {
		return err
	}
	delim, ok := tok.(json.Delim)
	if !ok {
		return nil
	}
	switch delim {
	case '{':
		seen := map[string]bool{}
		for dec.More() {
			keyTok, err := dec.Token()
			if err != nil {
				return err
			}
			key, _ := keyTok.(string)
			if seen[key] {
				return fmt.Errorf("duplicate JSON key %q in one object", key)
			}
			seen[key] = true
			if err := walkKeys(dec, depth+1); err != nil {
				return err
			}
		}
	case '[':
		for dec.More() {
			if err := walkKeys(dec, depth+1); err != nil {
				return err
			}
		}
	}
	_, err = dec.Token() // the closing delimiter
	return err
}

// decodeStrict decodes exactly one JSON value, keeping numbers exact.
func decodeStrict(raw []byte, v any) error {
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if err := dec.Decode(v); err != nil {
		return err
	}
	if dec.More() {
		return fmt.Errorf("trailing data after the JSON value")
	}
	return nil
}

func encodeCompact(v any) (string, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return "", err
	}
	return strings.TrimSuffix(buf.String(), "\n"), nil
}

// encode is the merged labels as indented JSON with a trailing newline: the
// same bytes for the same inputs (map keys encode sorted).
func encode(out map[string][]map[string]any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetIndent("", "  ")
	enc.SetEscapeHTML(false)
	if err := enc.Encode(out); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}
