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
  cargoStepsMissingLocked,
  structuralFailures,
  testsupportPathsMissing,
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
      const out = membersOutOfScope(planted, { "rust.yml": rust });
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

  describe("testsupport path filter", () => {
    const covered = [
      "on:",
      "  push:",
      "    paths:",
      '      - "rust/devmap-*/**"',
      '      - "rust/testsupport/**"',
      "  pull_request:",
      "    paths:",
      '      - "rust/testsupport/**"',
      "jobs:",
      "  test: {}",
      "",
    ].join("\n");

    it("accepts a push and a pull_request filter that each match a nested testsupport path", () => {
      assert.deepEqual(testsupportPathsMissing(covered, "rust.yml"), []);
      const unfiltered = "on:\n  push:\n  pull_request:\njobs:\n  test: {}\n";
      assert.deepEqual(testsupportPathsMissing(unfiltered, "rust.yml"), []);
    });

    it("rejects a filter that names the file only in a comment, only on one event, or only after jobs", () => {
      const comment = covered.replace('      - "rust/testsupport/**"\n', '      # - "rust/testsupport/**"\n');
      assert.deepEqual(testsupportPathsMissing(comment, "rust.yml"), [
        "rust.yml: push paths do not match rust/testsupport/**",
      ]);
      const pushOnly = covered.replace(
        '      - "rust/testsupport/**"\njobs:',
        "jobs:",
      );
      assert.ok(
        testsupportPathsMissing(pushOnly, "rust.yml").some((line) => line.includes("pull_request")),
      );
      const afterJobs = [
        "on:",
        "  push:",
        "    paths:",
        '      - "rust/devmap-*/**"',
        "  pull_request:",
        "    paths:",
        '      - "rust/devmap-*/**"',
        "jobs:",
        '  test:\n    # - "rust/testsupport/**"',
        "",
      ].join("\n");
      const missed = testsupportPathsMissing(afterJobs, "rust.yml");
      assert.equal(missed.length, 2);
      assert.deepEqual(testsupportPathsMissing(covered.replace(/\n/g, "\r\n"), "rust.yml"), []);
      assert.deepEqual(
        testsupportPathsMissing(covered.replaceAll("rust/testsupport/**", "rust/testsupport/*"), "rust.yml"),
        [
          "rust.yml: push paths do not match rust/testsupport/**",
          "rust.yml: pull_request paths do not match rust/testsupport/**",
        ],
      );
    });

    it("rejects a missing label and an oversized workflow", () => {
      assert.throws(() => testsupportPathsMissing(covered, ""), TypeError);
      assert.throws(() => testsupportPathsMissing("x".repeat(2_000_001), "rust.yml"), RangeError);
    });
  });

  describe("cargo --locked", () => {
    const root = fileURLToPath(new URL("..", import.meta.url));

    it("rejects a build, test, check, or clippy line that omits --locked and accepts one that has it", () => {
      for (const verb of ["build", "test", "check", "clippy"]) {
        assert.deepEqual(cargoStepsMissingLocked(`cargo ${verb} --locked`, "f"), []);
        assert.deepEqual(cargoStepsMissingLocked(`cargo ${verb}`, "f"), [`f:1: cargo ${verb} omits --locked`]);
        assert.deepEqual(cargoStepsMissingLocked(`cargo ${verb} -- --locked`, "f"), [
          `f:1: cargo ${verb} omits --locked`,
        ]);
      }
    });

    it("requires --locked on the rust workflows and verify.sh", () => {
      const workflows = readWorkflows(path.join(root, ".github", "workflows"));
      const verify = readFileSync(path.join(root, "rust", "verify.sh"), "utf8");
      assert.deepEqual(
        [
          ...cargoStepsMissingLocked(workflows["rust.yml"], "rust.yml"),
          ...cargoStepsMissingLocked(workflows["analysis-plane.yml"], "analysis-plane.yml"),
          ...cargoStepsMissingLocked(verify, "rust/verify.sh"),
        ],
        [],
      );
    });

    it("keeps --locked on ignored-tests.yml and the existing release-probe lines", () => {
      const ignored = readFileSync(path.join(root, ".github", "workflows", "ignored-tests.yml"), "utf8");
      assert.deepEqual(cargoStepsMissingLocked(ignored, "ignored-tests.yml"), []);
      const rust = readFileSync(path.join(root, ".github", "workflows", "rust.yml"), "utf8");
      const verify = readFileSync(path.join(root, "rust", "verify.sh"), "utf8");
      assert.match(rust, /cargo run --release --locked -p devmap-serve --example worker_recovery_probe/);
      assert.match(verify, /cargo run --release --locked -q -p devmap-serve --example worker_recovery_probe/);
      assert.match(verify, /cargo build --release --locked -q -p devmap-soak/);
      assert.match(verify, /cargo run --release --locked -q -p devmap-soak/);
    });

    it("ignores comments, cargo run, cargo fmt, cargo mutants, and echo of a cargo verb", () => {
      const text = [
        "# cargo test --workspace",
        "cargo test --locked # cargo build",
        "cargo run --release",
        "cargo fmt --all -- --check",
        "cargo mutants -p devmap-resolve",
        "echo cargo test --workspace",
        "name: cargo build",
        "echo 'cargo test --workspace'",
      ].join("\n");
      assert.deepEqual(cargoStepsMissingLocked(text, "f"), []);
    });

    it("joins continuations, counts a quoted flag, and rejects a glued or late flag", () => {
      assert.deepEqual(cargoStepsMissingLocked("cargo test --workspace \\\n  --locked\n", "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("cargo test --workspace \\\r\n  --no-fail-fast\r\n", "f"), [
        "f:1: cargo test omits --locked",
      ]);
      assert.deepEqual(cargoStepsMissingLocked('run: "cargo test --locked -p pkg"', "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("cargo test \"--locked\"", "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("cargo test '--locked'", "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("cargo test --locked-extra", "f"), [
        "f:1: cargo test omits --locked",
      ]);
      assert.deepEqual(cargoStepsMissingLocked("cargo test --locked=true", "f"), [
        "f:1: cargo test omits --locked",
      ]);
      assert.deepEqual(cargoStepsMissingLocked("cargo clippy --locked --workspace -- -D warnings", "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("FOO=bar cargo test --locked", "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("cargo test --locked && cargo build", "f"), [
        "f:1: cargo build omits --locked",
      ]);
      assert.deepEqual(cargoStepsMissingLocked("cargo test --locked -p \"名前\"", "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("cargo\ttest\t--locked", "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("cargo\ttest", "f"), ["f:1: cargo test omits --locked"]);
      assert.deepEqual(cargoStepsMissingLocked("command cargo test --locked", "f"), []);
      assert.deepEqual(cargoStepsMissingLocked("exec cargo build", "f"), ["f:1: cargo build omits --locked"]);
      assert.deepEqual(cargoStepsMissingLocked("echo \"$(cargo test --workspace)\"", "f"), [
        "f:1: cargo test omits --locked",
      ]);
      assert.deepEqual(cargoStepsMissingLocked("cargo test --locked `cargo build`", "f"), [
        "f:1: cargo build omits --locked",
      ]);
    });

    it("rejects an empty label, a non-string, and a script over the scan cap", () => {
      assert.throws(() => cargoStepsMissingLocked("", ""), TypeError);
      assert.throws(() => cargoStepsMissingLocked(/** @type {string} */ (null), "f"), TypeError);
      assert.throws(() => cargoStepsMissingLocked("x".repeat(2_000_001), "f"), RangeError);
      assert.deepEqual(cargoStepsMissingLocked("", "f"), []);
    });
  });
});
