package testsupport

import (
	"bytes"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"unicode/utf8"
)

func TestIsolateHostConfigRemovesEveryOverride(t *testing.T) {
	realSeat := t.TempDir()
	for _, name := range HostConfigEnv {
		t.Setenv(name, realSeat)
	}
	realHome := os.Getenv("HOME")

	t.Run("isolated", func(t *testing.T) {
		cfg := IsolateHostConfig(t)
		if cfg.Home == "" || cfg.Home == realHome || os.Getenv("HOME") != cfg.Home {
			t.Fatalf("HOME not redirected: cfg %q, env %q, was %q", cfg.Home, os.Getenv("HOME"), realHome)
		}
		for _, name := range HostConfigEnv {
			if v, set := os.LookupEnv(name); set {
				t.Errorf("%s is still set to %q", name, v)
			}
		}
	})

	if os.Getenv("HOME") != realHome {
		t.Errorf("HOME not restored: %q", os.Getenv("HOME"))
	}
	for _, name := range HostConfigEnv {
		if os.Getenv(name) != realSeat {
			t.Errorf("%s not restored: %q", name, os.Getenv(name))
		}
	}
}

func TestAssertWithin(t *testing.T) {
	scratch := t.TempDir()
	other := t.TempDir()
	AssertWithin(t, []string{scratch}, "rel/file.json", filepath.Join(scratch, "a", "b"), scratch)

	sub := &recorder{TB: t}
	AssertWithin(sub, []string{scratch}, filepath.Join(other, ".claude.json"), scratch+"-sibling/x")
	if len(sub.errs) != 2 {
		t.Fatalf("want both outside paths reported, got %v", sub.errs)
	}
}

// recorder captures Errorf so the guard's failures can be asserted on.
type recorder struct {
	testing.TB
	errs []string
}

func (r *recorder) Errorf(format string, args ...any) { r.errs = append(r.errs, format) }

// nonConfigEnv are the variables devmap reads that do not locate user-level
// configuration; see HostConfigEnv's audit. HOME is not here: it does locate
// configuration and is handled by IsolateHostConfig itself.
var nonConfigEnv = []string{
	"PATH", "TMPDIR", "TERM", "NO_COLOR", "COLUMNS",
	"DEVMAP_HOME", "RAYON_NUM_THREADS",
	// Project-root hints: repository-scoped, not user-level.
	"CLAUDE_PROJECT_DIR", "CURSOR_PROJECT_DIR",
}

// devmapCrateSources returns the src directory of devmap-cli and of every crate
// it depends on through path dependencies, transitively.
func devmapCrateSources(t *testing.T) []string {
	t.Helper()
	workspace := RustWorkspace(t)
	// Only dependency tables carry "name = { path = ... }"; [[bin]] and [[test]]
	// entries also have a path key, to a source file.
	pathDep := regexp.MustCompile(`(?m)^[A-Za-z0-9_-]+\s*=\s*\{[^}]*\bpath\s*=\s*"([^"]+)"`)
	var srcs []string
	seen := map[string]bool{}
	queue := []string{filepath.Join(workspace, "devmap-cli")}
	for len(queue) > 0 {
		dir := queue[0]
		queue = queue[1:]
		dir = filepath.Clean(dir)
		if seen[dir] {
			continue
		}
		seen[dir] = true
		manifest, err := os.ReadFile(filepath.Join(dir, "Cargo.toml"))
		if err != nil {
			t.Fatalf("read the manifest of a crate devmap-cli depends on: %v", err)
		}
		srcs = append(srcs, filepath.Join(dir, "src"))
		for _, m := range pathDep.FindAllSubmatch(manifest, -1) {
			queue = append(queue, filepath.Join(dir, string(m[1])))
		}
	}
	if len(srcs) < 5 {
		t.Fatalf("followed path dependencies to only %v: the walk is not reading devmap-cli's manifest", srcs)
	}
	return srcs
}

var (
	cfgTestMarker = regexp.MustCompile(`#\[cfg\(test\)\]`)
	// After the marker: more attributes, an optional visibility, then a
	// module with an inline body. "mod tests;" (no brace) is not matched.
	cfgTestModHead = regexp.MustCompile(`^(?:\s|#\[[^\]]*\])*(?:pub(?:\([^)]*\))?\s+)?mod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{`)
)

// stripCfgTestModules removes every #[cfg(test)] module (through its matching
// closing brace) from Rust source. Other #[cfg(test)] items, a const, a field,
// a fn, stay in the text: a test module is not always last in the file, and
// cutting at the first marker hid every literal after a mid-file test item.
func stripCfgTestModules(body []byte) []byte {
	var out []byte
	pos := 0
	for _, loc := range cfgTestMarker.FindAllIndex(body, -1) {
		if loc[0] < pos {
			continue // inside a module already removed
		}
		head := cfgTestModHead.FindIndex(body[loc[1]:])
		if head == nil {
			continue
		}
		end, closed := matchingBrace(body, loc[1]+head[1]-1)
		if !closed {
			continue // fail closed: an unparseable module is scanned, not stripped
		}
		out = append(out, body[pos:loc[0]]...)
		pos = end
	}
	return append(out, body[pos:]...)
}

// matchingBrace returns the index just past the brace that closes the one at
// open, skipping line and block comments, string, raw string and char
// literals; closed is false if the source ends first.
func matchingBrace(b []byte, open int) (end int, closed bool) {
	depth := 0
	for i := open; i < len(b); i++ {
		switch c := b[i]; {
		case c == '/' && i+1 < len(b) && b[i+1] == '/':
			for i < len(b) && b[i] != '\n' {
				i++
			}
		case c == '/' && i+1 < len(b) && b[i+1] == '*':
			nest := 1
			for i += 2; i < len(b) && nest > 0; i++ {
				if b[i] == '/' && i+1 < len(b) && b[i+1] == '*' {
					nest++
					i++
				} else if b[i] == '*' && i+1 < len(b) && b[i+1] == '/' {
					nest--
					i++
				}
			}
			i--
		case c == 'r' && rawStringStart(b, i) > 0:
			hashes := rawStringStart(b, i) - 2 - i
			closer := "\"" + strings.Repeat("#", hashes)
			if k := strings.Index(string(b[i+hashes+2:]), closer); k >= 0 {
				i += hashes + 2 + k + len(closer) - 1
			} else {
				return len(b), false
			}
		case c == '"':
			for i++; i < len(b) && b[i] != '"'; i++ {
				if b[i] == '\\' {
					i++
				}
			}
		case c == '\'':
			if k := charLiteralEnd(b, i); k > 0 {
				i = k
			} // else a lifetime
		case c == '{':
			depth++
		case c == '}':
			depth--
			if depth == 0 {
				return i + 1, true
			}
		}
	}
	return len(b), false
}

// charLiteralEnd returns the index of the closing quote of the char literal
// whose opening quote is at i: one UTF-8 rune or one escape (\n, \', \\,
// \x41, \u{2192}) then a quote. It returns 0 for a lifetime.
func charLiteralEnd(b []byte, i int) int {
	j := i + 1
	if j >= len(b) {
		return 0
	}
	if b[j] == '\\' {
		if j+1 >= len(b) {
			return 0
		}
		switch b[j+1] {
		case 'u':
			if j+2 < len(b) && b[j+2] == '{' {
				k := bytes.IndexByte(b[j+2:], '}')
				if k < 0 {
					return 0
				}
				j += 2 + k + 1
			} else {
				j += 2
			}
		case 'x':
			j += 4
		default:
			j += 2
		}
	} else {
		_, size := utf8.DecodeRune(b[j:])
		j += size
	}
	if j < len(b) && b[j] == '\'' {
		return j
	}
	return 0
}

// rawStringStart returns the index just past the opening quote of a raw string
// (r"..", r#".."#) beginning at i, or 0 if i does not start one.
func rawStringStart(b []byte, i int) int {
	i0 := i
	if i > 0 && (b[i-1] == 'b' || b[i-1] == 'c') {
		i-- // br"..", cr"..": the prefix is part of the literal
	}
	if i > 0 && isIdentByte(b[i-1]) {
		return 0 // the r ends an identifier
	}
	i = i0
	j := i + 1
	for j < len(b) && b[j] == '#' {
		j++
	}
	if j < len(b) && b[j] == '"' {
		return j + 1
	}
	return 0
}

// lexerCase wraps a test-module body that has a lexer hazard in a module that
// is not last in the file, followed by a production literal that must survive.
func lexerCase(hazard string) string {
	return "#[cfg(test)]\nmod tests { fn t() { " + hazard + " \"FAKE_HOME\"; } }\nfn z() { \"AFTER_DIR\"; }\n"
}

func isIdentByte(c byte) bool {
	return c == '_' || c >= '0' && c <= '9' || c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z'
}

func TestStripCfgTestModules(t *testing.T) {
	const midItem = "const A: u8 = 1;\n#[cfg(test)]\nconst X: u8 = 2;\nfn f() { env::var(\"CLAUDE_CONFIG_DIR\"); }\n"
	cases := []struct {
		name, in string
		keep     []string
		drop     []string
	}{
		{"mid-file cfg(test) const keeps later literals", midItem, []string{"CLAUDE_CONFIG_DIR"}, nil},
		{"test module is removed", "fn a() { \"REAL\"; }\n#[cfg(test)]\nmod tests {\n    fn t() { \"FAKE_HOME\"; }\n}\n", []string{"REAL"}, []string{"FAKE_HOME"}},
		{"nested braces and braces in strings", "fn a() { \"REAL\"; }\n#[cfg(test)]\npub(crate) mod tests {\n    fn t() { if x { let s = \"}}{\"; let c = '}'; \"FAKE_HOME\"; } }\n    // } stray\n    fn u() { \"FAKE_DIR\"; }\n}\n", []string{"REAL"}, []string{"FAKE_HOME", "FAKE_DIR"}},
		{"raw string and lifetime in test module", "#[cfg(test)]\n#[allow(dead_code)]\nmod tests {\n    fn t<'a>(x: &'a str) { r#\"}\"#; \"FAKE_HOME\"; }\n}\nfn z() { \"AFTER\"; }\n", []string{"AFTER"}, []string{"FAKE_HOME"}},
		{"test module not last keeps code after it", "#[cfg(test)]\nmod tests { fn t() { \"FAKE_HOME\"; } }\nfn z() { \"AFTER_DIR\"; }\n", []string{"AFTER_DIR"}, []string{"FAKE_HOME"}},
		{"br raw string", lexerCase("let p = br\"C:\\\"; let q = 1;"), []string{"AFTER_DIR"}, []string{"FAKE_HOME"}},
		{"br raw string with hashes", lexerCase("let p = br#\"say \"{\"#;"), []string{"AFTER_DIR"}, []string{"FAKE_HOME"}},
		{"non-ASCII char literals", lexerCase("let p = ['\u2192','{'];"), []string{"AFTER_DIR"}, []string{"FAKE_HOME"}},
		{"escaped quote char literal", lexerCase("let p = '\\'';let q = '{';"), []string{"AFTER_DIR"}, []string{"FAKE_HOME"}},
		{"unicode escape char literal", lexerCase("let p = '\\u{7b}'; let q = '\\x7b';"), []string{"AFTER_DIR"}, []string{"FAKE_HOME"}},
		{"unterminated raw string keeps the module", "#[cfg(test)]\nmod tests { fn t() { r#\"oops; \"KEPT_HOME\"; } }\nfn z() { \"AFTER_DIR\"; }\n", []string{"AFTER_DIR", "KEPT_HOME"}, nil},
		{"external test module has no body to remove", "#[cfg(test)]\nmod tests;\nfn z() { \"AFTER\"; }\n", []string{"AFTER"}, nil},
	}
	for _, c := range cases {
		got := string(stripCfgTestModules([]byte(c.in)))
		for _, k := range c.keep {
			if !strings.Contains(got, k) {
				t.Errorf("%s: %q was removed:\n%s", c.name, k, got)
			}
		}
		for _, d := range c.drop {
			if strings.Contains(got, d) {
				t.Errorf("%s: %q survived:\n%s", c.name, d, got)
			}
		}
	}
}

// Every upper-case string literal in the sources of devmap-cli and its path
// dependencies that looks like a home, config or directory variable must be
// classified: isolated (HostConfigEnv, or HOME), or listed in nonConfigEnv as
// not a config location. Literals are matched wherever they appear, so a read
// through a constant (env::var(CONST)) is caught as well as env::var("NAME").
// An allowlist entry no source mentions any more fails too, so it cannot go
// stale.
func TestHostConfigEnvCoversWhatDevmapReads(t *testing.T) {
	literal := regexp.MustCompile(`"([A-Z][A-Z0-9_]*)"`)
	configLike := regexp.MustCompile(`^[A-Z0-9_]*(HOME|CONFIG|_DIR)(_|$)`)
	seen := map[string]string{}
	for _, src := range devmapCrateSources(t) {
		err := filepath.WalkDir(src, func(path string, d os.DirEntry, err error) error {
			if err != nil || d.IsDir() || !strings.HasSuffix(path, ".rs") {
				return err
			}
			// Test code sets variables for git children (GIT_CONFIG_GLOBAL and
			// friends, which devmap itself never reads) and reads test-only ones
			// (DEVMAP_NON_REFLINK_DIR, CARGO_MANIFEST_DIR); the shipped binary is
			// what has to be isolated.
			if name := d.Name(); name == "tests.rs" || strings.HasSuffix(name, "_tests.rs") {
				return nil
			}
			body, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			body = stripCfgTestModules(body)
			for _, m := range literal.FindAllSubmatch(body, -1) {
				seen[string(m[1])] = path
			}
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
	}
	for _, must := range []string{"HOME", "CLAUDE_CONFIG_DIR"} {
		if _, ok := seen[must]; !ok {
			t.Fatalf("devmap's sources no longer mention %s; fix the scan or HostConfigEnv (saw %d literals)", must, len(seen))
		}
	}
	for name, path := range seen {
		classified := name == "HOME" || slices.Contains(HostConfigEnv, name) || slices.Contains(nonConfigEnv, name)
		if configLike.MatchString(name) && !classified {
			t.Errorf("devmap's sources mention %s (%s), which looks like a config location: add it to HostConfigEnv or to nonConfigEnv", name, path)
		}
	}
	for _, name := range nonConfigEnv {
		if _, ok := seen[name]; !ok {
			t.Errorf("nonConfigEnv lists %s but no devmap source mentions it: remove the stale entry", name)
		}
	}
}

func TestHostConfigChildEnv(t *testing.T) {
	t.Setenv("CLAUDE_CONFIG_DIR", "/real/seat")
	t.Setenv("CODEX_HOME", "/real/codex")
	t.Setenv("HOME", "/real/home")
	t.Setenv("KEEP_ME", "1")
	env := HostConfigChildEnv("/scratch")
	got := map[string]string{}
	for _, kv := range env {
		k, v, _ := strings.Cut(kv, "=")
		got[k] = v
	}
	if got["HOME"] != "/scratch" || got["KEEP_ME"] != "1" {
		t.Errorf("HOME/KEEP_ME wrong: %v", got["HOME"])
	}
	for _, name := range HostConfigEnv {
		if _, ok := got[name]; ok {
			t.Errorf("%s leaked into the child environment", name)
		}
	}
}
