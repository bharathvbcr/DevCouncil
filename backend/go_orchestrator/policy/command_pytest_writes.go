package policy

import (
	"path"
	"strings"
)

// pytestWriteOptions are the long options of pytest and its common reporting
// plugins that write the file or directory their value names. Matched as
// prefixes, because argparse accepts any unambiguous abbreviation:
// `--junitx=.env` writes .env exactly as `--junitxml=.env` does. A prefix that
// argparse would reject as ambiguous is read as a write too, which is the safe
// direction: it would not have run.
var pytestWriteOptions = []string{
	"--junitxml", "--junit-xml", "--basetemp", "--resultlog", "--result-log",
	"--report-log", "--log-file", "--cache-dir", "--html", "--json-report-file",
}

// pytestWriteIniKeys are the ini keys, settable with -o, whose value is a file
// or directory pytest writes.
var pytestWriteIniKeys = map[string]bool{"cache_dir": true, "log_file": true}

// pytestShortOptionsWithValue are the short options whose value is the rest of
// the cluster or the next word. Reading a cluster stops at the first one, so
// the `o` inside `-ocache_dir=x` is an option and the one inside `-kfoo` is not.
const pytestShortOptionsWithValue = "ocpkmWnrdx"

// maxPytestAddoptsDepth bounds the re-reading of -o addopts=… and
// PYTEST_ADDOPTS, which are themselves pytest command lines.
const maxPytestAddoptsDepth = 4

// pytestWrites returns the files and directories a pytest invocation writes
// through its options, and whether any could not be named.
//
// A test run is not a read-only command, and a lease holder may run one, but
// where it writes is chosen by arguments: a report file, the base temporary
// directory (which pytest empties and recreates), the cache directory, and —
// through --rootdir and -c — where that cache lands. Each is a write like any
// other and is returned for the write gate to judge, so `--junitxml=.env`,
// `--basetemp=.git` and `--junitxml=../outside.xml` meet the secret,
// restricted and outside-root rules instead of passing as "pytest *".
//
// Argument files (@file) and an addopts value that is not a literal are
// reported as opaque: their content decides what is written, and the gate does
// not hold it.
func pytestWrites(words []shellWord, at int, args []shellWord) ([]string, bool) {
	base := path.Base(words[at].text)
	start := -1
	switch {
	case base == "pytest" || base == "py.test":
		start = 0
	case base == "python" || base == "python3" || strings.HasPrefix(base, "python3.") ||
		base == "uv" || base == "uvx" || base == "poetry" || base == "pipx" || base == "hatch":
		for j, a := range args {
			if a.text != "pytest" && a.text != "py.test" {
				continue
			}
			if strings.HasPrefix(base, "python") && (j == 0 || args[j-1].text != "-m") {
				continue
			}
			start = j + 1
			break
		}
	}
	if start < 0 {
		return nil, false
	}
	targets, opaque := pytestOptionWrites(args[start:], 0)
	for _, w := range words[:at] {
		name, value, ok := strings.Cut(w.text, "=")
		if !ok || !isAssignmentWord(w.text) {
			continue
		}
		switch name {
		case "PYTEST_ADDOPTS":
			t, o := pytestAddoptsWrites(value, 1)
			targets, opaque = append(targets, t...), opaque || o
		case "PYTEST_DEBUG_TEMPROOT":
			t, o := pytestPathWrite(value)
			targets, opaque = append(targets, t...), opaque || o
		}
	}
	return targets, opaque
}

func pytestAddoptsWrites(value string, depth int) ([]string, bool) {
	if depth > maxPytestAddoptsDepth || hasExpansion(value) {
		return nil, true
	}
	return pytestOptionWrites(shellWords(value), depth)
}

// pytestPathWrite names one written path, or reports it opaque.
func pytestPathWrite(p string) ([]string, bool) {
	if unresolvableWord(p) {
		return nil, true
	}
	return []string{p}, false
}

func pytestOptionWrites(args []shellWord, depth int) ([]string, bool) {
	var targets []string
	opaque := false
	add := func(p string) {
		t, o := pytestPathWrite(p)
		targets, opaque = append(targets, t...), opaque || o
	}
	// The cache directory lands under the root directory. Under the current
	// directory that is where it already goes, so only a move is a new write.
	addCacheUnder := func(dir string) {
		if path.Clean(dir) != "." {
			add(path.Join(dir, ".pytest_cache"))
		}
	}
	// value takes an option's value from `=` or from the next word.
	value := func(k *int, attached string, has bool) (string, bool) {
		if has {
			return attached, true
		}
		if *k+1 < len(args) {
			*k++
			return args[*k].text, true
		}
		return "", false
	}
	override := func(kv string) {
		key, val, ok := strings.Cut(kv, "=")
		switch {
		case !ok:
		case key == "addopts":
			t, o := pytestAddoptsWrites(val, depth+1)
			targets, opaque = append(targets, t...), opaque || o
		case pytestWriteIniKeys[key]:
			add(val)
		}
	}
	for k := 0; k < len(args); k++ {
		w := args[k].text
		switch {
		case w == "--":
			return targets, opaque
		case strings.HasPrefix(w, "@"):
			opaque = true
		case strings.HasPrefix(w, "--"):
			name, attached, has := strings.Cut(w, "=")
			switch {
			case name == "--override-ini" || (len(name) >= 5 && strings.HasPrefix("--override-ini", name)):
				if v, ok := value(&k, attached, has); ok {
					override(v)
				}
			case name == "--rootdir" || (len(name) >= 4 && strings.HasPrefix("--rootdir", name)):
				if v, ok := value(&k, attached, has); ok {
					addCacheUnder(v)
				}
			case name == "--cov-report":
				if v, ok := value(&k, attached, has); ok {
					if _, dest, found := strings.Cut(v, ":"); found {
						add(dest)
					}
				}
			default:
				for _, option := range pytestWriteOptions {
					if len(name) >= 3 && strings.HasPrefix(option, name) {
						if v, ok := value(&k, attached, has); ok {
							add(v)
						}
						break
					}
				}
			}
		case len(w) > 1 && w[0] == '-':
			cluster := w[1:]
			for c := 0; c < len(cluster); c++ {
				if !strings.ContainsRune(pytestShortOptionsWithValue, rune(cluster[c])) {
					continue
				}
				rest := cluster[c+1:]
				v, ok := value(&k, rest, rest != "")
				if ok {
					switch cluster[c] {
					case 'o':
						override(v)
					case 'c':
						addCacheUnder(path.Dir(v))
					}
				}
				break
			}
		}
	}
	return targets, opaque
}
