package policy

import (
	"path"
	"strings"
)

// This file reads, from a command's arguments alone, the files rm, truncate,
// touch, install, ln, rsync, patch, dd and perl -i write, and says plainly when
// a command writes files its text does not name (git apply, patch with no file
// operand, xargs and find running a writer). argumentWriteTargets dispatches
// here; the files returned are judged by the same write evaluator as a
// redirection target, so "may this path be written" has one owner.

// argWrites is what one command's arguments say it writes.
type argWrites struct {
	// targets are the literal files written. Every deletion is in here too, so
	// a caller that does not tell the two apart still judges it as a write.
	targets []string
	// deletes are the targets the command removes rather than writes (rm,
	// rmdir, unlink, shred -u, git rm, git clean). A subset of targets: the
	// write evaluator judges them as dc.OpDelete, so a file planned for
	// modification is not also a file the command may remove.
	deletes []string
	// opaque reports a file that is written but whose name only the shell can
	// compute ($VAR, ~, a substitution): refused wherever it appears.
	opaque bool
	// unjudged is non-empty when the command writes files its text does not
	// name — the paths come from a patch body or from another program's output.
	// Nothing can be checked against a task's planned files before it runs, so
	// a caller that has a scope bound refuses; with none, there is no fence to
	// get past and it is left alone.
	unjudged string
}

func (w *argWrites) merge(o argWrites) {
	w.targets = append(w.targets, o.targets...)
	w.deletes = append(w.deletes, o.deletes...)
	w.opaque = w.opaque || o.opaque
	if w.unjudged == "" {
		w.unjudged = o.unjudged
	}
}

func literalWrites(paths []string) argWrites {
	targets, opaque := appendResolvable(nil, paths)
	return argWrites{targets: targets, opaque: opaque}
}

// literalDeletes is literalWrites for a command that removes its operands.
func literalDeletes(paths []string) argWrites {
	w := literalWrites(paths)
	w.deletes = w.targets
	return w
}

// maxWriterNesting bounds xargs/find re-reading the command they run.
const maxWriterNesting = 4

// commandWrites reads one command whose word is words[at]; args are its
// operands with redirections removed.
func commandWrites(words []shellWord, at int, args []shellWord, depth int) argWrites {
	name := path.Base(words[at].text)
	var w argWrites
	switch {
	case isGitWord(words[at].text):
		w = gitWrites(words[at+1:])
	case name == "sed":
		w.targets, w.opaque = sedWrites(args)
	case name == "tee":
		w.targets, w.opaque = teeWrites(args)
	case name == "cp":
		w.targets, w.opaque = copyWrites(args, false)
	case name == "mv":
		w.targets, w.opaque = copyWrites(args, true)
	case name == "rm":
		w = rmWrites(args)
	case name == "sort":
		// sort -o FILE writes FILE, which may also be an input.
		p := parseOptArgs(args, "okStT", "output", "key", "field-separator", "temporary-directory",
			"buffer-size", "parallel", "batch-size", "random-source", "files0-from", "compress-program")
		w = literalWrites(p.values("-o", "--output"))
	case name == "uniq":
		// uniq [INPUT [OUTPUT]]: the second operand is written.
		p := parseOptArgs(args, "fsw", "skip-fields", "skip-chars", "check-chars", "group")
		if len(p.operands) > 1 && p.operands[1] != "-" {
			w = literalWrites(p.operands[1:2])
		}
	case name == "rmdir":
		w = literalDeletes(parseOptArgs(args, "").operands)
	case name == "unlink":
		w = literalDeletes(parseOptArgs(args, "").operands)
	case name == "shred":
		w = shredWrites(args)
	case name == "mkdir":
		w = literalWrites(parseOptArgs(args, "mZ", "mode", "context").operands)
	case name == "chmod" || name == "chown" || name == "chgrp":
		w = ownerWrites(args)
	case name == "curl":
		w = curlWrites(args)
	case name == "wget":
		w = wgetWrites(args)
	case name == "ruby":
		w.targets, w.opaque, w.unjudged = rubyWrites(args)
	case name == "truncate":
		w = literalWrites(parseOptArgs(args, "sr", "size", "reference").operands)
	case name == "touch":
		w = literalWrites(touchOperands(args))
	case name == "install":
		w = installWrites(args)
	case name == "ln":
		w = lnWrites(args)
	case name == "rsync":
		w = rsyncWrites(args)
	case name == "patch":
		w = patchWrites(args)
	case name == "dd":
		w = ddWrites(args)
	case name == "perl":
		w.targets, w.opaque = perlWrites(args)
	case name == "xargs":
		w = xargsWrites(args, depth)
	case name == "find":
		w = findWrites(args, depth)
	default:
		w.targets, w.opaque = pytestWrites(words, at, args)
	}
	return w
}

// optArgs is an argument list split into options and operands.
type optArgs struct {
	operands []string
	// opts maps each option seen — "-x" or "--name" — to its value, "" for a
	// flag. Repeated options keep the last.
	opts map[string]string
	// all keeps every value of an option given more than once, in order:
	// `curl -o a URL -o .env URL` writes both.
	all map[string][]string
	// missing reports an option that needed a value and had none.
	missing bool
}

// parseOptArgs splits args the way getopt does: clusters of short options, a
// value attached or in the next word for the letters in shortValue, long
// options with their value after = or, for the names in longValue, in the next
// word, and "--" ending the options. A lone "-" is an operand.
func parseOptArgs(args []shellWord, shortValue string, longValue ...string) optArgs {
	out := optArgs{opts: map[string]string{}, all: map[string][]string{}}
	endOfOptions := false
	for k := 0; k < len(args); k++ {
		w := args[k].text
		if endOfOptions || w == "-" || !strings.HasPrefix(w, "-") {
			out.operands = append(out.operands, w)
			continue
		}
		if w == "--" {
			endOfOptions = true
			continue
		}
		if long, ok := strings.CutPrefix(w, "--"); ok {
			name, value, hasValue := strings.Cut(long, "=")
			if !hasValue && containsString(longValue, name) {
				if k+1 >= len(args) {
					out.missing = true
				} else {
					k++
					value = args[k].text
				}
			}
			out.opts["--"+name] = value
			out.all["--"+name] = append(out.all["--"+name], value)
			continue
		}
		cluster := w[1:]
		for c := 0; c < len(cluster); c++ {
			if strings.IndexByte(shortValue, cluster[c]) < 0 {
				out.opts["-"+string(cluster[c])] = ""
				out.all["-"+string(cluster[c])] = append(out.all["-"+string(cluster[c])], "")
				continue
			}
			value := cluster[c+1:]
			if value == "" {
				if k+1 >= len(args) {
					out.missing = true
				} else {
					k++
					value = args[k].text
				}
			}
			out.opts["-"+string(cluster[c])] = value
			out.all["-"+string(cluster[c])] = append(out.all["-"+string(cluster[c])], value)
			break
		}
	}
	return out
}

func containsString(list []string, s string) bool {
	for _, item := range list {
		if item == s {
			return true
		}
	}
	return false
}

func (o optArgs) has(names ...string) bool {
	for _, n := range names {
		if _, ok := o.opts[n]; ok {
			return true
		}
	}
	return false
}

// values returns every value the named options were given, in command-line
// order within each name.
func (o optArgs) values(names ...string) []string {
	var out []string
	for _, n := range names {
		out = append(out, o.all[n]...)
	}
	return out
}

func (o optArgs) value(names ...string) (string, bool) {
	for _, n := range names {
		if v, ok := o.opts[n]; ok {
			return v, true
		}
	}
	return "", false
}

// rmWrites reads rm. A pattern operand to a recursive rm reaches whatever the
// shell expands it to and everything beneath that, none of which the pattern
// names, so it is unjudged rather than judged as the one literal path.
func rmWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, "")
	w := literalDeletes(p.operands)
	if p.has("-r", "-R", "--recursive") {
		for _, operand := range p.operands {
			if strings.ContainsAny(operand, "*?[") {
				w.unjudged = "recursive `rm` of the pattern `" + operand + "` removes everything it expands to"
				break
			}
		}
	}
	return w
}

// touchOperands are touch's files: -d, -t and -r take a value, and "-" is
// standard output.
func touchOperands(args []shellWord) []string {
	parsed := parseOptArgs(args, "dtr", "date", "reference")
	var files []string
	for _, f := range parsed.operands {
		if f != "-" {
			files = append(files, f)
		}
	}
	return files
}

// installWrites reads install: with -d every operand is a directory it
// creates; otherwise it copies like cp, with -m, -o, -g and -S taking a value.
func installWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, "gmoSt", "group", "mode", "owner", "suffix", "target-directory", "strip-program")
	if p.missing {
		return argWrites{}
	}
	if p.has("-d", "--directory") {
		return literalWrites(p.operands)
	}
	dir, haveDir := p.value("-t", "--target-directory")
	targets, opaque := copyTargets(p.operands, haveDir, dir, p.has("-T", "--no-target-directory"), false, false)
	return argWrites{targets: targets, opaque: opaque}
}

// lnWrites reads ln, which names its link like cp names a copy — and with one
// operand makes the link in the current directory.
func lnWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, "St", "suffix", "target-directory")
	if p.missing {
		return argWrites{}
	}
	dir, haveDir := p.value("-t", "--target-directory")
	targets, opaque := copyTargets(p.operands, haveDir, dir, p.has("-T", "--no-target-directory"), false, true)
	w := argWrites{targets: targets, opaque: opaque}
	// A hard link gives the source's inode a second, writable name, so the source
	// of a non-symbolic ln is judged as a write when it is a prompt input:
	// `ln AGENTS.md notes.txt` would otherwise hand the agent a name for it that
	// no rule spells. Only prompt-input sources: judging every source would
	// refuse hard-linking an unplanned file, a scope question this change does not
	// own (TestFileWriterTargets pins ln's targets). Writes through any such
	// alias are refused regardless, by identity (reachesRootPromptInput).
	if !p.has("-s", "--symbolic") {
		sources := p.operands
		if !haveDir && len(sources) > 1 {
			sources = sources[:len(sources)-1]
		}
		var inputs []string
		for _, s := range sources {
			if IsPromptInput(s) {
				inputs = append(inputs, s)
			}
		}
		w.merge(literalWrites(inputs))
	}
	return w
}

// rsyncValueOptions are rsync's long options whose value may be the next word.
var rsyncValueOptions = []string{
	"rsh", "exclude", "include", "exclude-from", "include-from", "filter", "files-from",
	"port", "bwlimit", "chmod", "backup-dir", "temp-dir", "partial-dir", "block-size",
	"max-size", "min-size", "timeout", "contimeout", "log-file", "log-file-format",
	"compare-dest", "copy-dest", "link-dest", "rsync-path", "out-format", "suffix",
	"info", "debug", "usermap", "groupmap", "chown", "write-batch", "only-write-batch",
	"read-batch", "modify-window", "checksum-choice", "cc", "compress-level",
	"skip-compress", "address", "sockopts", "outbuf", "iconv", "protocol",
	"stop-after", "stop-at", "max-alloc", "early-input", "password-file",
}

// rsyncWrites reads rsync: the destination — or DEST/basename for each source
// when there are several — plus the sources it removes with
// --remove-source-files, and the files its options write (--backup-dir,
// --temp-dir, --log-file, batch files). A remote destination writes no local
// file. A source ending in "/" copies its contents into the destination
// itself, so the destination directory is what is written.
func rsyncWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, "efTBM", rsyncValueOptions...)
	var w argWrites
	for _, opt := range []string{"--backup-dir", "--temp-dir", "-T", "--log-file", "--write-batch", "--only-write-batch"} {
		if v, ok := p.opts[opt]; ok && v != "" {
			w.merge(literalWrites([]string{v}))
		}
	}
	if p.missing || len(p.operands) < 2 {
		return w
	}
	sources := p.operands[:len(p.operands)-1]
	dest := p.operands[len(p.operands)-1]
	if p.has("--remove-source-files") {
		var local []string
		for _, s := range sources {
			if !rsyncRemote(s) {
				local = append(local, s)
			}
		}
		w.merge(literalWrites(local))
	}
	if rsyncRemote(dest) {
		return w
	}
	if unresolvableWord(dest) {
		w.opaque = true
		return w
	}
	dir := strings.TrimRight(dest, "/")
	if dir == "" {
		dir = dest
	}
	if len(sources) == 1 && !strings.HasSuffix(sources[0], "/") && !strings.HasSuffix(dest, "/") {
		w.targets = append(w.targets, dest)
		return w
	}
	for _, s := range sources {
		if strings.HasSuffix(s, "/") {
			w.targets = append(w.targets, dir)
			continue
		}
		base := path.Base(rsyncPathPart(s))
		if unresolvableWord(base) {
			w.opaque = true
			continue
		}
		w.targets = append(w.targets, dir+"/"+base)
	}
	return w
}

// rsyncRemote reports whether an operand is host:path or an rsync:// URL.
func rsyncRemote(operand string) bool {
	if strings.HasPrefix(operand, "rsync://") {
		return true
	}
	i := strings.Index(operand, ":")
	return i > 0 && !strings.Contains(operand[:i], "/")
}

func rsyncPathPart(operand string) string {
	if rsyncRemote(operand) {
		return operand[strings.Index(operand, ":")+1:]
	}
	return operand
}

// patchWrites reads patch. The file it changes is its file operand, or -o's
// output; with neither, the files are named inside the patch, which this scan
// does not read. --dry-run and --check write nothing, and -d changes the
// directory every name is relative to.
func patchWrites(args []shellWord) argWrites {
	p := parseOptArgs(args, "piodrBVzFDg", "strip", "input", "output", "directory",
		"reject-file", "prefix", "suffix", "version-control", "fuzz", "ifdef", "get")
	if p.has("--dry-run", "--check", "-C") {
		return argWrites{}
	}
	if p.has("-d", "--directory") {
		return argWrites{unjudged: "`patch --directory` changes where the patch's file names are resolved"}
	}
	var w argWrites
	if r, ok := p.value("-r", "--reject-file"); ok && r != "" && r != "-" {
		w.merge(literalWrites([]string{r}))
	}
	if out, ok := p.value("-o", "--output"); ok {
		if out != "" && out != "-" {
			w.merge(literalWrites([]string{out}))
		}
		return w
	}
	if len(p.operands) > 0 {
		w.merge(literalWrites(p.operands[:1]))
		return w
	}
	w.unjudged = "`patch` with no file operand changes the files named inside the patch"
	return w
}

// ddWrites reads dd's of= operand.
func ddWrites(args []shellWord) argWrites {
	var w argWrites
	for _, a := range args {
		if out, ok := strings.CutPrefix(a.text, "of="); ok && out != "" {
			w.merge(literalWrites([]string{out}))
		}
	}
	return w
}

// perlWrites reads perl -i: every file operand is rewritten in place, and a
// backup is kept beside each when the -i suffix is given. Without -e/-E the
// first operand is the script, not a file. Perl reads a cluster left to right
// and -i takes the rest of it as its suffix, so `-pi.bak` and `-pie` both have
// one.
func perlWrites(args []shellWord) ([]string, bool) {
	targets, opaque, _ := inPlaceWrites(args, perlFlavor)
	return targets, opaque
}

// rubyWrites reads ruby -i, which rewrites its file operands exactly as
// perl -i does. -C changes the directory every operand is resolved against,
// which is the one thing the operands alone cannot say.
func rubyWrites(args []shellWord) ([]string, bool, string) {
	return inPlaceWrites(args, rubyFlavor)
}

// inPlaceFlavor names the option letters that matter to reading an in-place
// interpreter command line: the ones whose argument is the script (so the
// first operand is a file), the ones that take the next word when they end a
// cluster, and the ones that take the rest of the cluster whatever it holds.
type inPlaceFlavor struct {
	script, nextValue, restValue string
	// chdir is the option that changes the working directory, or 0.
	chdir byte
}

var (
	perlFlavor = inPlaceFlavor{script: "eE", nextValue: "I", restValue: "FMmCdDVx"}
	rubyFlavor = inPlaceFlavor{script: "e", nextValue: "rIEC", restValue: "Kx", chdir: 'C'}
)

func inPlaceWrites(args []shellWord, flavor inPlaceFlavor) ([]string, bool, string) {
	inPlace, haveScript, endOfOptions := false, false, false
	unjudged := ""
	suffix := ""
	var operands []string
	for k := 0; k < len(args); k++ {
		w := args[k].text
		if endOfOptions || w == "-" || !strings.HasPrefix(w, "-") {
			operands = append(operands, w)
			continue
		}
		if w == "--" {
			endOfOptions = true
			continue
		}
		if strings.HasPrefix(w, "--") {
			continue
		}
		cluster := w[1:]
	scan:
		for c := 0; c < len(cluster); c++ {
			ch := cluster[c]
			switch {
			case ch == 'i':
				inPlace = true
				suffix = cluster[c+1:]
				break scan
			case strings.IndexByte(flavor.script, ch) >= 0:
				haveScript = true
				if c+1 == len(cluster) {
					k++
				}
				break scan
			case strings.IndexByte(flavor.nextValue, ch) >= 0:
				if flavor.chdir != 0 && ch == flavor.chdir {
					unjudged = "`-" + string(ch) + "` changes the directory the files are resolved in"
				}
				if c+1 == len(cluster) {
					k++
				}
				break scan
			case ch == '0' || ch == 'l':
				for c+1 < len(cluster) && cluster[c+1] >= '0' && cluster[c+1] <= '7' {
					c++
				}
			case strings.IndexByte(flavor.restValue, ch) >= 0:
				break scan
			}
		}
	}
	if !inPlace {
		return nil, false, ""
	}
	if !haveScript && len(operands) > 0 {
		operands = operands[1:]
	}
	var targets []string
	opaque := suffix != "" && unresolvableWord(suffix)
	for _, file := range operands {
		if unresolvableWord(file) {
			opaque = true
			continue
		}
		targets = append(targets, file)
		if suffix != "" && !opaque {
			targets = append(targets, sedBackupName(file, suffix))
		}
	}
	return targets, opaque, unjudged
}

// gitApplyUnjudged says why `git apply` cannot be judged, or "" when the
// command is not an apply or only reads the patch. What apply changes is
// named inside the patch, which is not on the command line.
func gitApplyUnjudged(args []shellWord) string {
	if gitSubcommand(args) != "apply" {
		return ""
	}
	readOnly := false
	for _, a := range args {
		switch a.text {
		case "--apply":
			return "`git apply` changes the files named inside the patch"
		case "--check", "--stat", "--numstat", "--summary":
			readOnly = true
		}
	}
	if readOnly {
		return ""
	}
	return "`git apply` changes the files named inside the patch"
}

// xargsValueShort and xargsValueLong are xargs's options that take the next
// word as their value.
const xargsValueShort = "aEILnPsd"

var xargsValueLong = []string{"arg-file", "delimiter", "max-args", "max-lines", "max-procs", "max-chars", "process-slot-var"}

// writerProbes are operands added to a command run by xargs to find out
// whether it writes the files it is given.
var writerProbes = []shellWord{{text: "WRITER-PROBE-A"}, {text: "WRITER-PROBE-B"}}

// xargsWrites reads the command xargs runs. Its operands come from standard
// input, so a command that writes its operands writes files nothing here can
// name. Targets the command's own text does name are still returned.
func xargsWrites(args []shellWord, depth int) argWrites {
	if depth >= maxWriterNesting {
		return argWrites{unjudged: "`xargs` is nested too deeply to read"}
	}
	replacement := ""
	k := 0
	for ; k < len(args); k++ {
		w := args[k].text
		if w == "--" {
			k++
			break
		}
		if !strings.HasPrefix(w, "-") || w == "-" {
			break
		}
		if strings.HasPrefix(w, "--") {
			name, _, hasValue := strings.Cut(w[2:], "=")
			if !hasValue && containsString(xargsValueLong, name) {
				k++
			}
			if name == "replace" {
				replacement = "{}"
				if _, v, ok := strings.Cut(w, "="); ok {
					replacement = v
				}
			}
			continue
		}
		cluster := w[1:]
		for c := 0; c < len(cluster); c++ {
			if cluster[c] == 'i' {
				replacement = "{}"
				if rest := cluster[c+1:]; rest != "" {
					replacement = rest
				}
				break
			}
			if strings.IndexByte(xargsValueShort, cluster[c]) < 0 {
				continue
			}
			value := cluster[c+1:]
			if value == "" {
				k++
				if k < len(args) {
					value = args[k].text
				}
			}
			if cluster[c] == 'I' {
				replacement = value
			}
			break
		}
	}
	if k >= len(args) {
		return argWrites{}
	}
	inner := args[k:]
	at := argumentCommandWord(inner)
	if at < 0 {
		return argWrites{}
	}
	static := commandWrites(inner, at, inner[at+1:], depth+1)
	var w argWrites
	w.opaque, w.unjudged = static.opaque, static.unjudged
	for _, t := range static.targets {
		if replacement != "" && strings.Contains(t, replacement) {
			continue
		}
		w.targets = append(w.targets, t)
		if containsString(static.deletes, t) {
			w.deletes = append(w.deletes, t)
		}
	}
	if w.unjudged != "" {
		return w
	}
	probed := commandWrites(inner, at, append(append([]shellWord(nil), inner[at+1:]...), writerProbes...), depth+1)
	for _, t := range probed.targets {
		if strings.Contains(t, "WRITER-PROBE") {
			w.unjudged = "`xargs " + path.Base(inner[at].text) + "` writes paths only its input supplies"
			break
		}
	}
	if w.unjudged == "" && probed.unjudged != "" {
		w.unjudged = probed.unjudged
	}
	return w
}

// findWrites reads find: -delete removes whatever matches; -exec and its kin
// run a command on each match, which writes files no one can name when the
// command carries the {} placeholder; -fprint and -fls write the file they
// name.
func findWrites(args []shellWord, depth int) argWrites {
	if depth >= maxWriterNesting {
		return argWrites{unjudged: "`find` is nested too deeply to read"}
	}
	var w argWrites
	for k := 0; k < len(args); k++ {
		switch args[k].text {
		case "-delete":
			w.unjudged = "`find -delete` removes whatever matches"
		case "-fprint", "-fprint0", "-fls", "-fprintf":
			if k+1 < len(args) {
				k++
				w.merge(literalWrites([]string{args[k].text}))
			}
		case "-exec", "-execdir", "-ok", "-okdir":
			start := k + 1
			end := start
			for end < len(args) && args[end].text != ";" &&
				!(args[end].text == "+" && end > start && args[end-1].text == "{}") {
				end++
			}
			inner := args[start:end]
			k = end
			at := argumentCommandWord(inner)
			if at < 0 {
				continue
			}
			ran := commandWrites(inner, at, inner[at+1:], depth+1)
			w.opaque = w.opaque || ran.opaque
			if w.unjudged == "" {
				w.unjudged = ran.unjudged
			}
			for _, t := range ran.targets {
				if strings.Contains(t, "{}") {
					if w.unjudged == "" {
						w.unjudged = "`find -exec " + path.Base(inner[at].text) + "` writes every match"
					}
					continue
				}
				w.targets = append(w.targets, t)
				if containsString(ran.deletes, t) {
					w.deletes = append(w.deletes, t)
				}
			}
		}
	}
	return w
}
