import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { classifyLauncher, exitCodeFor, overallVerdict } from "./npm-registry-smoke-checks.mjs";

const MISSING =
  "DevCouncil requires the devcouncil binary on PATH (or ~/.local/bin).\nBuild: go -C backend/go_orchestrator build ...";

describe("classifyLauncher", () => {
  it("passes a launcher that ran the host", () => {
    assert.equal(classifyLauncher("help", { status: 0, stdout: "usage: devcouncil" }).verdict, "PASS");
  });
  it("reports a missing host as NOT RUN, not PASS (the false-green case)", () => {
    const r = classifyLauncher("help", { status: 1, stderr: MISSING });
    assert.equal(r.verdict, "NOT RUN");
    assert.match(r.detail, /host/);
  });
  it("recognises the older wording", () => {
    assert.equal(classifyLauncher("v", { status: 1, stderr: "devcouncil binary not found" }).verdict, "NOT RUN");
  });
  it("fails a non-zero exit that does not name the missing host", () => {
    assert.equal(classifyLauncher("help", { status: 1, stderr: "boom" }).verdict, "FAIL");
    assert.equal(classifyLauncher("help", { status: 1 }).verdict, "FAIL");
  });
  it("fails empty output, a spawn error and a signal death", () => {
    assert.equal(classifyLauncher("help", { status: 0, stdout: "  \n" }).verdict, "FAIL");
    assert.equal(classifyLauncher("help", { status: null, error: new Error("ENOENT") }).verdict, "FAIL");
    assert.equal(classifyLauncher("help", { status: null, stderr: MISSING }).verdict, "FAIL");
  });
  it("does not trust exit 0 alongside a missing-host message", () => {
    assert.equal(classifyLauncher("help", { status: 0, stderr: MISSING }).verdict, "FAIL");
  });
  it("tolerates null stdout and stderr", () => {
    assert.equal(classifyLauncher("help", { status: 2, stdout: null, stderr: null }).verdict, "FAIL");
  });
});

describe("overallVerdict and exitCodeFor", () => {
  const v = (...vs) => vs.map((verdict) => ({ verdict }));
  it("is PASS only when every check passed", () => {
    assert.equal(overallVerdict(v("PASS", "PASS")), "PASS");
    assert.equal(exitCodeFor("PASS"), 0);
  });
  it("is NOT RUN, exiting non-zero, when any check could not run", () => {
    assert.equal(overallVerdict(v("PASS", "NOT RUN")), "NOT RUN");
    assert.equal(overallVerdict(v("NOT RUN", "NOT RUN")), "NOT RUN");
    assert.notEqual(exitCodeFor("NOT RUN"), 0);
    assert.notEqual(exitCodeFor("NOT RUN"), exitCodeFor("FAIL"));
  });
  it("is NOT RUN when nothing ran", () => {
    assert.equal(overallVerdict([]), "NOT RUN");
  });
  it("lets FAIL outrank NOT RUN", () => {
    assert.equal(overallVerdict(v("NOT RUN", "FAIL", "PASS")), "FAIL");
  });
  it("fails closed on an unknown verdict", () => {
    assert.equal(exitCodeFor("MAYBE"), 1);
    assert.equal(overallVerdict(v("PASS", "MAYBE")), "NOT RUN");
  });
});
