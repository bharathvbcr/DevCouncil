import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";
import {
  INSTALL_HINT,
  hasWorkflowLevelPermissions,
  interpret,
  membersOutOfScope,
  readWorkflows,
  structuralFailures,
  testedPackages,
  triggerPaths,
  workflowsMissingConcurrency,
  workflowsNamingAChannel,
  workspaceMembers,
  workflowFiles,
  workflowsMissingPermissions,
} from "./check-workflows.mjs";

describe("check:workflows", () => {
  it("finds the repository's workflow files", () => {
    const files = workflowFiles(fileURLToPath(new URL("../.github/workflows", import.meta.url)));
    assert.ok(files.includes("ci.yml"));
    assert.ok(files.includes("release.yml"));
    assert.ok(files.includes("npm-publish.yml"));
    assert.ok(files.includes("rust.yml"));
    assert.ok(files.includes("analysis-plane.yml"));
  });

  it("reports a missing directory as empty rather than throwing", () => {
    assert.deepEqual(workflowFiles("/nonexistent/workflows"), []);
  });

  it("separates 'actionlint is absent' from 'workflows are faulty'", () => {
    const enoent = Object.assign(new Error("spawn actionlint ENOENT"), { code: "ENOENT" });
    assert.deepEqual(interpret({ status: null, error: enoent }), { code: 2, message: INSTALL_HINT });
    assert.equal(interpret({ status: 1 }).code, 1);
    assert.deepEqual(interpret({ status: 0 }), { code: 0, message: null });
  });

  it("treats actionlint's own failure codes as a check that could not run", () => {
    assert.equal(interpret({ status: 2 }).code, 2);
    assert.equal(interpret({ status: 3 }).code, 2);
  });

  it("requires a workflow-level permissions block so GITHUB_TOKEN is fail-closed", () => {
    assert.equal(hasWorkflowLevelPermissions("on: push\njobs:\n  a:\n    runs-on: ubuntu-latest\n"), false);
    assert.equal(
      hasWorkflowLevelPermissions(
        "on: push\npermissions:\n  contents: read\njobs:\n  a:\n    runs-on: ubuntu-latest\n",
      ),
      true,
    );
    assert.equal(
      hasWorkflowLevelPermissions("on: push\njobs:\n  a:\n    permissions:\n      contents: read\n"),
      false,
    );

    const dir = fileURLToPath(new URL("../.github/workflows", import.meta.url));
    assert.deepEqual(workflowsMissingPermissions(dir), []);
  });

  it("builds the Go host only after the Gusset sibling and staticlib exist", () => {
    const root = fileURLToPath(new URL("../.github/workflows", import.meta.url));
    const uses = (name) =>
      readFileSync(path.join(root, name), "utf8")
        .split("\n")
        .filter((line) => line.includes("./.github/actions/setup-gusset")).length;
    // One step per job that compiles the host. Dropping a call site puts
    // `replacement directory ../../../gusset does not exist` back on the tag.
    assert.equal(uses("ci.yml"), 1);
    assert.equal(uses("analysis-plane.yml"), 3);
    // verify.sh gates, embedder shape, and the static musl host.
    assert.equal(uses("rust.yml"), 3);
    assert.equal(uses("codeql.yml"), 1);
  });

  it("pins release actions and keeps contents write off pull_request jobs", () => {
    const source = readFileSync(
      fileURLToPath(new URL("../.github/workflows/release.yml", import.meta.url)),
      "utf8",
    );
    const head = source.slice(0, source.search(/^jobs\s*:/m));
    assert.match(head, /"contents": "read"/);
    assert.doesNotMatch(head, /"contents": "write"/);
    for (const ref of [
      "actions/checkout@v7",
      "actions/setup-node@v7",
      "actions/setup-go@v7",
      "actions/upload-artifact@v7",
      "actions/download-artifact@v8",
    ]) {
      assert.equal(source.includes(`uses: ${ref}\n`) || source.includes(`uses: ${ref} `), false, ref);
    }
    const pins = {
      "actions/checkout": "3d3c42e5aac5ba805825da76410c181273ba90b1 # v7",
      "actions/setup-node": "949feb2413d6458794dcd2491c4babbbce0c15c1 # v7",
      "actions/setup-go": "b7ad1dad31e06c5925ef5d2fc7ad053ef454303e # v7",
      "actions/upload-artifact": "cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7",
      "actions/download-artifact": "9000827ccba6bdab643e8b6fd33ac0654aef8333 # v8",
    };
    for (const [action, pin] of Object.entries(pins)) {
      assert.ok(source.includes(`uses: ${action}@${pin}`), action);
    }
    const job = (name) => {
      const matched = source.match(new RegExp(`\\n  ${name}:\\n([\\s\\S]*?)(?=\\n  [a-z0-9-]+:\\n|$)`));
      assert.ok(matched, name);
      return matched[1];
    };
    for (const name of ["identity", "plan", "build-local-artifacts", "build-global-artifacts", "announce"]) {
      assert.match(job(name), /"contents": "read"/, name);
      assert.doesNotMatch(job(name), /"contents": "write"/, name);
    }
    for (const name of ["create-release", "host"]) {
      assert.match(job(name), /"contents": "write"/, name);
    }
    assert.ok(source.includes("persist-credentials: false"));
    assert.ok(source.includes("dist host --steps=create"));
    assert.ok(source.includes("dist host ${{ needs.plan.outputs.tag-flag }} --steps=upload --steps=release"));
  });

  it("invokes the archive gate before publishing, including the Windows zip", () => {
    const source = readFileSync(
      fileURLToPath(new URL("../.github/workflows/release.yml", import.meta.url)),
      "utf8",
    );
    const artifactChecks = source.split("\n").filter((line) => line.includes("./cmd/dcrelease --artifacts"));
    assert.ok(
      artifactChecks.length >= 2,
      "release.yml must run the archive gate before cargo-dist publish and again after the flattened download",
    );
  });

  it("passes the structural checks on this repository", () => {
    assert.deepEqual(structuralFailures(fileURLToPath(new URL("..", import.meta.url))), []);
  });

  describe("workspace scope", () => {
    const members = [
      { dir: "devmap-a", name: "devmap-a" },
      { dir: "dc-b", name: "dc-b" },
    ];
    const rust = `on:
  push:
    paths:
      - "rust/devmap-*/**"
jobs:
  t:
    steps:
      - run: cargo test --workspace --exclude dc-b --no-fail-fast
`;

    it("flags a member no workflow names or triggers on", () => {
      const out = membersOutOfScope(members, { "rust.yml": rust });
      assert.deepEqual(out.map((m) => m.name), ["dc-b"]);
      assert.equal(out[0].tested, false);
    });

    it("flags a planted crate that is tested by --workspace but outside every paths filter", () => {
      const planted = [...members, { dir: "planted", name: "planted" }];
      const out = membersOutOfScope(planted, { "rust.yml": rust.replace(" --exclude dc-b", " --exclude dc-b") });
      assert.deepEqual(out.map((m) => m.name).sort(), ["dc-b", "planted"]);
      assert.equal(out.find((m) => m.name === "planted").tested, true);
    });

    it("accepts a member another workflow triggers on and names with -p", () => {
      const analysis = `on:
  push:
    paths:
      - "rust/**"
jobs:
  t:
    steps:
      - run: cargo test -p dc-b --no-fail-fast
`;
      assert.deepEqual(membersOutOfScope(members, { "rust.yml": rust, "analysis-plane.yml": analysis }), []);
    });

    it("reads -p, a for-loop list, line continuations and ignores comments", () => {
      const names = ["a", "b", "c", "d"];
      const yaml = `jobs:
  t:
    steps:
      # cargo test -p d
      - run: |
          for crate in a b; do
            cargo test -p "$crate"
          done
          cargo test -p c \
            --no-fail-fast
          cargo test -p d --test only_one
`;
      assert.deepEqual([...testedPackages(yaml, names)].sort(), ["a", "b", "c"]);
    });

    it("treats a workflow with no paths filter as covering every path", () => {
      assert.equal(triggerPaths("on:\n  push:\n    branches: [main]\njobs:\n"), "all");
    });

    it("lists this repository's real members", () => {
      const rustDir = fileURLToPath(new URL("../rust", import.meta.url));
      const names = workspaceMembers(rustDir).map((m) => m.name);
      assert.ok(names.includes("dc-regress"));
      assert.ok(names.includes("devmap-soak"));
    });
  });

  it("refuses a workflow that names a Rust channel, or has no concurrency group", () => {
    assert.deepEqual(
      workflowsNamingAChannel({
        "a.yml": "jobs:\n  t:\n    steps:\n      - uses: dtolnay/rust-toolchain@stable\n",
        "b.yml": "jobs:\n  t:\n    steps:\n      - uses: ./.github/actions/setup-rust\n",
      }),
      ["a.yml"],
    );
    assert.deepEqual(
      workflowsMissingConcurrency({
        "a.yml": "on: push\njobs:\n  t: {}\n",
        "b.yml": "on: push\nconcurrency:\n  group: g\njobs:\n  t: {}\n",
      }),
      ["a.yml"],
    );
    const real = readWorkflows(fileURLToPath(new URL("../.github/workflows", import.meta.url)));
    assert.deepEqual(workflowsNamingAChannel(real), []);
    assert.deepEqual(workflowsMissingConcurrency(real), []);
  });
});
