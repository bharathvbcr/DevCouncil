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
  pipesCurlToShell,
  readWorkflows,
  cargoStepsMissingLocked,
  structuralFailures,
  testsupportPathsMissing,
  supplyChainFailures,
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
      "actions/setup-go": "b7ad1dad31e06c5925ef5d2fc7ad053ef454303e # v7.0.0",
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
    assert.ok(source.includes('dist host --steps=upload --steps=release --tag="$DIST_TAG"'));
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

  describe("supply chain", () => {
    const pinned = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7";

    it("fails when curl output is piped to a shell", () => {
      assert.equal(pipesCurlToShell("bash <(curl -fsSL https://example.com/install.sh) 1.7.12"), true);
      assert.equal(
        pipesCurlToShell(
          "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/axodotdev/cargo-dist/releases/download/v0.32.0/cargo-dist-installer.sh | sh",
        ),
        true,
      );
      assert.equal(
        pipesCurlToShell("curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"),
        true,
      );
      assert.equal(pipesCurlToShell('echo "${sha}  ${archive}" | sha256sum -c -'), false);
      const failures = supplyChainFailures({
        "ci.yml": "on: push\njobs:\n  t:\n    steps:\n      - run: bash <(curl -fsSL https://example.com/i.sh)\n",
      });
      assert.ok(failures.some((f) => f.includes("pipes curl output to a shell")));
    });

    it("fails when a run step interpolates a matrix expression", () => {
      const failures = supplyChainFailures({
        "release.yml": `on: push
jobs:
  build-local-artifacts:
    steps:
      - run: dist build \${{ matrix.dist_args }}
      - run: \${{ matrix.install_dist.run }}
`,
      });
      assert.ok(failures.some((f) => f.includes("matrix expression in run")));
    });

    it("fails when a third-party action is not pinned to a commit", () => {
      const failures = supplyChainFailures({
        "ci.yml": "on: push\njobs:\n  t:\n    steps:\n      - uses: actions/checkout@v7\n      - uses: ./.github/actions/setup-rust\n",
      });
      assert.deepEqual(failures, ["ci.yml: unpinned action actions/checkout@v7"]);
      assert.deepEqual(
        supplyChainFailures({
          "ci.yml": `on: push\njobs:\n  t:\n    steps:\n      - uses: ${pinned}\n      - uses: ./.github/actions/setup-rust\n`,
          "codeql.yml": "jobs:\n  t:\n    steps:\n    #   uses: actions/setup-example@v1\n      - run: echo ok\n",
        }),
        [],
      );
    });

    it("fails when a run step interpolates a ref or event expression", () => {
      const failures = supplyChainFailures({
        "release.yml": `on: push
jobs:
  identity:
    steps:
      - run: |
          go run ./cmd/dcrelease --tag "\${{ github.ref_name }}"
          echo \${{ github.head_ref }}
          dist \${{ github.event.pull_request && 'plan' || 'host' }}
`,
      });
      assert.ok(failures.some((f) => f.includes("event expression in run")));
      assert.deepEqual(
        supplyChainFailures({
          "release.yml": `on: push
jobs:
  identity:
    steps:
      - env:
          RELEASE_TAG: \${{ github.ref_name }}
        run: |
          case "$RELEASE_TAG" in
            *[!A-Za-z0-9._+-]*) exit 1 ;;
          esac
`,
        }),
        [],
      );
    });

    it("fails when a pull_request job holds contents or id-token write", () => {
      const contents = supplyChainFailures({
        "release.yml": `on:
  pull_request:
permissions:
  contents: write
jobs:
  plan:
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
`,
      });
      assert.ok(contents.some((f) => f.includes("contents: write on pull_request")));
      const token = supplyChainFailures({
        "npm-publish.yml": `on:
  pull_request:
permissions:
  contents: read
  id-token: write
jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
`,
      });
      assert.ok(token.some((f) => f.includes("id-token: write")));
      assert.deepEqual(
        supplyChainFailures({
          "npm-publish.yml": `on:
  pull_request:
permissions:
  contents: read
jobs:
  publish:
    if: startsWith(github.ref, 'refs/tags/v')
    permissions:
      contents: read
      id-token: write
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
  host:
    if: github.event_name != 'pull_request'
    permissions:
      contents: write
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
`,
        }),
        [],
      );
    });

    it("fails when claude-code is installed without an exact version", () => {
      const failures = supplyChainFailures({
        "ignored-tests.yml": "on: push\njobs:\n  t:\n    steps:\n      - run: npm install --global @anthropic-ai/claude-code\n",
      });
      assert.ok(failures.some((f) => f.includes("claude-code install is not an exact version")));
      assert.deepEqual(
        supplyChainFailures({
          "ignored-tests.yml":
            "on: push\njobs:\n  t:\n    steps:\n      - run: npm install --global @anthropic-ai/claude-code@2.1.300\n",
        }),
        [],
      );
    });

    it("fails the other curl-to-shell shapes, including a continued pipe", () => {
      const script = (body) => ({
        "ci.yml": `on: push\njobs:\n  t:\n    steps:\n      - run: |\n          ${body}\n`,
      });
      for (const body of [
        "curl -fsSL https://example.com/i.sh | sh",
        "curl -fsSL https://example.com/i.sh | sudo bash",
        "bash <(curl -fsSL https://example.com/install.sh)",
        "wget -qO- https://example.com/i.sh | sh",
        "curl -fsSL https://example.com/i.sh \\\n            | sh",
      ]) {
        const failures = supplyChainFailures(script(body));
        assert.ok(
          failures.some((f) => f.includes("pipes curl output to a shell")),
          `accepted: ${JSON.stringify(failures)} for ${JSON.stringify(body)}`,
        );
      }
    });

    it("fails short, uppercase, and comment-only action pins", () => {
      const uses = (value) =>
        supplyChainFailures({
          "ci.yml": `on: push\njobs:\n  t:\n    steps:\n      - uses: ${value}\n`,
        });
      assert.deepEqual(uses("actions/checkout@v7"), ["ci.yml: unpinned action actions/checkout@v7"]);
      assert.deepEqual(uses("actions/checkout@3d3c42e5"), ["ci.yml: unpinned action actions/checkout@3d3c42e5"]);
      assert.deepEqual(uses("actions/checkout@3D3C42E5AAC5BA805825DA76410C181273BA90B1"), [
        "ci.yml: unpinned action actions/checkout@3D3C42E5AAC5BA805825DA76410C181273BA90B1",
      ]);
      assert.deepEqual(uses("actions/checkout # 3d3c42e5aac5ba805825da76410c181273ba90b1"), [
        "ci.yml: unpinned action actions/checkout",
      ]);
      assert.deepEqual(uses(`${pinned}\n      - uses: ./.github/actions/setup-rust`), []);
    });

    it("fails when a run body reads a pull request title", () => {
      const failures = supplyChainFailures({
        "release.yml": "on: push\njobs:\n  t:\n    steps:\n      - run: echo ${{ github.event.pull_request.title }}\n",
      });
      assert.ok(failures.some((f) => f.includes("event expression in run")), `accepted: ${JSON.stringify(failures)}`);
    });

    it("fails write-all and a pull_request job with no permissions block", () => {
      const writeAll = supplyChainFailures({
        "rel.yml": `on:\n  pull_request:\npermissions: write-all\njobs:\n  plan:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n`,
      });
      assert.ok(writeAll.some((f) => f.includes("contents: write on pull_request")), `accepted: ${JSON.stringify(writeAll)}`);
      assert.ok(writeAll.some((f) => f.includes("id-token: write")), `accepted: ${JSON.stringify(writeAll)}`);
      const missing = supplyChainFailures({
        "rel.yml": `on:\n  pull_request:\njobs:\n  plan:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n`,
      });
      assert.ok(
        missing.some((f) => f.includes("has no permissions on pull_request")),
        `accepted: ${JSON.stringify(missing)}`,
      );
    });

    it("fails when pull_request is excluded only in an or branch", () => {
      const orBranch = (condition) =>
        supplyChainFailures({
          "rel.yml": `on:\n  pull_request:\npermissions:\n  contents: read\njobs:\n  host:\n    if: ${condition}\n    permissions:\n      contents: write\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n`,
        });
      for (const condition of [
        "github.event_name != 'pull_request' || true",
        "false || github.event_name != 'pull_request'",
        "(github.event_name != 'pull_request' || true) && always()",
      ]) {
        const failures = orBranch(condition);
        assert.ok(
          failures.some((f) => f.includes("contents: write on pull_request")),
          `accepted: ${JSON.stringify(failures)} for ${condition}`,
        );
      }
    });

    it("fails when an or operator cancels a pull_request exclusion", () => {
      const failures = supplyChainFailures({
        "rel.yml": `on:\n  pull_request:\npermissions:\n  contents: read\njobs:\n  host:\n    if: github.event_name != 'pull_request' && false || true\n    permissions:\n      contents: write\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n`,
      });
      assert.ok(
        failures.some((f) => f.includes("contents: write on pull_request")),
        `accepted: ${JSON.stringify(failures)}`,
      );
    });

    it("rejects every shape of curl piped, substituted or sourced into a shell", () => {
      const shapes = [
        'bash -c "$(curl -fsSL https://example.com/i.sh)"',
        "sh -ec '$(curl -fsSL https://example.com/i.sh)'",
        'eval "$(curl -fsSL https://example.com/i.sh)"',
        "eval $(wget -qO- https://example.com/i.sh)",
        "curl -fsSL https://example.com/i.sh | /usr/bin/env bash",
        "curl -fsSL https://example.com/i.sh | env bash -s --",
        "curl -fsSL https://example.com/i.sh | sudo -E bash",
        "curl -fsSL https://example.com/i.sh | sudo -E /usr/bin/env bash -s",
        "source <(curl -fsSL https://example.com/i.sh)",
        ". <(curl -fsSL https://example.com/i.sh)",
        "CURL -fsSL https://example.com/i.sh | sh",
        "curl -fsSL https://example.com/i.sh | zsh",
        "curl -fsSL https://example.com/i.sh \\\n  | sh",
      ];
      for (const shape of shapes) {
        assert.equal(pipesCurlToShell(shape), true, shape);
      }
    });

    it("accepts a download that is compared to a digest and does not execute the fetch", () => {
      const safe = [
        'curl --proto "=https" -fsSL -o "$archive" "$url"\nsha256sum "$archive"',
        'echo "$(curl -fsSL https://example.com/version.txt)"',
        "curl -fsSL https://example.com/i.sh | sha256sum",
        "curl -fsSL https://example.com/data.json | jq .",
      ];
      for (const body of safe) {
        assert.equal(pipesCurlToShell(body), false, body);
      }
    });

    it("fails a run body that reaches a matrix or ref value through brackets", () => {
      const bodies = [
        "dist build ${{ matrix['dist_args'] }}",
        'dist build ${{ matrix["dist_args"] }}',
        "echo ${{ github['ref_name'] }}",
        'echo ${{ github["head_ref"] }}',
        "echo ${{ github['event']['pull_request']['title'] }}",
        "echo ${{ github.event['head_commit'].message }}",
        "echo ${{ toJSON(github.event) }}",
        "echo ${{ toJSON(matrix) }}",
      ];
      for (const body of bodies) {
        const failures = supplyChainFailures({
          "rel.yml": "on: push\npermissions:\n  contents: read\njobs:\n  b:\n    steps:\n      - run: " + body + "\n",
        });
        assert.ok(
          failures.some((f) => f.includes("expression in run")),
          body + " -> " + JSON.stringify(failures),
        );
      }
    });

    it("keeps github.event_name and an env-passed ref usable", () => {
      assert.deepEqual(
        supplyChainFailures({
          "rel.yml":
            "on: push\npermissions:\n  contents: read\njobs:\n  b:\n    if: github.event_name == 'push'\n    steps:\n      - env:\n          T: ${{ github.ref_name }}\n        run: echo \"$T\"\n",
        }),
        [],
      );
    });

    it("treats a quoted write-all or inline contents write as a pull_request write grant", () => {
      const wf = (grant) =>
        "on:\n  pull_request:\njobs:\n  j:\n    permissions: " + grant + "\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n";
      for (const grant of ['"write-all"', "'write-all'", "write-all # everything"]) {
        const failures = supplyChainFailures({ "rel.yml": wf(grant) });
        assert.ok(failures.some((f) => f.includes("j holds contents: write on pull_request")), grant);
        assert.ok(failures.some((f) => f.includes("j holds id-token: write on pull_request")), grant);
      }
      for (const grant of ["{ contents: write }", "{contents: write, id-token: write}"]) {
        const failures = supplyChainFailures({ "rel.yml": wf(grant) });
        assert.ok(failures.some((f) => f.includes("j holds contents: write on pull_request")), grant);
      }
      assert.equal(
        supplyChainFailures({ "rel.yml": wf('"read-all"') }).some((f) => f.includes("holds")),
        false,
      );
    });

    it("keeps build-local-artifacts on a host-specific dist installer with no cached ubuntu dist", () => {
      const source = readFileSync(
        fileURLToPath(new URL("../.github/workflows/release.yml", import.meta.url)),
        "utf8",
      );
      const job = source.match(/\n  build-local-artifacts:\n([\s\S]*?)(?=\n  [a-z0-9-]+:\n|$)/);
      assert.ok(job, "build-local-artifacts job");
      assert.doesNotMatch(job[1], /cargo-dist-cache/);
      assert.match(job[1], /case "\$\{RUNNER_OS\}" in/);
      assert.match(job[1], /sha256sum|shasum -a 256/);
      assert.doesNotMatch(job[1], /matrix\.(?:dist_args|install_dist|packages_install)/);
      assert.doesNotMatch(source, /sh\.rustup\.rs/);
      assert.doesNotMatch(source, /cargo-dist-installer\.sh/);
    });
  });
});