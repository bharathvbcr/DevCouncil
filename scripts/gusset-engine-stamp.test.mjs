import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { InputError, engineInputs, stampVerdict, writeStamp } from "./gusset-engine-stamp.mjs";

// Manifest forms the engine stamp must read or refuse. The six escaping forms
// are the ones the 2026-11-24 review (of a64f991b) showed being accepted.

const GIT_ENV = {
  ...process.env,
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_SYSTEM: "/dev/null",
  GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.invalid",
  GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.invalid",
};
const roots = [];
after(() => roots.forEach((r) => rmSync(r, { recursive: true, force: true })));

function git(cwd, ...args) {
  const r = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8", env: GIT_ENV });
  assert.equal(r.status, 0, `git ${args.join(" ")}: ${r.stderr}`);
  return r.stdout.trim();
}
const put = (file, text) => { mkdirSync(path.dirname(file), { recursive: true }); writeFileSync(file, text); };

const GUSSET = 'gusset_core = { package = "gusset", path = "../../../gusset/crates/gusset" }\n';
const WORKSPACE = '[workspace]\nmembers = ["dc-glob"]\n\n[workspace.package]\nversion = "0.2.5"\nlicense = "Apache-2.0"\n\n[workspace.lints.rust]\nunsafe_code = "forbid"\n';
const DC_GLOB = '[package]\nname = "dc-glob"\nversion.workspace = true\nlicense.workspace = true\n\n[lints]\nworkspace = true\n';

/** A repository with rust/Cargo.toml (the workspace root), rust/dc-glob (a member), the engine (its own workspace), and a sibling gusset. */
function fixture(engineToml = `[dependencies]\n${GUSSET}dc-glob = { path = "../dc-glob" }\n`, { outer = false } = {}) {
  const top = realpathSync(mkdtempSync(path.join(tmpdir(), "engine-stamp-")));
  roots.push(top);
  const gusset = path.join(top, "gusset");
  put(path.join(gusset, "README"), "g\n");
  git(top, "init", "-q", gusset);
  git(gusset, "add", "."); git(gusset, "commit", "-qm", "g");
  const repo = path.join(top, "DevCouncil");
  git(top, "init", "-q", repo);
  put(path.join(repo, ".gitignore"), "target/\n");
  put(path.join(repo, "rust/Cargo.toml"), WORKSPACE);
  put(path.join(repo, "rust/dc-glob/Cargo.toml"), DC_GLOB);
  put(path.join(repo, "rust/dc-glob/src/lib.rs"), "// glob\n");
  put(path.join(repo, "rust/gusset-engine/Cargo.toml"), outer ? engineToml : `${engineToml}\n[workspace]\n`);
  put(path.join(repo, "rust/gusset-engine/Cargo.lock"), "# lock\n");
  put(path.join(repo, "rust/gusset-engine/src/lib.rs"), "// engine\n");
  put(path.join(repo, "rust-toolchain.toml"), '[toolchain]\nchannel = "1.100"\n');
  git(repo, "add", "."); git(repo, "commit", "-qm", "m");
  return { top, gusset, repo };
}
const commit = (f, rel, text) => { put(path.join(f.repo, rel), text); git(f.repo, "add", "."); git(f.repo, "commit", "-qm", rel); };
const inputs = (f) => engineInputs(f.repo, f.gusset);
const refused = (f, pattern, label) => assert.throws(() => inputs(f), (e) => e instanceof InputError && pattern.test(e.message), label);

describe("manifest path forms that escape the repository are refused, never skipped", () => {
  const OUTSIDE_DEP = /path dependency \.\.\/\.\.\/\.\.\/evil outside the repository and outside/;
  const cases = {
    "single-quoted path in an inline table": [`[dependencies]\n${GUSSET}evil = { path = '../../../evil' }\n`, OUTSIDE_DEP],
    "bare [patch] with a nested inline table": [`[dependencies]\n${GUSSET}[patch]\ncrates-io = { foo = { path = "../../../evil" } }\n`, OUTSIDE_DEP],
    "[workspace] dotted dependencies.foo = { path }": [`[dependencies]\n${GUSSET}[workspace]\ndependencies.foo = { path = "../../../evil" }\n`, OUTSIDE_DEP],
    "top-level dotted dependencies.foo.path": [`dependencies.foo.path = "../../../evil"\n[dependencies]\n${GUSSET}`, OUTSIDE_DEP],
    "top-level lib = { path } outside the crate": [`lib = { path = "../x.rs" }\n[dependencies]\n${GUSSET}`, /\[lib\] path \.\.\/x\.rs resolves outside the crate/],
    "[package] build outside the crate": [`[package]\nbuild = "../../../evil/build.rs"\n[dependencies]\n${GUSSET}`, /\[package\] build \.\.\/\.\.\/\.\.\/evil\/build\.rs resolves outside the crate/],
    "multi-line [dependencies.evil] (already refused)": [`[dependencies]\n${GUSSET}[dependencies.evil]\npath = "../../../evil"\n`, OUTSIDE_DEP],
    "single-quoted [lib] path": [`[lib]\npath = '../x.rs'\n[dependencies]\n${GUSSET}`, /\[lib\] path/],
    "dependencies in a quoted table header": [`[dependencies]\n${GUSSET}["dev-dependencies"]\nevil = { path = "../../../evil" }\n`, OUTSIDE_DEP],
    "target-specific dependencies with a cfg key": [`[dependencies]\n${GUSSET}[target.'cfg(unix)'.dependencies]\nevil = { path = "../../../evil" }\n`, OUTSIDE_DEP],
    "path in a multi-line inline table": [`[dependencies]\n${GUSSET}evil = {\n  path = "../../../evil",\n}\n`, OUTSIDE_DEP],
  };
  for (const [name, [toml, pattern]] of Object.entries(cases)) {
    it(name, () => {
      const f = fixture(toml);
      refused(f, pattern, name);
    });
  }
  it("a path key outside every table the stamp classifies is refused by name", () => {
    for (const toml of [
      `[dependencies]\n${GUSSET}[features]\npath = []\n`,
      `[dependencies]\n${GUSSET}[unknown-table]\nthing = { path = "../../../evil" }\n`,
    ]) {
      refused(fixture(toml), /path .*not in a table the engine stamp classifies/, toml);
    }
  });
  it("a manifest it cannot parse completely is refused, not read as far as it goes", () => {
    for (const toml of [
      `[dependencies]\n${GUSSET}evil = { path = "../../../evil\n`,
      `[dependencies\n${GUSSET}`,
      `[dependencies]\n${GUSSET}evil = { path = ../../../evil }\n[other]\nx = [1, 2\n`,
    ]) {
      refused(fixture(toml), /cannot read this manifest/, toml);
    }
  });
});

describe("manifest forms the stamp reads", () => {
  it("single-quoted, dotted, quoted-header and multi-line path dependencies inside the repository are inputs", () => {
    for (const toml of [
      `[dependencies]\n${GUSSET}dc-glob = { path = '../dc-glob' }\n`,
      `dependencies.gusset_core = { package = "gusset", path = "../../../gusset/crates/gusset" }\ndependencies.dc-glob.path = "../dc-glob"\n`,
      `["dependencies"]\n${GUSSET}"dc-glob" = { path = "../dc-glob" }\n`,
      `[dependencies]\n${GUSSET}dc-glob = {\n  path = "../dc-glob", # ../../../evil\n}\n`,
      `[dependencies]\n${GUSSET}[patch.crates-io]\ndc-glob = { path = "../dc-glob" }\n`,
    ]) {
      const f = fixture(toml);
      assert.ok("rust/dc-glob" in inputs(f).stamp.trees, toml);
    }
  });
  it("a path in a comment, a string value or a basic-string escape is not a path key", () => {
    const f = fixture(`# path = '../../../evil'\n[package]\ndescription = "path = '../../../evil'"\nbuild = "build.rs"\n[dependencies]\n${GUSSET}dc-glob = { path = "../dc-glob" } # path = "../../../evil"\n`);
    assert.deepEqual(Object.keys(inputs(f).stamp.trees).sort(), ["rust-toolchain.toml", "rust/Cargo.toml", "rust/dc-glob", "rust/gusset-engine"]);
  });
  it("[lib]/[[bin]] paths inside the crate and build = false are fine", () => {
    const f = fixture(`[package]\nbuild = false\n[lib]\npath = "src/lib.rs"\n[[bin]]\nname = "b"\npath = 'src/bin/b.rs'\n[dependencies]\n${GUSSET}`);
    assert.ok("rust/gusset-engine" in inputs(f).stamp.trees);
  });
});

describe("inherited workspace keys are inputs", () => {
  it("rust/dc-glob inherits version, license and lints from rust/Cargo.toml: changing any changes the stamp", () => {
    const f = fixture();
    const base = inputs(f).stamp;
    assert.ok("rust/Cargo.toml" in base.trees, "the workspace root manifest is an input");
    const dir = path.join(f.repo, "rust/gusset-engine/target/release");
    put(path.join(dir, "libgusset.a"), "archive");
    writeStamp(f.repo, f.gusset, dir);
    assert.equal(stampVerdict(inputs(f), dir).ok, true);
    for (const [what, from, to] of [
      ["version", 'version = "0.2.5"', 'version = "0.2.6"'],
      ["license", 'license = "Apache-2.0"', 'license = "MIT"'],
      ["lints", 'unsafe_code = "forbid"', 'unsafe_code = "deny"'],
    ]) {
      commit(f, "rust/Cargo.toml", WORKSPACE.replace(from, to));
      const verdict = stampVerdict(inputs(f), dir);
      assert.equal(verdict.ok, false, what);
      assert.match(verdict.reason, /rust\/Cargo\.toml/, what);
      commit(f, "rust/Cargo.toml", WORKSPACE);
      assert.equal(stampVerdict(inputs(f), dir).ok, true, `${what} restored`);
    }
  });
  it("a crate that inherits nothing does not pull in the workspace root", () => {
    const f = fixture();
    commit(f, "rust/dc-glob/Cargo.toml", '[package]\nname = "dc-glob"\nversion = "0.1.0"\n');
    assert.ok(!("rust/Cargo.toml" in inputs(f).stamp.trees));
  });
  it("an inherited path dependency is followed through [workspace.dependencies] of the root", () => {
    const f = fixture();
    commit(f, "rust/other/Cargo.toml", '[package]\nname = "other"\n');
    commit(f, "rust/Cargo.toml", `${WORKSPACE}\n[workspace.dependencies]\nother = { path = "other" }\n`);
    commit(f, "rust/dc-glob/Cargo.toml", `${DC_GLOB}\n[dependencies]\nother.workspace = true\n`);
    assert.ok("rust/other" in inputs(f).stamp.trees);
    commit(f, "rust/Cargo.toml", `${WORKSPACE}\n[workspace.dependencies]\nother = { path = "../../evil" }\n`);
    refused(f, /path dependency \.\.\/\.\.\/evil outside the repository/, "inherited outside path");
  });
  it("inheriting from a workspace root the repository does not contain is refused", () => {
    const f = fixture();
    commit(f, "rust/Cargo.toml", "[package]\nname = 'not-a-workspace'\n");
    refused(f, /inherits from a workspace root but none was found/, "no root");
  });
  it("an inherited dependency the root does not declare is refused", () => {
    const f = fixture();
    commit(f, "rust/dc-glob/Cargo.toml", `${DC_GLOB}\n[dependencies]\nmissing.workspace = true\n`);
    refused(f, /missing.*not declared in .*workspace\.dependencies/, "undeclared");
  });
});

describe("workspace members and the engine's own workspace root", () => {
  const OUTSIDE = /\[workspace\] (members|default-members|exclude) entry .* resolves outside the crate directory/;
  it("members, default-members and exclude outside the crate directory are refused (literal and glob)", () => {
    for (const ws of [
      'members = ["../evil"]',
      'members = ["."]\ndefault-members = [".", "../evil"]',
      'exclude = ["../evil"]',
      'members = ["../*"]',
      'members = ["../../../evil/crates/*"]',
    ]) {
      refused(fixture(`[dependencies]\n${GUSSET}[workspace]\n${ws}\n`, { outer: true }), OUTSIDE, ws);
    }
  });
  it("members that stay inside the crate, or an empty [workspace], are fine; a non-list is refused", () => {
    for (const ws of ["", 'members = ["."]', 'members = ["sub/*", "tools/x"]', "members = []"]) {
      assert.ok("rust/gusset-engine" in inputs(fixture(`[dependencies]\n${GUSSET}[workspace]\n${ws}\n`, { outer: true })).stamp.trees, ws);
    }
    refused(fixture(`[dependencies]\n${GUSSET}[workspace]\nmembers = "x"\n`, { outer: true }), /members is not a list of strings/, "string");
  });
  it("an engine without its own [workspace] is refused: an outer root's patch, profile and lockfile are invisible", () => {
    for (const toml of [
      `[package]\nworkspace = ".."\n[dependencies]\n${GUSSET}`,
      `[package]\nname = "e"\n[dependencies]\n${GUSSET}`,
    ]) {
      refused(fixture(toml, { outer: true }), /must declare its own \[workspace\]/, toml);
    }
  });
  it("an engine without a Cargo.lock in its directory is refused", () => {
    const f = fixture();
    git(f.repo, "rm", "-q", "rust/gusset-engine/Cargo.lock");
    git(f.repo, "commit", "-qm", "rm lock");
    refused(f, /Cargo\.lock is missing/, "lock");
  });
});

describe("manifest spellings cargo accepts", () => {
  it("a BOM, space-separated datetimes, the \\e escape and path under [package.metadata] are not refusals", () => {
    const f = fixture(`\uFEFF[package]\nname = "e"\nwhen = 1979-05-27 07:32:00Z\nesc = "a\\e[0m"\nd = 1979-05-27\n[package.metadata.x]\npath = "../../../evil"\n[dependencies]\n${GUSSET}dc-glob = { path = "../dc-glob" }\n`);
    assert.ok("rust/dc-glob" in inputs(f).stamp.trees);
  });
});

describe("refusals say where, and the line names a way out", () => {
  it("an unclassified path names file:line", () => {
    refused(fixture(`[dependencies]\n${GUSSET}\n[unknown]\nx = { path = "../../../evil" }\n`), /rust\/gusset-engine\/Cargo\.toml:\d+: a path key at unknown\.x /, "line");
  });
  it("a dependency and a target path name file:line", () => {
    refused(fixture(`[dependencies]\n${GUSSET}evil = { path = "../../../evil" }\n`), /Cargo\.toml:3 has a path dependency/, "dep line");
    refused(fixture(`[dependencies]\n${GUSSET}[lib]\npath = "../x.rs"\n`), /Cargo\.toml:3 \[lib\] path/, "target line");
  });
});

describe("every inherited key form is an input", () => {
  const cases = {
    "edition.workspace": '[package]\nname = "dc-glob"\nedition.workspace = true\n',
    "table-form [package.rust-version]": '[package]\nname = "dc-glob"\n\n[package.rust-version]\nworkspace = true\n',
    "[lints] workspace": '[package]\nname = "dc-glob"\n\n[lints]\nworkspace = true\n',
    "a workspace dependency with features": '[package]\nname = "dc-glob"\n\n[dependencies]\nserde = { workspace = true, features = ["derive"] }\n',
  };
  for (const [name, toml] of Object.entries(cases)) {
    it(name, () => {
      const f = fixture();
      commit(f, "rust/Cargo.toml", `${WORKSPACE}\n[workspace.dependencies]\nserde = "1"\n`);
      commit(f, "rust/dc-glob/Cargo.toml", toml);
      assert.ok("rust/Cargo.toml" in inputs(f).stamp.trees, name);
    });
  }
});
