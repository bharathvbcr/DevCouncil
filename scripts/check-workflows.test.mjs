import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";
import {
  INSTALL_HINT,
  hasWorkflowLevelPermissions,
  interpret,
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

  it("invokes the archive gate before publishing, including the Windows zip", () => {
    const source = readFileSync(
      fileURLToPath(new URL("../.github/workflows/release.yml", import.meta.url)),
      "utf8",
    );
    const artifactChecks = source.split("\n").filter((line) => line.includes("check-release.mjs --artifacts"));
    assert.ok(
      artifactChecks.length >= 2,
      "release.yml must run the archive gate before cargo-dist publish and again after the flattened download",
    );
  });
});
