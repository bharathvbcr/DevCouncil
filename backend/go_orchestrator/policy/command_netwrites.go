package policy

import (
	"net/url"
	"path"
	"strings"
)

// This file reads the files curl and wget, shred, chmod, chown and chgrp write
// from their arguments. They are the ordinary way an agent puts bytes into a
// path without a redirection: `curl -o .env URL`, `wget -O .git/hooks/x URL`,
// `shred -u .env`, `chmod +x .git/hooks/pre-commit`. Each returns what
// command_filewrites.go's argWrites carries, so the write evaluator judges
// them as it judges a redirection.

// curlValueShort are curl's short options that take a value, attached or in
// the next word; curlValueLong the long ones that take it in the next word.
// A long option missing from the list leaves its value among the operands,
// where it is read as a URL — which judges one name too many, never one too few.
const curlValueShort = "AbcCdDeEFHKmoPQrtTuUwxXyYz"

var curlValueLong = []string{
	"output", "output-dir", "dump-header", "cookie-jar", "cookie", "config", "trace", "trace-ascii",
	"stderr", "libcurl", "etag-save", "etag-compare", "hsts", "alt-svc", "header", "data", "data-raw",
	"data-binary", "data-ascii", "data-urlencode", "form", "form-string", "user", "user-agent",
	"request", "referer", "range", "proxy", "proxy-user", "max-time", "connect-timeout", "retry",
	"retry-delay", "retry-max-time", "write-out", "upload-file", "url", "cacert", "capath", "cert",
	"key", "resolve", "connect-to", "interface", "limit-rate", "max-filesize", "max-redirs",
	"continue-at", "time-cond", "oauth2-bearer", "unix-socket", "json", "variable", "expand-url",
}

// curlWrites reads curl: every -o/--output file, the files --dump-header,
// --cookie-jar, --trace and their kin write, and — with -O/--remote-name —
// the name each URL ends in, in the current directory or --output-dir. -K
// reads more options, including more outputs, from a file nobody here reads.
func curlWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, curlValueShort, curlValueLong...)
	var w argWrites
	dir, _ := p.value("--output-dir")
	inDir := func(file string) string {
		if dir == "" || path.IsAbs(file) {
			return file
		}
		return strings.TrimRight(dir, "/") + "/" + file
	}
	for _, out := range p.values("-o", "--output") {
		if out != "" && out != "-" {
			w.merge(literalWrites([]string{inDir(out)}))
		}
	}
	for _, opt := range []string{"-D", "--dump-header", "-c", "--cookie-jar", "--trace", "--trace-ascii",
		"--stderr", "--libcurl", "--etag-save", "--hsts", "--alt-svc"} {
		for _, file := range p.values(opt) {
			if file != "" && file != "-" {
				w.merge(literalWrites([]string{file}))
			}
		}
	}
	if p.has("-O", "--remote-name", "--remote-name-all") {
		urls := append(append([]string(nil), p.operands...), p.values("--url")...)
		for _, u := range urls {
			if name := urlFileName(u); name != "" {
				w.merge(literalWrites([]string{inDir(name)}))
			}
		}
	}
	if p.has("-K", "--config") {
		w.unjudged = "`curl --config` reads more options, output files included, from a file"
	}
	return w
}

// wgetValueShort are wget's short options that take a value.
const wgetValueShort = "aABDeiIlOoPQRtTUwXx"

var wgetValueLong = []string{
	"output-document", "output-file", "append-output", "directory-prefix", "input-file", "execute",
	"user-agent", "header", "tries", "timeout", "wait", "waitretry", "limit-rate", "quota",
	"accept", "reject", "include-directories", "exclude-directories", "domains", "exclude-domains",
	"base", "level", "user", "password", "http-user", "http-password", "ca-certificate",
	"certificate", "private-key", "post-data", "post-file", "referer", "load-cookies",
	"save-cookies", "bind-address", "dns-timeout", "connect-timeout", "read-timeout",
}

// wgetWrites reads wget, which always writes: the -O file, or a file named
// after each URL in the current directory or -P's. -o/-a name the log it
// writes. A recursive fetch, an input list and an -e command write files
// whose names the line does not give.
func wgetWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, wgetValueShort, wgetValueLong...)
	var w argWrites
	for _, opt := range []string{"-o", "--output-file", "-a", "--append-output", "--save-cookies"} {
		for _, file := range p.values(opt) {
			if file != "" && file != "-" {
				w.merge(literalWrites([]string{file}))
			}
		}
	}
	for _, setting := range p.values("-e", "--execute") {
		key, _, _ := strings.Cut(strings.ReplaceAll(setting, " ", ""), "=")
		switch strings.ReplaceAll(strings.ToLower(key), "-", "_") {
		case "output_document", "dir_prefix", "logfile", "dirstruct", "input", "save_cookies":
			w.unjudged = "`wget -e " + key + "` redirects where wget writes"
		}
	}
	if p.has("-r", "--recursive", "-m", "--mirror", "-i", "--input-file", "-p", "--page-requisites") {
		w.unjudged = "`wget` in recursive or list mode writes every file it finds"
	}
	if out, ok := p.value("-O", "--output-document"); ok {
		if out != "" && out != "-" {
			w.merge(literalWrites([]string{out}))
		}
		return w
	}
	prefix, _ := p.value("-P", "--directory-prefix")
	for _, u := range p.operands {
		name := urlFileName(u)
		if name == "" {
			name = "index.html"
		}
		if prefix != "" {
			name = strings.TrimRight(prefix, "/") + "/" + name
		}
		w.merge(literalWrites([]string{name}))
	}
	return w
}

// urlFileName is the last path component of a URL — what curl -O and wget
// name the file they save — or "" when the URL ends in a slash or has no path.
func urlFileName(raw string) string {
	if raw == "" || unresolvableWord(raw) {
		return ""
	}
	rest := raw
	if i := strings.Index(rest, "://"); i >= 0 {
		rest = rest[i+3:]
		if j := strings.IndexByte(rest, '/'); j >= 0 {
			rest = rest[j:]
		} else {
			return ""
		}
	}
	if i := strings.IndexAny(rest, "?#"); i >= 0 {
		rest = rest[:i]
	}
	if strings.HasSuffix(rest, "/") {
		return ""
	}
	name := path.Base(rest)
	if decoded, err := url.PathUnescape(name); err == nil {
		name = decoded
	}
	if name == "." || name == "/" || name == ".." {
		return ""
	}
	return name
}

// shredWrites reads shred: it overwrites every file operand, and with -u or
// --remove deletes it afterwards.
func shredWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, "ns", "iterations", "size", "random-source")
	var files []string
	for _, f := range p.operands {
		if f != "-" {
			files = append(files, f)
		}
	}
	if p.has("-u", "--remove") {
		return literalDeletes(files)
	}
	for opt := range p.opts {
		if strings.HasPrefix(opt, "--remove") {
			return literalDeletes(files)
		}
	}
	return literalWrites(files)
}

// ownerWrites reads chmod, chown and chgrp: every operand after the mode or
// owner is a file whose metadata changes — and `chmod +x` on a hook is what
// makes a planted hook run. A symbolic mode can start with "-" (`chmod -x f`),
// so a leading word made only of mode letters is the mode, not an option.
func ownerWrites(args []shellWord) argWrites {
	var operands []string
	haveReference, endOfOptions := false, false
	for _, a := range args {
		w := a.text
		switch {
		case endOfOptions || w == "" || w[0] != '-' || w == "-":
			operands = append(operands, w)
		case w == "--":
			endOfOptions = true
		case strings.HasPrefix(w, "--"):
			if name, _, _ := strings.Cut(w[2:], "="); name == "reference" {
				haveReference = true
			}
		case strings.Trim(w[1:], "RvfchHLPn") == "":
			// a flag cluster
		default:
			operands = append(operands, w)
		}
	}
	if !haveReference && len(operands) > 0 {
		operands = operands[1:]
	}
	return literalWrites(operands)
}
