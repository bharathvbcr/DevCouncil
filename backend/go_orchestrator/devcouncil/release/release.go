// Package release is the release identity gate. Every producer of a vX.Y.Z
// tag reads a different file: the Git tag, package.json, package-lock.json,
// the rust workspace version, every Cargo.lock under rust/, and the Go
// `Version` stamp. The version must also continue the release line in
// docs/releases. A tag that names 0.2.0 while those files still say something
// else ships the wrong product under the right name. This check fails before
// any build minutes are spent.
//
// It also owns the GitHub-release invariants that CI cannot see until a tag
// is pushed: notes come from docs/releases/vX.Y.Z.md (never through a 48 KB
// environment variable), npm-publish.yml must not create a GitHub Release
// (cargo-dist's release.yml is the one owner), and cargo-dist must not
// package test-helper binaries.
//
// The package imports nothing outside the standard library on purpose. The
// release workflows run this before anything is built, on runners that have
// Go but not the Gusset checkout the host binary links; `cmd/dcrelease` is
// the thin entry point they call.
package release

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"
)

// DistPackage is the one cargo-dist package that ships.
const DistPackage = "devmap-cli"

// RequiredArchives is every archive a release must carry.
var RequiredArchives = []string{
	"devmap-cli-x86_64-unknown-linux-gnu.tar.xz",
	"devmap-cli-aarch64-apple-darwin.tar.xz",
	"devmap-cli-x86_64-apple-darwin.tar.xz",
	"devmap-cli-x86_64-pc-windows-msvc.zip",
}

// WithdrawnMarker is what a notes file carries when its version is not in
// the release line.
const WithdrawnMarker = "<!-- release: withdrawn -->"

// notesLimit keeps notes far under GitHub's 1 MB release-body limit, so they
// never have to travel through GITHUB_ENV (48 KB cap).
const notesLimit = 100_000

var (
	semverRe        = regexp.MustCompile(`^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$`)
	sectionHeaderRe = regexp.MustCompile(`^\[[^\]]+\]$`)
	versionLineRe   = regexp.MustCompile(`^version\s*=\s*"([^"]*)"`)
	commentRe       = regexp.MustCompile(`#.*$`)
	lockPackageRe   = regexp.MustCompile(`(?m)^\[\[package\]\][ \t\r]*$`)
	nextSectionRe   = regexp.MustCompile(`(?m)^\[`)
	nameFieldRe     = regexp.MustCompile(`(?m)^name\s*=\s*"([^"]*)"`)
	crateNameRe     = regexp.MustCompile(`(?m)^name\s*=\s*"([^"]+)"`)
	versionFieldRe  = regexp.MustCompile(`(?m)^version\s*=\s*"([^"]*)"`)
	sourceFieldRe   = regexp.MustCompile(`(?m)^source\s*=`)
	goVersionRe     = regexp.MustCompile(`(?m)^const Version = "([^"]+)"`)
	distTargetsRe   = regexp.MustCompile(`targets\s*=\s*\[((?s:.*?))\]`)
	quotedRe        = regexp.MustCompile(`"([^"]+)"`)
	distFlagRe      = regexp.MustCompile(`^dist\s*=\s*(true|false)\b`)
	workspaceHdrRe  = regexp.MustCompile(`(?m)^\[workspace\]`)
	membersRe       = regexp.MustCompile(`members\s*=\s*\[((?s:.*?))\]`)
	inheritsRe      = regexp.MustCompile(`(?m)^[ \t]*version\.workspace[ \t]*=[ \t]*true[ \t\r]*$`)
	notesNameRe     = regexp.MustCompile(`^v(.+)\.md$`)
	ghCreateRe      = regexp.MustCompile(`(?m)^[ \t]*gh release create\b`)
	announcementRe  = regexp.MustCompile(`(?m)^[ \t]*ANNOUNCEMENT_BODY:`)
	elseRe          = regexp.MustCompile(`\n\s*else\b`)
	editNotesRe     = regexp.MustCompile(`gh release edit\b[^\n]*--notes-file`)
	uploadClobberRe = regexp.MustCompile(`gh release upload\b[^\n]*--clobber`)
)

// quote renders a string the way the gate's messages always have: as a JSON
// string literal.
func quote(s string) string {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(s); err != nil {
		return strconv.Quote(s)
	}
	return strings.TrimRight(buf.String(), "\n")
}

func splitLines(source string) []string {
	return strings.Split(strings.ReplaceAll(source, "\r\n", "\n"), "\n")
}

// ParseTag accepts v<major>.<minor>.<patch> with no suffix and returns the
// version, or the reason it is not a release tag.
func ParseTag(tag string) (version string, reason string) {
	if !strings.HasPrefix(tag, "v") {
		return "", fmt.Sprintf(`tag %s must start with "v"`, quote(tag))
	}
	version = tag[1:]
	if !semverRe.MatchString(version) {
		return "", fmt.Sprintf("tag %s must be v<major>.<minor>.<patch> with no suffix", quote(tag))
	}
	return version, ""
}

// TagNamesHead reports whether a release tag names the commit whose notes
// would actually ship, returning "" when it does.
//
// release.yml checks out the *tagged* commit and publishes
// docs/releases/<tag>.md from it. So notes edited after the tag was cut are
// invisible to the release, and **no CI gate can see the discrepancy**: at the
// tagged commit the old notes are internally consistent, mention the version,
// and sit under the size cap, so every check passes while the wrong text
// ships. Measured 2026-09-14 — v0.2.2 named a commit whose notes were 277
// lines against 653 at HEAD.
//
// This is therefore a pre-push check by construction, not an oversight that
// it runs only locally. A tag that does not exist yet (tagSha == "") is fine:
// pushing creates it at HEAD.
func TagNamesHead(tag, tagSha, headSha string) string {
	if tagSha == "" || tagSha == headSha {
		return ""
	}
	short := func(sha string) string {
		if len(sha) > 12 {
			return sha[:12]
		}
		return sha
	}
	return fmt.Sprintf("tag %s names %s but HEAD is %s; the release would publish docs/releases/%s.md as it was at %s. Retag at HEAD, or push a different tag.",
		tag, short(tagSha), short(headSha), tag, short(tagSha))
}

// ParseTomlSectionVersion reads `version = "..."` from one TOML section,
// e.g. "[workspace.package]" or "[package]".
func ParseTomlSectionVersion(source, section string) (string, bool) {
	inSection := false
	for _, raw := range splitLines(source) {
		line := strings.TrimSpace(commentRe.ReplaceAllString(raw, ""))
		if sectionHeaderRe.MatchString(line) {
			inSection = line == section
			continue
		}
		if !inSection {
			continue
		}
		if m := versionLineRe.FindStringSubmatch(line); m != nil {
			return m[1], true
		}
	}
	return "", false
}

// lockBlocks is the body of every `[[package]]` in a Cargo.lock, cut at the
// next line that opens a section.
func lockBlocks(source string) []string {
	cuts := lockPackageRe.FindAllStringIndex(source, -1)
	blocks := make([]string, 0, len(cuts))
	for i, cut := range cuts {
		end := len(source)
		if i+1 < len(cuts) {
			end = cuts[i+1][0]
		}
		block := source[cut[1]:end]
		if next := nextSectionRe.FindStringIndex(block); next != nil {
			block = block[:next[0]]
		}
		blocks = append(blocks, block)
	}
	return blocks
}

// ParseCargoLockVersion is the version a Cargo.lock records for crate.
func ParseCargoLockVersion(source, crate string) (string, bool) {
	for _, block := range lockBlocks(source) {
		name := nameFieldRe.FindStringSubmatch(block)
		if name == nil || name[1] != crate {
			continue
		}
		version := versionFieldRe.FindStringSubmatch(block)
		if version == nil {
			return "", false
		}
		return version[1], true
	}
	return "", false
}

// LockPackage is one `[[package]]` of a Cargo.lock. Local is true for a path
// dependency — a workspace member, or a crate a nested lock reaches through
// `path = "../..."` — which records no `source`.
type LockPackage struct {
	Name       string
	Version    string
	HasVersion bool
	Local      bool
}

// ParseCargoLockPackages lists every package in a Cargo.lock.
func ParseCargoLockPackages(source string) []LockPackage {
	var out []LockPackage
	for _, block := range lockBlocks(source) {
		name := nameFieldRe.FindStringSubmatch(block)
		if name == nil {
			continue
		}
		pkg := LockPackage{Name: name[1], Local: !sourceFieldRe.MatchString(block)}
		if version := versionFieldRe.FindStringSubmatch(block); version != nil {
			pkg.Version, pkg.HasVersion = version[1], true
		}
		out = append(out, pkg)
	}
	return out
}

// ParseGoVersion reads the Go product stamp, and not another Version constant.
func ParseGoVersion(source string) (string, bool) {
	if m := goVersionRe.FindStringSubmatch(source); m != nil {
		return m[1], true
	}
	return "", false
}

// ParseNpmVersion reads package.json's or package-lock.json's version.
func ParseNpmVersion(source string) (string, error) {
	var payload struct {
		Version any `json:"version"`
	}
	if err := json.Unmarshal([]byte(source), &payload); err != nil {
		return "", err
	}
	version, ok := payload.Version.(string)
	if !ok || version == "" {
		return "", nil
	}
	return version, nil
}

// ParseDistWorkspaceTargets lists the targets in dist-workspace.toml.
func ParseDistWorkspaceTargets(source string) []string {
	m := distTargetsRe.FindStringSubmatch(source)
	if m == nil {
		return nil
	}
	return quotedStrings(m[1])
}

func quotedStrings(s string) []string {
	var out []string
	for _, q := range quotedRe.FindAllStringSubmatch(s, -1) {
		out = append(out, q[1])
	}
	return out
}

// PackageIsDistable reports whether cargo-dist would package a binary crate:
// it does unless `[package.metadata.dist] dist = false`.
func PackageIsDistable(source string) bool {
	inDist := false
	for _, raw := range splitLines(source) {
		line := strings.TrimSpace(commentRe.ReplaceAllString(raw, ""))
		if sectionHeaderRe.MatchString(line) {
			inDist = line == "[package.metadata.dist]"
			continue
		}
		if !inDist {
			continue
		}
		if m := distFlagRe.FindStringSubmatch(line); m != nil {
			return m[1] == "true"
		}
	}
	return true
}

// ParseWorkspaceMembers is the `[workspace] members` list, in declaration
// order.
func ParseWorkspaceMembers(source string) []string {
	hdr := workspaceHdrRe.FindStringIndex(source)
	if hdr == nil {
		return nil
	}
	body := source[hdr[1]:]
	if next := nextSectionRe.FindStringIndex(body); next != nil {
		body = body[:next[0]]
	}
	m := membersRe.FindStringSubmatch(body)
	if m == nil {
		return nil
	}
	return quotedStrings(m[1])
}

func readText(path string) (string, bool) {
	b, err := os.ReadFile(path)
	if err != nil {
		return "", false
	}
	return string(b), true
}

func exists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

// MembersNotInheritingVersion lists workspace members that do not take
// `version.workspace = true`.
//
// Every per-binary version check compares a program's answer against its own
// CARGO_PKG_VERSION, so a crate that swapped inheritance for a literal would
// keep passing all of them while shipping a different number than the
// product. Inheritance is what makes `[workspace.package] version` the single
// owner, so it is asserted here rather than assumed: this is the only place
// that sees every crate at once.
//
// A member whose manifest is missing is reported too — an unbuildable
// workspace must not read as "every crate inherits".
func MembersNotInheritingVersion(rustRoot string, members []string) []string {
	var offenders []string
	for _, member := range members {
		source, ok := readText(filepath.Join(rustRoot, member, "Cargo.toml"))
		if !ok {
			offenders = append(offenders, member+" is a workspace member with no Cargo.toml")
			continue
		}
		if own, ok := ParseTomlSectionVersion(source, "[package]"); ok {
			offenders = append(offenders, fmt.Sprintf("%s declares its own version %s instead of version.workspace = true", member, quote(own)))
			continue
		}
		if !inheritsRe.MatchString(source) {
			offenders = append(offenders, member+" sets no version: it must take version.workspace = true")
		}
	}
	return offenders
}

// WorkspaceLockfiles lists the Cargo.lock files that record workspace crates:
// the workspace's own lock and the lock of every standalone crate directly
// under rustRoot that opts out of the workspace (rust/gusset-engine).
// Measured 2026-09-27: the v1.3.5 bump regenerated rust/Cargo.lock but not
// rust/gusset-engine/Cargo.lock, which kept dc-glob at 0.2.3. This gate read
// only the first, so it passed while every `cargo build --locked` of the
// umbrella — CI's setup-gusset, verify.sh, ci-local — failed.
func WorkspaceLockfiles(rustRoot string) []string {
	entries, err := os.ReadDir(rustRoot)
	if err != nil {
		return nil
	}
	var found []string
	if top := filepath.Join(rustRoot, "Cargo.lock"); exists(top) {
		found = append(found, top)
	}
	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		if lock := filepath.Join(rustRoot, entry.Name(), "Cargo.lock"); exists(lock) {
			found = append(found, lock)
		}
	}
	sort.Strings(found)
	return found
}

// StaleWorkspaceLockEntries lists workspace crates a lockfile records at a
// version other than the product's. Every member inherits
// `[workspace.package] version`, so a lock that names one at anything else
// is stale: `cargo build --locked` against it fails.
func StaleWorkspaceLockEntries(rustRoot string, members []string, identity string) []string {
	crateNames := map[string]bool{}
	for _, member := range members {
		source, ok := readText(filepath.Join(rustRoot, member, "Cargo.toml"))
		if !ok {
			continue
		}
		if m := crateNameRe.FindStringSubmatch(source); m != nil {
			crateNames[m[1]] = true
		} else {
			crateNames[member] = true
		}
	}
	var stale []string
	for _, lockPath := range WorkspaceLockfiles(rustRoot) {
		rel, err := filepath.Rel(filepath.Dir(rustRoot), lockPath)
		if err != nil {
			rel = lockPath
		}
		rel = filepath.ToSlash(rel)
		source, _ := readText(lockPath)
		for _, pkg := range ParseCargoLockPackages(source) {
			if !pkg.Local || !crateNames[pkg.Name] || (pkg.HasVersion && pkg.Version == identity) {
				continue
			}
			recorded := "null"
			if pkg.HasVersion {
				recorded = quote(pkg.Version)
			}
			stale = append(stale, fmt.Sprintf("%s records %s %s, expected %s; regenerate it (cargo update -p %s --offline in %s)",
				rel, pkg.Name, recorded, quote(identity), pkg.Name, path.Dir(rel)))
		}
	}
	return stale
}

// NotesAreWithdrawn reports the marker on a line of its own. Notes that only
// *quote* it — v0.2.4.md explains the marker inline — are not withdrawn.
func NotesAreWithdrawn(notes string) bool {
	for _, line := range splitLines(notes) {
		if strings.TrimSpace(line) == WithdrawnMarker {
			return true
		}
	}
	return false
}

type semver [3]int

func semverParts(version string) (semver, bool) {
	m := semverRe.FindStringSubmatch(version)
	if m == nil {
		return semver{}, false
	}
	var out semver
	for i := range out {
		n, err := strconv.Atoi(m[i+1])
		if err != nil {
			return semver{}, false
		}
		out[i] = n
	}
	return out, true
}

func compareParts(a, b semver) int {
	for i := range a {
		if a[i] != b[i] {
			if a[i] < b[i] {
				return -1
			}
			return 1
		}
	}
	return 0
}

// SuccessorVersions is the versions that may follow prior: the next patch,
// minor or major.
func SuccessorVersions(prior string) []string {
	p, ok := semverParts(prior)
	if !ok {
		return nil
	}
	return []string{
		fmt.Sprintf("%d.%d.%d", p[0], p[1], p[2]+1),
		fmt.Sprintf("%d.%d.0", p[0], p[1]+1),
		fmt.Sprintf("%d.0.0", p[0]+1),
	}
}

// ReleaseLineGaps reports whether the product version continues the release
// line in notesDir.
//
// Every other check here compares the product's files with each other, so a
// bump that moves all of them together to a wrong number passes all of them.
// Measured 2026-09-27: 0.2.3 went to 1.3.5 in one commit, and the gate said
// "consistent". The notes directory is the release history this repository
// owns, so the product version must be the newest release in it or a single
// step past it, and no release in it may be newer than the product.
//
// A notes file carrying WithdrawnMarker on its own line is history kept for
// its readers but outside the line (a pre-unification number, a mistaken
// bump).
func ReleaseLineGaps(notesDir, identity string) []string {
	current, ok := semverParts(identity)
	if !ok {
		return []string{fmt.Sprintf("product version %s is not <major>.<minor>.<patch>", quote(identity))}
	}
	entries, err := os.ReadDir(notesDir)
	if err != nil {
		return nil
	}
	type release struct {
		version string
		parts   semver
	}
	var line []release
	var errs []string
	for _, entry := range entries {
		m := notesNameRe.FindStringSubmatch(entry.Name())
		if m == nil {
			continue
		}
		parts, ok := semverParts(m[1])
		if !ok {
			continue
		}
		notes, _ := readText(filepath.Join(notesDir, entry.Name()))
		if NotesAreWithdrawn(notes) {
			if m[1] == identity {
				errs = append(errs, fmt.Sprintf("product version %s is marked withdrawn in docs/releases/%s", identity, entry.Name()))
			}
			continue
		}
		line = append(line, release{version: m[1], parts: parts})
	}
	var newer []string
	var prior *release
	for i := range line {
		r := &line[i]
		switch c := compareParts(r.parts, current); {
		case c > 0:
			newer = append(newer, "v"+r.version)
		case c < 0:
			if prior == nil || compareParts(r.parts, prior.parts) > 0 {
				prior = r
			}
		}
	}
	if len(newer) > 0 {
		errs = append(errs, fmt.Sprintf("docs/releases has notes newer than product version %s: %s. Bump past them, or mark each withdrawn with %s",
			identity, strings.Join(newer, ", "), WithdrawnMarker))
	}
	if prior != nil {
		successors := SuccessorVersions(prior.version)
		follows := false
		for _, s := range successors {
			if s == identity {
				follows = true
			}
		}
		if !follows {
			errs = append(errs, fmt.Sprintf("product version %s does not follow the last release %s; expected one of %s",
				identity, prior.version, strings.Join(successors, ", ")))
		}
	}
	return errs
}

// BinaryPackage is a crate cargo would build a binary for.
type BinaryPackage struct {
	Name     string
	Distable bool
	Dir      string
}

// BinaryPackages lists the packages under rustRoot that cargo would build a
// binary for: they have src/main.rs or at least one src/bin/*.rs. A library
// crate with no binary is not dist-able even when `dist` is left at the
// default.
//
// The candidates are every directory directly under rustRoot *and* every
// `[workspace] members` entry. cargo-dist packages workspace members wherever
// they live, so a binary crate nested deeper (rust/tools/soak) would
// otherwise ship without this gate having seen it.
func BinaryPackages(rustRoot string) []BinaryPackage {
	entries, err := os.ReadDir(rustRoot)
	if err != nil {
		return nil
	}
	seen := map[string]bool{}
	var dirs []string
	add := func(dir string) {
		dir = filepath.Clean(dir)
		if !seen[dir] {
			seen[dir] = true
			dirs = append(dirs, dir)
		}
	}
	for _, entry := range entries {
		if entry.IsDir() {
			add(filepath.Join(rustRoot, entry.Name()))
		}
	}
	if manifest, ok := readText(filepath.Join(rustRoot, "Cargo.toml")); ok {
		for _, member := range ParseWorkspaceMembers(manifest) {
			add(filepath.Join(rustRoot, filepath.FromSlash(member)))
		}
	}
	var found []BinaryPackage
	for _, dir := range dirs {
		source, ok := readText(filepath.Join(dir, "Cargo.toml"))
		if !ok {
			continue
		}
		if !exists(filepath.Join(dir, "src", "main.rs")) && !hasRustFile(filepath.Join(dir, "src", "bin")) {
			continue
		}
		name := filepath.Base(dir)
		if m := crateNameRe.FindStringSubmatch(source); m != nil {
			name = m[1]
		}
		found = append(found, BinaryPackage{Name: name, Distable: PackageIsDistable(source), Dir: dir})
	}
	sort.Slice(found, func(i, j int) bool { return found[i].Name < found[j].Name })
	return found
}

func hasRustFile(dir string) bool {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return false
	}
	for _, entry := range entries {
		if strings.HasSuffix(entry.Name(), ".rs") {
			return true
		}
	}
	return false
}

// NpmPublishCreatesGitHubRelease reports a `gh release create` step.
func NpmPublishCreatesGitHubRelease(yaml string) bool {
	return ghCreateRe.MatchString(yaml)
}

// NpmPublishSkipsExistingVersion reports the registry probe that skips a
// version npm already has.
func NpmPublishSkipsExistingVersion(yaml string) bool {
	return strings.Contains(yaml, "npm ping") &&
		strings.Contains(yaml, "published=false") &&
		strings.Contains(yaml, "npm view")
}

// ReleasePutsAnnouncementBodyInEnv reports an ANNOUNCEMENT_BODY key on any
// line. The script this replaced anchored the pattern to the start of the
// whole file, so the indented `env:` entry it exists to catch never matched.
func ReleasePutsAnnouncementBodyInEnv(yaml string) bool {
	return announcementRe.MatchString(yaml)
}

// ReleaseUsesNotesFile reports that release notes come from docs/releases.
func ReleaseUsesNotesFile(yaml string) bool {
	return strings.Contains(yaml, "docs/releases/") && strings.Contains(yaml, "--notes-file")
}

// ReleaseIsIdempotent reports the view/edit/upload path an existing tag takes.
func ReleaseIsIdempotent(yaml string) bool {
	return strings.Contains(yaml, "gh release view") &&
		strings.Contains(yaml, "gh release edit") &&
		strings.Contains(yaml, "gh release upload")
}

// ReleaseNotesPublishGaps lists gaps in the path that runs when cargo-dist
// has already opened the tag.
//
// `host --steps=release` creates the GitHub Release before this step, with
// cargo-dist's own changelog. The shell then uses `set -euo pipefail`. If
// `gh release upload` runs first and fails — the usual retry error is an
// asset that already exists — the notes edit never runs, and the release
// that users see is the changelog, not docs/releases/<tag>.md.
//
// The existing-release branch must edit the notes first, then re-upload with
// --clobber.
func ReleaseNotesPublishGaps(yaml string) []string {
	if !strings.Contains(yaml, "gh release view") {
		return []string{"existing-release path must `gh release view` before editing notes; without it a second publish runs `gh release create` and fails after cargo-dist has already opened the tag"}
	}
	marker := "gh release view"
	if strings.Contains(yaml, "if gh release view") {
		marker = "if gh release view"
	}
	_, afterView, _ := strings.Cut(yaml, marker)
	afterView, _, _ = strings.Cut(afterView, marker)
	branch := afterView
	if loc := elseRe.FindStringIndex(afterView); loc != nil {
		branch = afterView[:loc[0]]
	}
	editAt, uploadAt := -1, -1
	if loc := editNotesRe.FindStringIndex(branch); loc != nil {
		editAt = loc[0]
	}
	if loc := uploadClobberRe.FindStringIndex(branch); loc != nil {
		uploadAt = loc[0]
	}
	var errs []string
	if editAt < 0 {
		errs = append(errs, "existing-release path must `gh release edit` with --notes-file; cargo-dist opens the release first, so this is the path that publishes docs/releases")
	}
	if uploadAt < 0 {
		errs = append(errs, "existing-release path must `gh release upload --clobber`; without --clobber a retry fails after cargo-dist has already attached the archives")
	}
	if editAt >= 0 && uploadAt >= 0 && editAt > uploadAt {
		errs = append(errs, "existing-release path uploads before editing notes; a failed upload skips the notes edit and the release keeps cargo-dist's changelog")
	}
	return errs
}

// MissingArchives lists the required archives absent anywhere under dir.
func MissingArchives(dir string, required []string) []string {
	have := map[string]bool{}
	if info, err := os.Stat(dir); err == nil && info.IsDir() {
		_ = filepath.WalkDir(dir, func(_ string, d os.DirEntry, err error) error {
			if err == nil && !d.IsDir() {
				have[d.Name()] = true
			}
			return nil
		})
	}
	var missing []string
	for _, name := range required {
		if !have[name] {
			missing = append(missing, name)
		}
	}
	return missing
}

// Sources is every file the gate reads.
type Sources struct {
	PackagePath         string
	PackageLockPath     string
	CargoTomlPath       string
	CargoLockPath       string
	UmbrellaLockPath    string
	GoVersionPath       string
	RustRoot            string
	DistWorkspacePath   string
	NpmPublishPath      string
	ReleaseWorkflowPath string
	NotesDir            string
}

// DefaultSources is where the gate reads from in a checkout rooted at root.
func DefaultSources(root string) Sources {
	return Sources{
		PackagePath:     filepath.Join(root, "package.json"),
		PackageLockPath: filepath.Join(root, "package-lock.json"),
		CargoTomlPath:   filepath.Join(root, "rust", "Cargo.toml"),
		CargoLockPath:   filepath.Join(root, "rust", "Cargo.lock"),
		// The Gusset umbrella is its own workspace with its own lock, and
		// dc-glob inherits the product version, so a version bump that skips
		// this lock breaks every `cargo build --locked` of the umbrella
		// (setup-gusset, verify.sh, cgo-env.sh) — which is how 0.2.3 sat in it
		// at 1.3.5.
		UmbrellaLockPath: filepath.Join(root, "rust", "gusset-engine", "Cargo.lock"),
		// devcouncil/version, not cmd/devcouncil: the constant moved into a
		// library so devcouncil/mcp could import it, and this gate must read
		// the owner rather than the alias `package main` keeps for printing.
		GoVersionPath:       filepath.Join(root, "backend", "go_orchestrator", "devcouncil", "version", "version.go"),
		RustRoot:            filepath.Join(root, "rust"),
		DistWorkspacePath:   filepath.Join(root, "dist-workspace.toml"),
		NpmPublishPath:      filepath.Join(root, ".github", "workflows", "npm-publish.yml"),
		ReleaseWorkflowPath: filepath.Join(root, ".github", "workflows", "release.yml"),
		NotesDir:            filepath.Join(root, "docs", "releases"),
	}
}

// Options narrows or widens what Inspect checks.
type Options struct {
	Tag           string
	ArtifactsDir  string
	SkipWorkflows bool
}

// Result is the product version the files agree on (empty when none could
// be read) and every way they do not.
type Result struct {
	Version string
	Errors  []string
}

type stamp struct {
	label   string
	version string
	ok      bool
}

func archiveFor(target string) string {
	if strings.Contains(target, "windows") {
		return DistPackage + "-" + target + ".zip"
	}
	return DistPackage + "-" + target + ".tar.xz"
}

func contains(list []string, s string) bool {
	for _, v := range list {
		if v == s {
			return true
		}
	}
	return false
}

// Inspect runs the gate.
func Inspect(src Sources, opts Options) Result {
	var errs []string
	read := func(file, label string) (string, bool) {
		text, ok := readText(file)
		if !ok {
			errs = append(errs, fmt.Sprintf("missing %s: %s", label, file))
		}
		return text, ok
	}

	pkg, havePkg := read(src.PackagePath, "package.json")
	lock, haveLock := read(src.PackageLockPath, "package-lock.json")
	cargoToml, haveCargoToml := read(src.CargoTomlPath, "rust/Cargo.toml")
	cargoLock, haveCargoLock := read(src.CargoLockPath, "rust/Cargo.lock")
	goSrc, haveGo := read(src.GoVersionPath, "version.go")

	npmStamp := func(label, text string, have bool) stamp {
		if !have {
			return stamp{label: label}
		}
		v, err := ParseNpmVersion(text)
		if err != nil {
			errs = append(errs, fmt.Sprintf("%s is not valid JSON: %v", label, err))
			return stamp{label: label}
		}
		return stamp{label: label, version: v, ok: v != ""}
	}
	stamps := []stamp{
		npmStamp("package.json", pkg, havePkg),
		npmStamp("package-lock.json", lock, haveLock),
		{label: "rust/Cargo.toml [workspace.package]"},
		{label: "rust/Cargo.lock " + DistPackage},
		{label: "version.go"},
	}
	if haveCargoToml {
		stamps[2].version, stamps[2].ok = ParseTomlSectionVersion(cargoToml, "[workspace.package]")
	}
	if haveCargoLock {
		stamps[3].version, stamps[3].ok = ParseCargoLockVersion(cargoLock, DistPackage)
	}
	if haveGo {
		stamps[4].version, stamps[4].ok = ParseGoVersion(goSrc)
	}
	// Optional so fixtures without an umbrella still inspect; the umbrella's
	// own --locked build refuses a missing lock.
	if umbrella, ok := readText(src.UmbrellaLockPath); ok && src.UmbrellaLockPath != "" {
		v, found := ParseCargoLockVersion(umbrella, "dc-glob")
		stamps = append(stamps, stamp{label: "rust/gusset-engine/Cargo.lock dc-glob", version: v, ok: found})
	}

	identity := ""
	for _, s := range stamps {
		if s.ok && s.version != "" {
			identity = s.version
			break
		}
	}
	for _, s := range stamps {
		switch {
		case !s.ok:
			errs = append(errs, s.label+" has no version")
		case identity != "" && s.version != identity:
			errs = append(errs, fmt.Sprintf("%s is %s, expected %s", s.label, quote(s.version), quote(identity)))
		}
	}

	if opts.Tag != "" {
		version, reason := ParseTag(opts.Tag)
		switch {
		case reason != "":
			errs = append(errs, reason)
		case identity != "" && version != identity:
			errs = append(errs, fmt.Sprintf("tag %s names %s, product version is %s", quote(opts.Tag), quote(version), quote(identity)))
		}
	}

	if identity != "" {
		notesPath := filepath.Join(src.NotesDir, "v"+identity+".md")
		notes, ok := readText(notesPath)
		switch {
		case !ok:
			errs = append(errs, "missing release notes: "+notesPath)
		case strings.TrimSpace(notes) == "":
			errs = append(errs, "release notes are empty: "+notesPath)
		case !strings.Contains(notes, identity):
			errs = append(errs, fmt.Sprintf("release notes do not mention %s: %s", identity, notesPath))
		case utf8.RuneCountInString(notes) > notesLimit:
			errs = append(errs, fmt.Sprintf("release notes exceed %d characters: %s", notesLimit, notesPath))
		}
	}

	if haveCargoToml {
		members := ParseWorkspaceMembers(cargoToml)
		if len(members) == 0 {
			errs = append(errs, "rust/Cargo.toml declares no workspace members")
		} else {
			errs = append(errs, MembersNotInheritingVersion(src.RustRoot, members)...)
			if identity != "" {
				errs = append(errs, StaleWorkspaceLockEntries(src.RustRoot, members, identity)...)
			}
		}
	}

	if identity != "" {
		errs = append(errs, ReleaseLineGaps(src.NotesDir, identity)...)
	}

	if exists(src.RustRoot) {
		packages := BinaryPackages(src.RustRoot)
		var extras []string
		var dist *BinaryPackage
		for i, p := range packages {
			if p.Name == DistPackage {
				dist = &packages[i]
			} else if p.Distable {
				extras = append(extras, p.Name)
			}
		}
		if len(extras) > 0 {
			errs = append(errs, fmt.Sprintf("cargo-dist would ship helper/analysis binaries: %s. Set [package.metadata.dist] dist = false on each.", strings.Join(extras, ", ")))
		}
		switch {
		case dist == nil:
			errs = append(errs, fmt.Sprintf("binary package %s was not found under rust/", DistPackage))
		case !dist.Distable:
			errs = append(errs, DistPackage+" has dist = false; GitHub Releases would have no archives")
		}
	}

	if distToml, ok := readText(src.DistWorkspacePath); ok && src.DistWorkspacePath != "" {
		targets := ParseDistWorkspaceTargets(distToml)
		if len(targets) == 0 {
			errs = append(errs, "dist-workspace.toml declares no targets")
		} else {
			declared := make([]string, 0, len(targets))
			for _, target := range targets {
				archive := archiveFor(target)
				declared = append(declared, archive)
				if !contains(RequiredArchives, archive) {
					errs = append(errs, fmt.Sprintf("dist-workspace.toml target %s (%s) is missing from REQUIRED_ARCHIVES", target, archive))
				}
			}
			for _, req := range RequiredArchives {
				if !contains(declared, req) {
					errs = append(errs, fmt.Sprintf("REQUIRED_ARCHIVES entry %s is not declared in dist-workspace.toml", req))
				}
			}
		}
	}

	if !opts.SkipWorkflows {
		if npmYaml, ok := read(src.NpmPublishPath, "npm-publish.yml"); ok {
			if NpmPublishCreatesGitHubRelease(npmYaml) {
				errs = append(errs, "npm-publish.yml runs `gh release create`; a tag would race cargo-dist's release.yml and one of them would fail")
			}
			if !NpmPublishSkipsExistingVersion(npmYaml) {
				errs = append(errs, "npm-publish.yml does not skip versions already on the registry; republishing 0.2.0 would fail the whole tag")
			}
		}
		if relYaml, ok := read(src.ReleaseWorkflowPath, "release.yml"); ok {
			if ReleasePutsAnnouncementBodyInEnv(relYaml) {
				errs = append(errs, "release.yml puts ANNOUNCEMENT_BODY in the environment; GitHub caps env values at 48 KB and the notes would fail after the builds")
			}
			if !ReleaseUsesNotesFile(relYaml) {
				errs = append(errs, "release.yml does not publish docs/releases/ via --notes-file")
			}
			if !ReleaseIsIdempotent(relYaml) {
				errs = append(errs, "release.yml is not idempotent (needs gh release view/edit/upload); a second create on an existing tag fails after the artifacts are built")
			}
			errs = append(errs, ReleaseNotesPublishGaps(relYaml)...)
		}
	}

	if opts.ArtifactsDir != "" {
		if missing := MissingArchives(opts.ArtifactsDir, RequiredArchives); len(missing) > 0 {
			errs = append(errs, "required release archives missing: "+strings.Join(missing, ", "))
		}
	}

	return Result{Version: identity, Errors: errs}
}

// FindRoot walks up from dir to the checkout this gate inspects: the nearest
// directory holding both package.json and rust/Cargo.toml.
func FindRoot(dir string) (string, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return "", err
	}
	for d := abs; ; {
		if exists(filepath.Join(d, "package.json")) && exists(filepath.Join(d, "rust", "Cargo.toml")) {
			return d, nil
		}
		parent := filepath.Dir(d)
		if parent == d {
			return "", fmt.Errorf("no directory above %s holds package.json and rust/Cargo.toml; pass --root", abs)
		}
		d = parent
	}
}
