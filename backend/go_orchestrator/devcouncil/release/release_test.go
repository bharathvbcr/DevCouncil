package release

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
)

type tree struct {
	pkg, rust, goVer string
	notes, distStore bool
}

func defaultTree() tree {
	return tree{pkg: "0.2.0", rust: "0.2.0", goVer: "0.2.0", notes: true}
}

func write(t *testing.T, path, body string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func mustJSON(t *testing.T, v any) string {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func scratchTree(t *testing.T, v tree) string {
	t.Helper()
	dir := t.TempDir()
	write(t, filepath.Join(dir, "package.json"), mustJSON(t, map[string]any{"name": "devcouncil", "version": v.pkg}))
	write(t, filepath.Join(dir, "package-lock.json"), mustJSON(t, map[string]any{
		"name": "devcouncil", "version": v.pkg, "lockfileVersion": 3,
		"packages": map[string]any{"": map[string]any{"version": v.pkg}},
	}))
	write(t, filepath.Join(dir, "rust", "Cargo.toml"),
		"[workspace]\nmembers = [\"devmap-cli\", \"devmap-store\"]\n\n[workspace.package]\nversion = \""+v.rust+"\"\n")
	write(t, filepath.Join(dir, "rust", "Cargo.lock"), "[[package]]\nname = \""+DistPackage+"\"\nversion = \""+v.rust+"\"\n")
	write(t, filepath.Join(dir, "backend", "go_orchestrator", "devcouncil", "version", "version.go"),
		"package version\n\nconst Version = \""+v.goVer+"\"\n")
	write(t, filepath.Join(dir, "rust", "devmap-cli", "Cargo.toml"), "[package]\nname = \""+DistPackage+"\"\nversion.workspace = true\n")
	write(t, filepath.Join(dir, "rust", "devmap-cli", "src", "main.rs"), "fn main() {}\n")
	store := "[package]\nname = \"devmap-store\"\nversion.workspace = true\n"
	if !v.distStore {
		store += "[package.metadata.dist]\ndist = false\n"
	}
	write(t, filepath.Join(dir, "rust", "devmap-store", "Cargo.toml"), store)
	write(t, filepath.Join(dir, "rust", "devmap-store", "src", "bin", "hold_uncommitted_child.rs"), "fn main() {}\n")
	if v.notes {
		write(t, filepath.Join(dir, "docs", "releases", "v"+v.pkg+".md"), "# DevCouncil v"+v.pkg+"\n\nNative host.\n")
	} else if err := os.MkdirAll(filepath.Join(dir, "docs", "releases"), 0o755); err != nil {
		t.Fatal(err)
	}
	write(t, filepath.Join(dir, "dist-workspace.toml"), strings.Join([]string{
		"[workspace]",
		`members = ["cargo:./rust"]`,
		"",
		"[dist]",
		"targets = [",
		`    "x86_64-unknown-linux-gnu",`,
		`    "aarch64-apple-darwin",`,
		`    "x86_64-apple-darwin",`,
		`    "x86_64-pc-windows-msvc",`,
		"]",
		"",
	}, "\n"))
	write(t, filepath.Join(dir, ".github", "workflows", "npm-publish.yml"), strings.Join([]string{
		"name: npm",
		"jobs:",
		"  publish:",
		"    steps:",
		"      - run: npm ping",
		"      - run: npm view devcouncil@$VERSION version",
		"      - run: echo published=false >> $GITHUB_OUTPUT",
	}, "\n"))
	write(t, filepath.Join(dir, ".github", "workflows", "release.yml"), strings.Join([]string{
		"name: Release",
		"jobs:",
		"  host:",
		"    steps:",
		`      - run: NOTES_FILE="docs/releases/${RELEASE_TAG}.md"`,
		`      - run: gh release view "$RELEASE_TAG"`,
		`      - run: gh release edit "$RELEASE_TAG" --notes-file "$NOTES_FILE"`,
		`      - run: gh release upload "$RELEASE_TAG" artifacts/* --clobber`,
	}, "\n"))
	return dir
}

func repoRoot(t *testing.T) string {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	root, err := FindRoot(wd)
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func readFile(t *testing.T, path string) string {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func readWorkflow(t *testing.T, name string) string {
	t.Helper()
	return readFile(t, filepath.Join(repoRoot(t), ".github", "workflows", name))
}

func anyContains(list []string, parts ...string) bool {
	for _, s := range list {
		all := true
		for _, p := range parts {
			if !strings.Contains(s, p) {
				all = false
				break
			}
		}
		if all {
			return true
		}
	}
	return false
}

func mustEqual(t *testing.T, got, want any) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %#v, want %#v", got, want)
	}
}

func TestParseTagAcceptsAThreePartTagAndRejectsSuffixes(t *testing.T) {
	v, reason := ParseTag("v0.2.0")
	mustEqual(t, v, "0.2.0")
	mustEqual(t, reason, "")
	for _, bad := range []string{"0.2.0", "v0.2.0-rc.1"} {
		if _, reason := ParseTag(bad); reason == "" {
			t.Fatalf("%s accepted", bad)
		}
	}
}

const (
	headSha  = "bc26de4a1b2c3d4e5f60718293a4b5c6d7e8f901"
	olderSha = "1d680487a6b5c4d3e2f10918273645a5b4c3d2e1"
)

func TestTagNamesHeadPassesWhenTheTagNamesHead(t *testing.T) {
	mustEqual(t, TagNamesHead("v0.2.2", headSha, headSha), "")
}

func TestTagNamesHeadPassesWhenTheTagDoesNotExistYet(t *testing.T) {
	// The push creates it at HEAD.
	mustEqual(t, TagNamesHead("v0.2.2", "", headSha), "")
}

func TestTagNamesHeadFailsWhenTheTagIsBehindHeadNamingBothCommitsAndTheNotesFile(t *testing.T) {
	reason := TagNamesHead("v0.2.2", olderSha, headSha)
	// The operator has to be told which notes would actually ship, because
	// every other gate passes on the stale ones.
	for _, want := range []string{"1d680487a6b5", "bc26de4a1b2c", "docs/releases/v0.2.2.md"} {
		if !strings.Contains(reason, want) {
			t.Fatalf("reason %q lacks %q", reason, want)
		}
	}
}

func TestParsersReadWorkspacePackageRatherThanADependencyVersion(t *testing.T) {
	source := "[workspace.package]\nversion = \"0.2.0\"\n\n[workspace.dependencies]\nrusqlite = { version = \"0.40.2\" }\n"
	v, ok := ParseTomlSectionVersion(source, "[workspace.package]")
	mustEqual(t, v, "0.2.0")
	mustEqual(t, ok, true)
	if _, ok := ParseTomlSectionVersion(source, "[package]"); ok {
		t.Fatal("[package] has no version")
	}
}

func TestParsersReadTheNamedCrateFromCargoLockNotTheNextPackage(t *testing.T) {
	source := "[[package]]\nname = \"clap\"\nversion = \"4.5.0\"\n\n[[package]]\nname = \"devmap-cli\"\nversion = \"0.2.0\"\n"
	v, ok := ParseCargoLockVersion(source, "devmap-cli")
	mustEqual(t, v, "0.2.0")
	mustEqual(t, ok, true)
	if _, ok := ParseCargoLockVersion(source, "missing"); ok {
		t.Fatal("missing crate found")
	}
}

func TestParsersReadTheGoProductStampAndNotAnotherVersionConstant(t *testing.T) {
	v, ok := ParseGoVersion("package main\n\nconst Version = \"0.2.0\"\n")
	mustEqual(t, v, "0.2.0")
	mustEqual(t, ok, true)
	if _, ok := ParseGoVersion("const SchemaVersion = 9\n"); ok {
		t.Fatal("SchemaVersion read as the product stamp")
	}
}

func TestParsersTreatMissingDistMetadataAsDistableAndDistFalseAsNot(t *testing.T) {
	mustEqual(t, PackageIsDistable("[package]\nname = \"x\"\n"), true)
	mustEqual(t, PackageIsDistable("[package.metadata.dist]\ndist = false\n"), false)
	mustEqual(t, PackageIsDistable("[package.metadata.dist]\ndist = true\n"), true)
}

func TestParsersParseTargetsFromDistWorkspaceToml(t *testing.T) {
	source := "[workspace]\nmembers = [\"cargo:./rust\"]\n\n[dist]\ntargets = [\n    \"x86_64-unknown-linux-gnu\",\n    \"aarch64-apple-darwin\",\n]\n"
	mustEqual(t, ParseDistWorkspaceTargets(source), []string{"x86_64-unknown-linux-gnu", "aarch64-apple-darwin"})
}

func TestMissingArchivesNamesEveryArchiveWhenEmptyAndNothingWhenAllArePresent(t *testing.T) {
	empty := t.TempDir()
	mustEqual(t, MissingArchives(empty, RequiredArchives), RequiredArchives)

	full := t.TempDir()
	for _, name := range RequiredArchives {
		write(t, filepath.Join(full, name), "x")
	}
	mustEqual(t, len(MissingArchives(full, RequiredArchives)), 0)
}

func TestMissingArchivesDoesNotTreatACappedSampleAsComplete(t *testing.T) {
	// A linux archive is not the set.
	dir := t.TempDir()
	write(t, filepath.Join(dir, RequiredArchives[0]), "x")
	mustEqual(t, MissingArchives(dir, RequiredArchives), RequiredArchives[1:])
}

func TestMissingArchivesRequiresTheWindowsMsvcZipByName(t *testing.T) {
	// So a missing grammar build cannot ship.
	if !slices.Contains(RequiredArchives, "devmap-cli-x86_64-pc-windows-msvc.zip") {
		t.Fatal("Windows zip is not required")
	}
}

func TestInspectAcceptsAConsistentTree(t *testing.T) {
	result := Inspect(DefaultSources(scratchTree(t, defaultTree())), Options{})
	mustEqual(t, len(result.Errors), 0)
	mustEqual(t, result.Version, "0.2.0")
}

func TestInspectFailsWhenPackageJSONDriftsFromTheGoStamp(t *testing.T) {
	v := defaultTree()
	v.pkg = "0.4.2"
	result := Inspect(DefaultSources(scratchTree(t, v)), Options{})
	if !anyContains(result.Errors, "0.4.2", "0.2.0") {
		t.Fatalf("errors: %v", result.Errors)
	}
}

func TestInspectFailsWhenTheGussetUmbrellasLockNamesAnotherDcGlobVersion(t *testing.T) {
	root := scratchTree(t, defaultTree())
	lock := func(v string) string {
		return "version = 4\n\n[[package]]\nname = \"dc-glob\"\nversion = \"" + v + "\"\n"
	}
	path := filepath.Join(root, "rust", "gusset-engine", "Cargo.lock")
	write(t, path, lock("0.2.0"))
	mustEqual(t, len(Inspect(DefaultSources(root), Options{}).Errors), 0)
	write(t, path, lock("0.1.9"))
	if errs := Inspect(DefaultSources(root), Options{}).Errors; !anyContains(errs, "gusset-engine/Cargo.lock", "0.1.9") {
		t.Fatalf("errors: %v", errs)
	}
}

func TestInspectFailsWhenTheNotesFileForThisVersionIsMissing(t *testing.T) {
	v := defaultTree()
	v.notes = false
	if errs := Inspect(DefaultSources(scratchTree(t, v)), Options{}).Errors; !anyContains(errs, "missing release notes") {
		t.Fatalf("errors: %v", errs)
	}
}

func TestInspectFailsWhenTheTagNamesADifferentVersion(t *testing.T) {
	errs := Inspect(DefaultSources(scratchTree(t, defaultTree())), Options{Tag: "v0.4.2"}).Errors
	if !anyContains(errs, "v0.4.2") {
		t.Fatalf("errors: %v", errs)
	}
}

func TestInspectFailsWhenAHelperBinaryPackageIsStillDistable(t *testing.T) {
	v := defaultTree()
	v.distStore = true
	root := scratchTree(t, v)
	found := false
	for _, p := range BinaryPackages(filepath.Join(root, "rust")) {
		if p.Distable && p.Name == "devmap-store" {
			found = true
		}
	}
	if !found {
		t.Fatal("devmap-store not reported dist-able")
	}
	if errs := Inspect(DefaultSources(root), Options{}).Errors; !anyContains(errs, "devmap-store") {
		t.Fatalf("errors: %v", errs)
	}
}

func TestInspectFailsWhenANestedWorkspaceMemberBinaryIsDistable(t *testing.T) {
	// cargo-dist packages workspace members wherever they live; a binary crate
	// below rust/<dir>/ is not a direct child, and the script this replaced
	// scanned only direct children.
	root := scratchTree(t, defaultTree())
	write(t, filepath.Join(root, "rust", "Cargo.toml"),
		"[workspace]\nmembers = [\"devmap-cli\", \"devmap-store\", \"tools/soak\"]\n\n[workspace.package]\nversion = \"0.2.0\"\n")
	write(t, filepath.Join(root, "rust", "tools", "soak", "Cargo.toml"), "[package]\nname = \"dc-soak\"\nversion.workspace = true\n")
	write(t, filepath.Join(root, "rust", "tools", "soak", "src", "main.rs"), "fn main() {}\n")
	if errs := Inspect(DefaultSources(root), Options{}).Errors; !anyContains(errs, "helper/analysis binaries", "dc-soak") {
		t.Fatalf("errors: %v", errs)
	}
	write(t, filepath.Join(root, "rust", "tools", "soak", "Cargo.toml"),
		"[package]\nname = \"dc-soak\"\nversion.workspace = true\n[package.metadata.dist]\ndist = false\n")
	mustEqual(t, Inspect(DefaultSources(root), Options{}).Errors, []string(nil))
}

func TestInspectFailsWhenDistWorkspaceTomlMissesARequiredTarget(t *testing.T) {
	root := scratchTree(t, defaultTree())
	write(t, filepath.Join(root, "dist-workspace.toml"),
		"[workspace]\nmembers = [\"cargo:./rust\"]\n\n[dist]\ntargets = [\"x86_64-unknown-linux-gnu\"]\n")
	if errs := Inspect(DefaultSources(root), Options{}).Errors; !anyContains(errs, "REQUIRED_ARCHIVES entry", "not declared") {
		t.Fatalf("errors: %v", errs)
	}
}

func TestInspectReportsUnparseablePackageJSONInsteadOfCrashing(t *testing.T) {
	root := scratchTree(t, defaultTree())
	write(t, filepath.Join(root, "package.json"), "{not json")
	errs := Inspect(DefaultSources(root), Options{}).Errors
	if !anyContains(errs, "package.json is not valid JSON") || !anyContains(errs, "package.json has no version") {
		t.Fatalf("errors: %v", errs)
	}
}

func TestThisRepositoryDoesNotLetNpmPublishCreateAGitHubRelease(t *testing.T) {
	// That races cargo-dist.
	mustEqual(t, NpmPublishCreatesGitHubRelease(readWorkflow(t, "npm-publish.yml")), false)
}

func TestThisRepositorySkipsNpmPublishWhenTheVersionIsAlreadyOnTheRegistry(t *testing.T) {
	mustEqual(t, NpmPublishSkipsExistingVersion(readWorkflow(t, "npm-publish.yml")), true)
}

func TestThisRepositoryDoesNotPutTheAnnouncementBodyInTheEnvironment(t *testing.T) {
	mustEqual(t, ReleasePutsAnnouncementBodyInEnv(readWorkflow(t, "release.yml")), false)
}

func TestAnnouncementBodyIsCaughtOnAnIndentedEnvLine(t *testing.T) {
	// Where an env entry actually sits. The script this replaced anchored the
	// pattern to the start of the file and never matched this.
	yaml := "jobs:\n  host:\n    env:\n      ANNOUNCEMENT_BODY: ${{ fromJson(x).body }}\n"
	mustEqual(t, ReleasePutsAnnouncementBodyInEnv(yaml), true)
	mustEqual(t, ReleasePutsAnnouncementBodyInEnv("# ANNOUNCEMENT_BODY: is not set here\n"), false)
}

func TestThisRepositoryPublishesNotesViaNotesFileAndIsIdempotentOnAnExistingTag(t *testing.T) {
	rel := readWorkflow(t, "release.yml")
	mustEqual(t, ReleaseUsesNotesFile(rel), true)
	mustEqual(t, ReleaseIsIdempotent(rel), true)
	mustEqual(t, ReleaseNotesPublishGaps(rel), []string(nil))
}

func TestRejectsAnExistingReleasePathThatUploadsBeforeTheNotesEdit(t *testing.T) {
	yaml := `
          if gh release view "$RELEASE_TAG" >/dev/null 2>&1; then
            gh release upload "$RELEASE_TAG" "${files[@]}" --clobber
            gh release edit "$RELEASE_TAG" --notes-file "$NOTES_FILE"
          else
            gh release create "$RELEASE_TAG" --notes-file "$NOTES_FILE" "${files[@]}"
          fi
`
	if gaps := ReleaseNotesPublishGaps(yaml); !anyContains(gaps, "uploads before editing notes") {
		t.Fatalf("gaps: %v", gaps)
	}
}

func TestThisRepositoryDoesNotKeepOrAttachJudgeReleaseNotes(t *testing.T) {
	if _, err := os.Stat(filepath.Join(repoRoot(t), "docs", "judge-release-notes.md")); err == nil {
		t.Fatal("docs/judge-release-notes.md exists")
	}
	for _, name := range []string{"npm-publish.yml", "release.yml"} {
		if strings.Contains(readWorkflow(t, name), "judge-release-notes") {
			t.Fatalf("%s attaches judge-release-notes", name)
		}
	}
}

func TestWorkspaceMembersAreReadFromTheMembersListNotADependencyArray(t *testing.T) {
	source := "[workspace]\nresolver = \"3\"\nmembers = [\"dc-store\", \"dc-verify\"]\n\n" +
		"[workspace.dependencies]\nserde = { version = \"1.0\", features = [\"derive\"] }\n"
	mustEqual(t, ParseWorkspaceMembers(source), []string{"dc-store", "dc-verify"})
}

func TestInheritancePassesWhenEveryMemberInherits(t *testing.T) {
	root := scratchTree(t, defaultTree())
	mustEqual(t, MembersNotInheritingVersion(filepath.Join(root, "rust"), []string{"devmap-cli", "devmap-store"}), []string(nil))
}

func TestInheritanceFailsAMemberThatDeclaresItsOwnVersionNamingTheNumber(t *testing.T) {
	root := scratchTree(t, defaultTree())
	write(t, filepath.Join(root, "rust", "devmap-store", "Cargo.toml"),
		"[package]\nname = \"devmap-store\"\nversion = \"0.1.0\"\n[package.metadata.dist]\ndist = false\n")
	offenders := MembersNotInheritingVersion(filepath.Join(root, "rust"), []string{"devmap-cli", "devmap-store"})
	mustEqual(t, len(offenders), 1)
	if !anyContains(offenders, "devmap-store", "0.1.0") {
		t.Fatalf("offenders: %v", offenders)
	}
	// …and it reaches the gate, not just the helper.
	if errs := Inspect(DefaultSources(root), Options{}).Errors; !anyContains(errs, "devmap-store", "0.1.0") {
		t.Fatalf("errors: %v", errs)
	}
}

func TestInheritanceFailsAMemberThatSetsNoVersionAtAll(t *testing.T) {
	root := scratchTree(t, defaultTree())
	write(t, filepath.Join(root, "rust", "devmap-store", "Cargo.toml"),
		"[package]\nname = \"devmap-store\"\n[package.metadata.dist]\ndist = false\n")
	offenders := MembersNotInheritingVersion(filepath.Join(root, "rust"), []string{"devmap-store"})
	mustEqual(t, len(offenders), 1)
	if !anyContains(offenders, "version.workspace = true") {
		t.Fatalf("offenders: %v", offenders)
	}
}

func TestInheritanceReportsAMemberWithNoManifestRatherThanReadingItAsInheriting(t *testing.T) {
	root := scratchTree(t, defaultTree())
	offenders := MembersNotInheritingVersion(filepath.Join(root, "rust"), []string{"dc-nonexistent"})
	mustEqual(t, len(offenders), 1)
	if !anyContains(offenders, "no Cargo.toml") {
		t.Fatalf("offenders: %v", offenders)
	}
}

func TestInheritanceHoldsForThisRepository(t *testing.T) {
	// All 13 crates inherit the one workspace version.
	rustRoot := filepath.Join(repoRoot(t), "rust")
	members := ParseWorkspaceMembers(readFile(t, filepath.Join(rustRoot, "Cargo.toml")))
	if len(members) < 13 {
		t.Fatalf("members=%d", len(members))
	}
	mustEqual(t, MembersNotInheritingVersion(rustRoot, members), []string(nil))
}

func TestLockPackagesTellPathPackagesFromRegistryOnes(t *testing.T) {
	lock := "[[package]]\nname = \"dc-glob\"\nversion = \"0.2.3\"\n\n" +
		"[[package]]\nname = \"libc\"\nversion = \"0.2.189\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n"
	mustEqual(t, ParseCargoLockPackages(lock), []LockPackage{
		{Name: "dc-glob", Version: "0.2.3", HasVersion: true, Local: true},
		{Name: "libc", Version: "0.2.189", HasVersion: true, Local: false},
	})
}

func TestLockFailsANestedCratesLockThatKeptAWorkspaceCrateAtTheOldVersion(t *testing.T) {
	// The v1.3.5 shape: rust/Cargo.lock regenerated, rust/gusset-engine/Cargo.lock not.
	root := scratchTree(t, defaultTree())
	write(t, filepath.Join(root, "rust", "gusset-engine", "Cargo.lock"),
		"[[package]]\nname = \"devmap-store\"\nversion = \"0.1.9\"\n\n[[package]]\nname = \"gusset\"\nversion = \"0.0.1\"\n")
	rustRoot := filepath.Join(root, "rust")
	mustEqual(t, len(WorkspaceLockfiles(rustRoot)), 2)
	stale := StaleWorkspaceLockEntries(rustRoot, []string{"devmap-cli", "devmap-store"}, "0.2.0")
	mustEqual(t, len(stale), 1)
	if !anyContains(stale, "rust/gusset-engine/Cargo.lock", "devmap-store") {
		t.Fatalf("stale: %v", stale)
	}
	// …and it reaches the gate. The foreign path crate (gusset) is not ours to judge.
	errs := Inspect(DefaultSources(root), Options{}).Errors
	if !anyContains(errs, "gusset-engine", "0.1.9") || anyContains(errs, "records gusset ") {
		t.Fatalf("errors: %v", errs)
	}
}

func TestLockIgnoresARegistryCrateThatSharesAWorkspaceCratesName(t *testing.T) {
	root := scratchTree(t, defaultTree())
	write(t, filepath.Join(root, "rust", "Cargo.lock"),
		"[[package]]\nname = \""+DistPackage+"\"\nversion = \"0.2.0\"\n\n"+
			"[[package]]\nname = \"devmap-store\"\nversion = \"9.9.9\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n")
	mustEqual(t, StaleWorkspaceLockEntries(filepath.Join(root, "rust"), []string{"devmap-cli", "devmap-store"}, "0.2.0"), []string(nil))
}

func TestLockHoldsForThisRepository(t *testing.T) {
	// Every lock under rust/ records the product version.
	rustRoot := filepath.Join(repoRoot(t), "rust")
	manifest := readFile(t, filepath.Join(rustRoot, "Cargo.toml"))
	version, _ := ParseTomlSectionVersion(manifest, "[workspace.package]")
	umbrella := false
	for _, p := range WorkspaceLockfiles(rustRoot) {
		if strings.HasSuffix(p, filepath.Join("gusset-engine", "Cargo.lock")) {
			umbrella = true
		}
	}
	if !umbrella {
		t.Fatal("rust/gusset-engine/Cargo.lock is not among the lockfiles")
	}
	mustEqual(t, StaleWorkspaceLockEntries(rustRoot, ParseWorkspaceMembers(manifest), version), []string(nil))
}

func notesDir(t *testing.T, notes map[string]string) string {
	t.Helper()
	dir := t.TempDir()
	for name, body := range notes {
		write(t, filepath.Join(dir, name), body)
	}
	return dir
}

func TestReleaseLineNamesTheNextPatchMinorAndMajor(t *testing.T) {
	mustEqual(t, SuccessorVersions("0.2.3"), []string{"0.2.4", "0.3.0", "1.0.0"})
}

func TestReleaseLineFailsThe023To135JumpEveryFileAgreedOn(t *testing.T) {
	gaps := ReleaseLineGaps(notesDir(t, map[string]string{"v0.2.3.md": "0.2.3", "v1.3.5.md": "1.3.5"}), "1.3.5")
	mustEqual(t, len(gaps), 1)
	if !anyContains(gaps, "0.2.4, 0.3.0, 1.0.0") {
		t.Fatalf("gaps: %v", gaps)
	}
}

func TestReleaseLineAcceptsTheCurrentReleaseAndASingleStepPastIt(t *testing.T) {
	dir := notesDir(t, map[string]string{"v0.2.2.md": "0.2.2", "v0.2.3.md": "0.2.3"})
	mustEqual(t, ReleaseLineGaps(dir, "0.2.3"), []string(nil))
	mustEqual(t, ReleaseLineGaps(dir, "0.3.0"), []string(nil))
}

func TestReleaseLineFailsNotesNewerThanTheProductUntilMarkedWithdrawn(t *testing.T) {
	dir := notesDir(t, map[string]string{"v0.2.3.md": "0.2.3", "v1.3.5.md": "1.3.5"})
	if gaps := ReleaseLineGaps(dir, "0.2.3"); !anyContains(gaps, "v1.3.5") {
		t.Fatalf("gaps: %v", gaps)
	}
	write(t, filepath.Join(dir, "v1.3.5.md"), "# 1.3.5\n\n"+WithdrawnMarker+"\n")
	mustEqual(t, ReleaseLineGaps(dir, "0.2.3"), []string(nil))
}

func TestReleaseLineCountsTheMarkerOnlyOnALineOfItsOwn(t *testing.T) {
	mustEqual(t, NotesAreWithdrawn("# v1.3.5\n\n"+WithdrawnMarker+"\n> Withdrawn.\n"), true)
	mustEqual(t, NotesAreWithdrawn("Notes outside the line carry `"+WithdrawnMarker+"`.\n"), false)
}

func TestReleaseLineDoesNotLetTheProductVersionBeAWithdrawnOne(t *testing.T) {
	dir := notesDir(t, map[string]string{"v0.2.3.md": "0.2.3", "v1.3.5.md": WithdrawnMarker + "\n"})
	if gaps := ReleaseLineGaps(dir, "1.3.5"); !anyContains(gaps, "withdrawn") {
		t.Fatalf("gaps: %v", gaps)
	}
}

func TestReleaseLineHoldsForThisRepository(t *testing.T) {
	root := repoRoot(t)
	version, err := ParseNpmVersion(readFile(t, filepath.Join(root, "package.json")))
	if err != nil {
		t.Fatal(err)
	}
	mustEqual(t, ReleaseLineGaps(filepath.Join(root, "docs", "releases"), version), []string(nil))
}

func TestThisRepositoryOnlyDistablesTheCLIPackage(t *testing.T) {
	// So hold_uncommitted_child cannot ship.
	packages := BinaryPackages(filepath.Join(repoRoot(t), "rust"))
	var distable []string
	storeHeld := false
	for _, p := range packages {
		if p.Distable {
			distable = append(distable, p.Name)
		}
		if p.Name == "devmap-store" && !p.Distable {
			storeHeld = true
		}
	}
	mustEqual(t, distable, []string{DistPackage})
	if !storeHeld {
		t.Fatal("devmap-store is not held back from cargo-dist")
	}
}

func TestThisRepositoryIsConsistent(t *testing.T) {
	result := Inspect(DefaultSources(repoRoot(t)), Options{})
	mustEqual(t, result.Errors, []string(nil))
}

func TestLiteralStringMembersAndTargetsAreNotSkipped(t *testing.T) {
	// TOML literal strings are as valid as basic ones. A member the parser
	// skipped would escape both the inheritance and the dist checks.
	source := "[workspace]\nmembers = ['dc-store', \"dc-verify\", 'tools/soak']\n\n[workspace.package]\nversion = \"0.2.0\"\n"
	mustEqual(t, ParseWorkspaceMembers(source), []string{"dc-store", "dc-verify", "tools/soak"})
	mustEqual(t, ParseDistWorkspaceTargets("[dist]\ntargets = ['x86_64-unknown-linux-gnu', \"aarch64-apple-darwin\"]\n"),
		[]string{"x86_64-unknown-linux-gnu", "aarch64-apple-darwin"})
	// A quote of one kind inside a string of the other is part of the value.
	mustEqual(t, ParseWorkspaceMembers("[workspace]\nmembers = [\"it's\", 'say \"hi\"']\n"), []string{"it's", `say "hi"`})
}

func TestAMemberTheParserCannotReadFailsClosed(t *testing.T) {
	// Anything in the array that is not a string — a bare word, a table — is
	// a member the gate cannot see, which must not read as "all members fine".
	root := scratchTree(t, defaultTree())
	write(t, filepath.Join(root, "rust", "Cargo.toml"),
		"[workspace]\nmembers = [\"devmap-cli\", devmap-store]\n\n[workspace.package]\nversion = \"0.2.0\"\n")
	if errs := Inspect(DefaultSources(root), Options{}).Errors; !anyContains(errs, "members", "devmap-store") {
		t.Fatalf("errors: %v", errs)
	}
}
