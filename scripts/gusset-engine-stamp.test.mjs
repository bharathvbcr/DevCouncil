import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { InputError, engineInputs, parseToml, stampVerdict, writeStamp } from "./gusset-engine-stamp.mjs";

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
  put(path.join(repo, "rust/gusset-engine/Cargo.toml"), outer || /^\[workspace\]/m.test(engineToml) ? engineToml : `${engineToml}\n[workspace]\n`);
  put(path.join(repo, "rust/gusset-engine/Cargo.lock"), "# lock\n");
  put(path.join(repo, "rust/gusset-engine/src/lib.rs"), "// engine\n");
  put(path.join(repo, "rust/gusset-engine/src/bin/b.rs"), "// bin\n");
  put(path.join(repo, "rust/gusset-engine/build.rs"), "// build\n");
  put(path.join(repo, "rust/gusset-engine/tools/x/Cargo.toml"), "[package]\n");
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
  const OUTSIDE = /\[workspace\] (members|default-members|exclude) entry .* (resolves outside the crate directory|has a \.\. segment)/;
  it("members, default-members and exclude outside the crate directory are refused (literal and glob)", () => {
    for (const ws of [
      'members = ["../evil"]',
      'members = ["."]\ndefault-members = [".", "../evil"]',
      'exclude = ["../evil"]',
      'members = ["../*"]',
      'members = ["../../../evil/crates/*"]',
      'members = ["*/../../evil"]',
      'default-members = ["s[r]c/../../evil"]',
      'members = ["sr?/../../evil"]',
      'exclude = ["{a,b}/../../evil"]',
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

describe("symlinks under the inputs are refused, not followed", () => {
  const link = (f, target, rel) => {
    symlinkSync(target, path.join(f.repo, rel));
    git(f.repo, "add", "."); git(f.repo, "commit", "-qm", `link ${rel}`);
  };
  const SYMLINK = /is a symlink under the engine inputs/;
  it("a symlinked workspace member directory", () => {
    const f = fixture(`[dependencies]\n${GUSSET}[workspace]\nmembers = ["link"]\n`, { outer: true });
    mkdirSync(path.join(f.top, "outside"));
    link(f, path.join(f.top, "outside"), "rust/gusset-engine/link");
    refused(f, SYMLINK, "member");
  });
  it("a Cargo.lock that is a symlink", () => {
    const f = fixture();
    put(path.join(f.top, "outside.lock"), "# elsewhere\n");
    git(f.repo, "rm", "-q", "rust/gusset-engine/Cargo.lock");
    link(f, path.join(f.top, "outside.lock"), "rust/gusset-engine/Cargo.lock");
    refused(f, SYMLINK, "lock");
  });
  it("a path dependency through a symlinked directory, inside the crate or beside it", () => {
    for (const [dep, where] of [["vendor", "rust/gusset-engine/vendor"], ["../vendor", "rust/vendor"]]) {
      const f = fixture(`[dependencies]\n${GUSSET}evil = { path = "${dep}" }\n`);
      mkdirSync(path.join(f.top, "outside"));
      link(f, path.join(f.top, "outside"), where);
      refused(f, SYMLINK, dep);
    }
  });
});

describe("a duplicate table header is refused (cargo rejects it)", () => {
  it("[a] twice, [a.b] twice, but [[a]] repeated and a super-table after its child are fine", () => {
    assert.throws(() => parseToml("[a]\nx = 1\n[a]\ny = 2\n", "m"), /table header is defined twice/);
    assert.throws(() => parseToml("[a.b]\n[c]\n[a.b]\n", "m"), /table header is defined twice/);
    parseToml("[[a]]\nx = 1\n[a.s]\n[[a]]\n[a.s]\n[b.c]\n[b]\n", "m");
  });
  it("a manifest with two [workspace] headers is refused by the stamp", () => {
    refused(fixture(`[workspace]\n[dependencies]\n${GUSSET}[workspace]\n`, { outer: true }), /table header is defined twice/, "dup");
  });
});

describe("every path the stamp relies on must be in HEAD", () => {
  // gen/ and target/ are gitignored: their files are in no tree, and git status does not report them.
  const ignored = (toml, make) => {
    const f = fixture(toml, { outer: false });
    commit(f, ".gitignore", "target/\ngen/\n");
    make(f);
    return f;
  };
  const NOT_COMMITTED = /is not committed in HEAD|ignored directory|build output/;
  it("an [lib] path or build script in an ignored directory", () => {
    for (const toml of [`[lib]\npath = "gen/lib.rs"\n[dependencies]\n${GUSSET}`, `[package]\nbuild = "gen/build.rs"\n[dependencies]\n${GUSSET}`]) {
      const f = ignored(toml, (g) => put(path.join(g.repo, "rust/gusset-engine/gen/lib.rs"), "// x\n") || put(path.join(g.repo, "rust/gusset-engine/gen/build.rs"), "// x\n"));
      refused(f, /is not committed in HEAD/, toml);
    }
  });
  it("a literal or glob workspace member in an ignored directory", () => {
    for (const [ws, re] of [['members = ["gen/x"]', /is not committed in HEAD/], ['default-members = ["gen/x"]', /is not committed in HEAD/], ['members = ["gen/*"]', /ignored directory/]]) {
      const f = ignored(`[dependencies]\n${GUSSET}[workspace]\n${ws}\n`, (g) => put(path.join(g.repo, "rust/gusset-engine/gen/x/Cargo.toml"), "[package]\nname = 'x'\n"));
      refused(f, re, ws);
    }
  });
  it("a member (or anything) under the engine's target/", () => {
    const f = ignored(`[dependencies]\n${GUSSET}[workspace]\nmembers = ["target/link"]\n`, (g) => {
      mkdirSync(path.join(g.repo, "rust/gusset-engine/target"), { recursive: true });
      symlinkSync(g.top, path.join(g.repo, "rust/gusset-engine/target/link"));
    });
    refused(f, /build output/, "target member");
    const g = ignored(`[lib]\npath = "target/gen.rs"\n[dependencies]\n${GUSSET}`, (h) => put(path.join(h.repo, "rust/gusset-engine/target/gen.rs"), "// x\n"));
    refused(g, /build output/, "target lib");
  });
  it("a path dependency or Cargo.lock that is not in HEAD", () => {
    const f = ignored(`[dependencies]\n${GUSSET}evil = { path = "gen/dep" }\n`, (g) => put(path.join(g.repo, "rust/gusset-engine/gen/dep/Cargo.toml"), "[package]\nname = 'dep'\n"));
    refused(f, NOT_COMMITTED, "dep");
    const h = fixture();
    commit(h, ".gitignore", "target/\nCargo.lock\n");
    git(h.repo, "rm", "-q", "--cached", "rust/gusset-engine/Cargo.lock");
    git(h.repo, "commit", "-qm", "untrack lock");
    refused(h, /Cargo\.lock is not committed in HEAD/, "lock");
  });
});

describe("one rule: nothing under an input may be neither in HEAD nor shown dirty", () => {
  const E = "rust/gusset-engine";
  const UNSEEN = /under the engine inputs/;
  it("an ignored auto-discovered build.rs", () => {
    const f = fixture();
    git(f.repo, "rm", "-q", `${E}/build.rs`);
    commit(f, ".gitignore", "target/\nbuild.rs\n");
    put(path.join(f.repo, E, "build.rs"), "// ignored, but cargo runs it\n");
    refused(f, /ignored file\(s\) under the engine inputs \(rust\/gusset-engine\/build\.rs\)/, "build.rs");
  });
  it("a glob member expanding into an ignored subdirectory", () => {
    const f = fixture(`[dependencies]\n${GUSSET}[workspace]\nmembers = ["crates/*"]\n`, { outer: true });
    commit(f, `${E}/crates/a/Cargo.toml`, "[package]\nname = 'a'\n");
    commit(f, ".gitignore", "target/\ngen/\n");
    put(path.join(f.repo, E, "crates/gen/Cargo.toml"), "[package]\nname = 'gen'\n");
        refused(f, /ignored file\(s\) under the engine inputs \(rust\/gusset-engine\/crates\/gen\/\)/, "crates/gen");
  });
  it("an ignored src/gen.rs, a skip-worktree file and an assume-unchanged file", () => {
    const f = fixture();
    commit(f, ".gitignore", "target/\ngen.rs\n");
    put(path.join(f.repo, E, "src/gen.rs"), "// ignored module\n");
    refused(f, /ignored file\(s\).*src\/gen\.rs/, "ignored module");
    rmSync(path.join(f.repo, E, "src/gen.rs"));
    assert.ok(E in inputs(f).stamp.trees);
    git(f.repo, "update-index", "--skip-worktree", `${E}/src/lib.rs`);
    put(path.join(f.repo, E, "src/lib.rs"), "// edited, and git status will not say so\n");
    refused(f, /index-flagged.*src\/lib\.rs/, "skip-worktree");
    git(f.repo, "update-index", "--no-skip-worktree", `${E}/src/lib.rs`);
    git(f.repo, "update-index", "--assume-unchanged", `${E}/src/lib.rs`);
    refused(f, /index-flagged.*src\/lib\.rs/, "assume-unchanged");
  });
  it("a gitlink under the inputs", () => {
    const f = fixture();
    git(f.repo, "update-index", "--add", "--cacheinfo", `160000,${git(f.repo, "rev-parse", "HEAD")},${E}/sub`);
    git(f.repo, "commit", "-qm", "gitlink");
    refused(f, /rust\/gusset-engine\/sub is a gitlink/, "gitlink");
  });
  it("the crate's own target/ is skipped, and a message lists at most three files and the count", () => {
    const f = fixture();
    put(path.join(f.repo, E, "target/release/libgusset.a"), "a");
    assert.ok(E in inputs(f).stamp.trees);
    commit(f, ".gitignore", "target/\n*.bak\n");
    for (const n of "abcde") put(path.join(f.repo, E, `src/${n}.bak`), "x");
    refused(f, /5 ignored file\(s\) under the engine inputs \(.*, and 2 more\)/, "count");
  });
  it("a git that does not finish is a refusal, and every git child runs with fsmonitor disabled", () => {
    const f = fixture();
    const bin = path.join(f.top, "bin");
    const log = path.join(f.top, "env.log");
    const real = spawnSync("which", ["git"], { encoding: "utf8" }).stdout.trim();
    put(path.join(bin, "git"), `#!/bin/sh\necho "$GIT_CONFIG_COUNT $GIT_CONFIG_KEY_0 $GIT_CONFIG_VALUE_0" >> '${log}'\nif [ -n "$SLOW" ]; then sleep 5; fi\nexec '${real}' "$@"\n`);
    chmodSync(path.join(bin, "git"), 0o755);
    const saved = { PATH: process.env.PATH, MS: process.env.GUSSET_STAMP_GIT_TIMEOUT_MS };
    try {
      process.env.PATH = `${bin}${path.delimiter}${saved.PATH}`;
      assert.ok(E in inputs(f).stamp.trees);
      assert.match(readFileSync(log, "utf8"), /^1 core\.fsmonitor false$/m);
      process.env.SLOW = "1";
      process.env.GUSSET_STAMP_GIT_TIMEOUT_MS = "300";
      refused(f, /did not finish within 300 ms/, "timeout");
    } finally {
      delete process.env.SLOW;
      process.env.PATH = saved.PATH;
      if (saved.MS === undefined) delete process.env.GUSSET_STAMP_GIT_TIMEOUT_MS;
      else process.env.GUSSET_STAMP_GIT_TIMEOUT_MS = saved.MS;
    }
  });
});

describe("only the engine's own target/ is tolerated build output", () => {
  const E = "rust/gusset-engine";
  it("an ignored rust/dc-glob/target/g.rs is refused", () => {
    const f = fixture();
    put(path.join(f.repo, "rust/dc-glob/target/g.rs"), "// a source, not build output\n");
    refused(f, /ignored file\(s\) under the engine inputs \(rust\/dc-glob\/target\/\)/, "dc-glob target");
  });
  it("the engine's real target/ is still exempt", () => {
    const f = fixture();
    put(path.join(f.repo, E, "target/release/libgusset.a"), "a");
    assert.ok(E in inputs(f).stamp.trees);
  });
  it("a members glob that starts with a glob can expand into the engine's target/, and is refused", () => {
    for (const member of ["*/x", "t*/x", "?arget/x", "[t]arget/x", "{target,src}/x"]) {
      const f = fixture(`[dependencies]\n${GUSSET}[workspace]\nmembers = ["${member}"]\n`, { outer: true });
      put(path.join(f.repo, E, "target/x/Cargo.toml"), "[package]\nname = 'x'\n");
      refused(f, /starts with a glob in its first path segment/, member);
    }
  });
});
