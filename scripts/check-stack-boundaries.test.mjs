import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { defaultRoots, loadMatrix, routePath, runChecks } from "./check-stack-boundaries.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function makeFixtureTree() {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), "stack-boundaries-"));
  const dc = path.join(base, "DevCouncil");
  fs.cpSync(
    path.join(REPO_ROOT, "specs", "003-stack-boundaries-and-non-overlap", "contracts", "ownership-matrix.json"),
    path.join(dc, "specs", "003-stack-boundaries-and-non-overlap", "contracts", "ownership-matrix.json"),
    { recursive: true },
  );
  // Copy matrix and populate all DevCouncil seams minimally
  const matrix = loadMatrix(dc);
  for (const cap of matrix.capabilities) {
    for (const s of [...(cap.owner_seams || []), ...(cap.adapters || []).flatMap((a) => a.seams || [])]) {
      if (!s.startsWith("DevCouncil:")) continue;
      let rel = s.slice("DevCouncil:".length).replace(/\s+\([^)]*\)\s*$/, "").trim();
      rel = rel.replace(/:\d+(?:[-–]\d+)?$/, "").trim();
      // expand simple braces
      const expanded = rel.match(/^(.*)\{([^{}]+)\}(.*)$/)
        ? rel.match(/^(.*)\{([^{}]+)\}(.*)$/)[2].split(",").map((p) => `${rel.match(/^(.*)\{([^{}]+)\}(.*)$/)[1]}${p.trim()}${rel.match(/^(.*)\{([^{}]+)\}(.*)$/)[3]}`)
        : [rel];
      for (const item of expanded) {
        const full = path.join(dc, item);
        if (item.endsWith("/")) {
          fs.mkdirSync(full, { recursive: true });
        } else {
          fs.mkdirSync(path.dirname(full), { recursive: true });
          fs.writeFileSync(full, "");
        }
      }
    }
  }
  const regPath = path.join(dc, "backend", "go_orchestrator", "devcouncil", "registry.go");
  fs.mkdirSync(path.dirname(regPath), { recursive: true });
  fs.writeFileSync(regPath, 'package devcouncil\nvar x = []string{Name: "devcouncil_checkout_task"}\n');
  fs.mkdirSync(path.join(dc, "rust", "devmap-cli", "skills", "devmap"), { recursive: true });
  fs.mkdirSync(path.join(dc, "backend", "go_orchestrator", "devcouncil", "skills", "library"), { recursive: true });
  return {
    base,
    roots: {
      devcouncil: dc,
      manvi: path.join(base, "Manvi-absent"),
      waylode: path.join(base, "Waylode-absent"),
      labs: path.join(base, "Labs-absent"),
      relay: path.join(base, "Relay-absent"),
    },
  };
}

test("repo matrix passes boundary validation cleanly", () => {
  const report = runChecks(defaultRoots(REPO_ROOT));
  assert.equal(report.ok, true, `unexpected violations: ${JSON.stringify(report.violations)}`);
  assert.equal(report.capability_count, 12);
  assert.equal(report.lane_count, 4);
  assert.equal(report.seams_summary.missing, 0);
  assert.ok(!report.tracked_open_overlaps.some((o) => o.id === "OV-VERIFY-01"));
  assert.ok(!report.tracked_open_overlaps.some((o) => o.id === "OV-TOOLS-01"));
  assert.ok(report.tracked_open_overlaps.some((o) => o.id === "OV-INTEGRATE-01"));
  assert.ok(report.tracked_open_overlaps.some((o) => o.id === "OV-SKILLS-01"));
});

test("detects un-delegated shared tool overlap in Manvi when OV-TOOLS-01 is closed", () => {
  const { base, roots } = makeFixtureTree();
  try {
    roots.manvi = path.join(base, "Manvi");
    const matrix = loadMatrix(roots.devcouncil);
    for (const cap of matrix.capabilities) {
      for (const s of [...(cap.owner_seams || []), ...(cap.adapters || []).flatMap((a) => a.seams || [])]) {
        if (!s.startsWith("Manvi:")) continue;
        let rel = s.slice("Manvi:".length).replace(/\s+\([^)]*\)\s*$/, "").trim();
        const full = path.join(roots.manvi, rel);
        if (rel.endsWith("/")) {
          fs.mkdirSync(full, { recursive: true });
        } else {
          fs.mkdirSync(path.dirname(full), { recursive: true });
          fs.writeFileSync(full, "");
        }
      }
    }
    const mvTools = path.join(roots.manvi, "manvi", "devcouncil", "tools.go");
    fs.mkdirSync(path.dirname(mvTools), { recursive: true });
    fs.writeFileSync(mvTools, 'package devcouncil\nvar x = "devcouncil_checkout_task"\n');

    const failReport = runChecks(roots);
    assert.equal(failReport.ok, false);
    assert.ok(failReport.violations.some((v) => v.rule === "unregistered_shared_tool_overlap"));

    fs.writeFileSync(
      mvTools,
      'package devcouncil\nimport "github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc"\nvar _, _ = dc.SharedToolSpecByName("devcouncil_checkout_task")\n',
    );
    const okReport = runChecks(roots);
    assert.equal(okReport.ok, true, JSON.stringify(okReport.violations));
    assert.equal(okReport.shared_tools.manvi_delegates_to_dc, true);
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
  }
});

test("absent sibling repos report absent cleanly without false pass or fail", () => {
  const { base, roots } = makeFixtureTree();
  try {
    const report = runChecks(roots);
    assert.equal(report.ok, true, JSON.stringify(report.violations));
    assert.ok(report.seams_summary.absent > 0);
    assert.equal(report.forbidden_guards.Manvi, "absent");
    assert.equal(report.forbidden_guards.Waylode, "absent");
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
  }
});

test("detects Waylode vendored devmap-serve, WayLode casing, and skill duplication", () => {
  const { base, roots } = makeFixtureTree();
  try {
    roots.waylode = path.join(base, "Waylode");
    fs.mkdirSync(path.join(roots.waylode, "src-tauri", "vendored", "devmap-serve"), { recursive: true });
    fs.mkdirSync(path.join(roots.waylode, "plugins", "waylode", "skills", "devmap"), { recursive: true });
    fs.writeFileSync(path.join(roots.waylode, "README.md"), "# WayLode\n");

    const report = runChecks(roots);
    assert.equal(report.ok, false);
    const rules = new Set(report.violations.map((v) => v.rule));
    assert.ok(rules.has("waylode_vendored_devmap_serve"));
    assert.ok(rules.has("cross_repo_skill_duplication"));
    assert.ok(rules.has("forbidden_brand_casing"));
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
  }
});

test("detects private Labs doc link in tracked public docs", () => {
  const { base, roots } = makeFixtureTree();
  try {
    fs.mkdirSync(path.join(roots.devcouncil, "docs"), { recursive: true });
    fs.writeFileSync(path.join(roots.devcouncil, "docs", "TEST.md"), "See [labs](LABS.md)\n");
    const report = runChecks(roots);
    assert.equal(report.ok, false);
    assert.ok(report.violations.some((v) => v.rule === "private_labs_doc_link"));
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
  }
});

test("--route assigns paths across all 4 lanes deterministically", () => {
  const roots = defaultRoots(REPO_ROOT);
  const a = routePath("DevCouncil:rust/devmap-serve/src/mcp.rs", roots);
  assert.equal(a.ok, true);
  assert.equal(a.owning_lane, "lane-a-devcouncil");

  const b = routePath("Manvi:manvi/serve/verify.go", roots);
  assert.equal(b.ok, true);
  assert.equal(b.owning_lane, "lane-b-manvi");

  const c = routePath("GitPulse:src-tauri/src/engine/worktree.rs", roots);
  assert.equal(c.ok, true);
  assert.equal(c.owning_lane, "lane-c-waylode-primary");

  const d = routePath("Waylode Labs:src-tauri/src/features/registry.rs", roots);
  assert.equal(d.ok, true);
  assert.equal(d.forbidden, false);
  assert.equal(d.owning_lane, "lane-d-waylode-labs");
});

test("--route routes Waylode:<labs-path> to lane-d-waylode-labs", () => {
  const roots = defaultRoots(REPO_ROOT);
  for (const target of [
    "Waylode:src-tauri/src/features/registry.rs",
    "Waylode:src-tauri/src/labs/sync.rs",
    "Waylode:src/lib/account/billing.ts",
    "GitPulse:src-tauri/src/features/registry.rs",
  ]) {
    const routed = routePath(target, roots);
    assert.equal(routed.ok, true, `${target} should route cleanly: ${JSON.stringify(routed)}`);
    assert.equal(routed.forbidden, false);
    assert.equal(routed.owning_lane, "lane-d-waylode-labs", `${target} routed to ${routed.owning_lane}`);
    assert.equal(routed.edition, "labs");
  }
});

test("--route flags forbidden_paths with forbidden: true, owning_lane, and exit code 1", () => {
  const roots = defaultRoots(REPO_ROOT);
  const spec = "Waylode:src-tauri/vendored/devmap-serve/src/lib.rs";
  const routed = routePath(spec, roots);
  assert.equal(routed.ok, false);
  assert.equal(routed.forbidden, true);
  assert.equal(routed.owning_lane, "lane-c-waylode-primary");
  assert.equal(routed.matched_forbidden_path, "src-tauri/vendored/devmap-serve/");
  assert.match(routed.error, /lane-c-waylode-primary/);

  const scriptPath = path.join(REPO_ROOT, "scripts", "check-stack-boundaries.mjs");
  const jsonRun = spawnSync(
    process.execPath,
    [scriptPath, "--route", spec, "--json"],
    { encoding: "utf8" },
  );
  assert.equal(jsonRun.status, 1, `stderr=${jsonRun.stderr} stdout=${jsonRun.stdout}`);
  const parsed = JSON.parse(jsonRun.stdout);
  assert.equal(parsed.ok, false);
  assert.equal(parsed.forbidden, true);
  assert.equal(parsed.owning_lane, "lane-c-waylode-primary");

  const textRun = spawnSync(
    process.execPath,
    [scriptPath, "--route", spec],
    { encoding: "utf8" },
  );
  assert.equal(textRun.status, 1);
  assert.match(textRun.stderr, /lane-c-waylode-primary/);
});
