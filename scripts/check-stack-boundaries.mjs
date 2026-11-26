#!/usr/bin/env node
/**
 * Stack boundary, work-split, and anti-overlap gate for DevCouncil, Manvi, and Waylode.
 *
 * Modes:
 *   node scripts/check-stack-boundaries.mjs [--json]
 *   node scripts/check-stack-boundaries.mjs --lanes [--json]
 *   node scripts/check-stack-boundaries.mjs --route <repo>:<path> [--json]
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const DEFAULT_DEVCOUNCIL_ROOT = path.resolve(__dirname, "..");

const ALLOWED_OWNERS = new Set(["DevCouncil", "Manvi", "Waylode", "Waylode Labs"]);

export function defaultRoots(devcouncilRoot = DEFAULT_DEVCOUNCIL_ROOT) {
  const parent = path.dirname(devcouncilRoot);
  const waylode = path.join(parent, "GitPulse");
  return {
    devcouncil: devcouncilRoot,
    manvi: path.join(parent, "Manvi"),
    waylode,
    labs: path.join(waylode, ".gitpulse", "worktrees", "waylode-labs"),
    relay: path.join(parent, "gitpulse-relay"),
  };
}

export function normalizeRepoName(name) {
  const clean = String(name || "").trim();
  const lower = clean.toLowerCase();
  if (lower === "devcouncil") return "DevCouncil";
  if (lower === "manvi") return "Manvi";
  if (lower === "waylode" || lower === "gitpulse" || lower === "waylode primary") return "Waylode";
  if (lower === "waylode labs" || lower === "waylode-labs" || lower === "gitpulse-labs") {
    return "Waylode Labs";
  }
  return clean;
}

export function expandBraces(input) {
  const match = input.match(/^(.*)\{([^{}]+)\}(.*)$/);
  if (!match) return [input];
  const [, prefix, inner, suffix] = match;
  const out = [];
  for (const part of inner.split(",")) {
    for (const expanded of expandBraces(`${prefix}${part.trim()}${suffix}`)) {
      out.push(expanded);
    }
  }
  return out;
}

export function parseSeamSpec(rawSeam) {
  const colonIdx = rawSeam.indexOf(":");
  if (colonIdx <= 0) return [];
  const repo = normalizeRepoName(rawSeam.slice(0, colonIdx));
  let rest = rawSeam.slice(colonIdx + 1).trim();
  // Strip trailing inline descriptions such as " (21 tools)"
  rest = rest.replace(/\s+\([^)]*\)\s*$/, "").trim();
  // Strip optional line range suffix such as ":22-64,131-181"
  rest = rest.replace(/:\d+(?:[-–]\d+)?(?:,\d+(?:[-–]\d+)?)*$/, "").trim();
  return expandBraces(rest).map((relPath) => ({ repo, relPath, raw: rawSeam }));
}

function repoRootFor(repo, roots) {
  switch (repo) {
    case "DevCouncil":
      return roots.devcouncil;
    case "Manvi":
      return roots.manvi;
    case "Waylode":
      return roots.waylode;
    case "Waylode Labs":
      return roots.labs;
    default:
      return null;
  }
}

function walkFiles(dir, { ignoreDirs = new Set(), ext = null, ignoreTests = false } = {}) {
  const out = [];
  if (!fs.existsSync(dir)) return out;
  const stack = [dir];
  while (stack.length > 0) {
    const current = stack.pop();
    let entries;
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) {
        if (ignoreDirs.has(entry.name)) continue;
        stack.push(full);
      } else if (entry.isFile()) {
        if (ext && !entry.name.endsWith(ext)) continue;
        if (ignoreTests) {
          const lower = entry.name.toLowerCase();
          if (
            lower.endsWith("_test.go") ||
            lower.endsWith("_tests.rs") ||
            lower.includes("test") ||
            full.includes(`${path.sep}tests${path.sep}`)
          ) {
            continue;
          }
        }
        out.push(full);
      }
    }
  }
  return out.sort();
}

function collectMatrixOverlaps(matrix) {
  const overlaps = [];
  for (const cap of matrix.capabilities || []) {
    for (const ov of cap.current_overlaps || []) {
      overlaps.push({
        capability: cap.id,
        id: ov.id,
        repo: ov.repo,
        status: ov.status,
        closure_task: ov.closure_task,
        seam: ov.seam,
      });
    }
  }
  return overlaps;
}

function checkOwnershipAndLanes(matrix, violations) {
  const seenCaps = new Set();
  for (const cap of matrix.capabilities || []) {
    if (!cap.id) {
      violations.push({ rule: "capability_missing_id", detail: "Capability entry lacks id" });
      continue;
    }
    if (seenCaps.has(cap.id)) {
      violations.push({ rule: "duplicate_capability_id", detail: `Duplicate capability id: ${cap.id}` });
    }
    seenCaps.add(cap.id);
    if (!ALLOWED_OWNERS.has(cap.canonical_owner)) {
      violations.push({
        rule: "invalid_canonical_owner",
        detail: `${cap.id} has invalid canonical_owner '${cap.canonical_owner}'`,
      });
    }
  }

  const seenLanes = new Set();
  const seenOrders = new Set();
  const byRepoEdition = new Map();
  for (const lane of matrix.work_lanes || []) {
    if (!lane.lane_id || seenLanes.has(lane.lane_id)) {
      violations.push({ rule: "invalid_lane_id", detail: `Duplicate or missing lane_id '${lane.lane_id}'` });
    }
    seenLanes.add(lane.lane_id);
    if (typeof lane.landing_order === "number") {
      if (seenOrders.has(lane.landing_order)) {
        violations.push({
          rule: "duplicate_landing_order",
          detail: `Duplicate landing_order ${lane.landing_order} on ${lane.lane_id}`,
        });
      }
      seenOrders.add(lane.landing_order);
    }

    const key = `${lane.repo}::${lane.edition}`;
    const list = byRepoEdition.get(key) || [];
    for (const rawOwned of lane.owned_paths || []) {
      const owned = rawOwned.replace(/\/+$/, "");
      for (const prev of list) {
        if (owned === prev || owned.startsWith(`${prev}/`) || prev.startsWith(`${owned}/`)) {
          violations.push({
            rule: "overlapping_lane_owned_paths",
            detail: `Overlapping owned_paths in ${key}: '${owned}' vs '${prev}'`,
          });
        }
      }
      list.push(owned);
    }
    byRepoEdition.set(key, list);
  }
}

function checkSeams(matrix, roots, violations) {
  const seamResults = [];
  const refs = [];
  for (const cap of matrix.capabilities || []) {
    for (const s of cap.owner_seams || []) refs.push({ capability: cap.id, role: "owner", raw: s });
    for (const adapter of cap.adapters || []) {
      for (const s of adapter.seams || []) {
        refs.push({ capability: cap.id, role: adapter.role || "adapter", raw: s });
      }
    }
  }

  for (const ref of refs) {
    const parsed = parseSeamSpec(ref.raw);
    for (const entry of parsed) {
      let targetRoot = repoRootFor(entry.repo, roots);
      let rel = entry.relPath;
      if (entry.repo === "Waylode Labs" && rel === "gitpulse-relay") {
        targetRoot = roots.relay;
        rel = ".";
      }
      if (!targetRoot || !fs.existsSync(targetRoot)) {
        seamResults.push({
          capability: ref.capability,
          repo: entry.repo,
          path: entry.relPath,
          status: "absent",
        });
        continue;
      }
      const fullPath = rel === "." ? targetRoot : path.join(targetRoot, rel);
      if (fs.existsSync(fullPath)) {
        seamResults.push({
          capability: ref.capability,
          repo: entry.repo,
          path: entry.relPath,
          status: "ok",
        });
      } else {
        seamResults.push({
          capability: ref.capability,
          repo: entry.repo,
          path: entry.relPath,
          status: "missing",
        });
        violations.push({
          rule: "missing_seam_path",
          detail: `${entry.repo}:${entry.relPath} (cited in ${ref.capability}) does not exist at ${fullPath}`,
        });
      }
    }
  }
  return seamResults;
}

function checkPublicDocHygiene(repoLabel, repoRoot, violations) {
  if (!repoRoot || !fs.existsSync(repoRoot)) return { repo: repoLabel, status: "absent", scanned: 0 };
  const skipFiles = new Set([
    "CHANGELOG.md",
    "markdown-kitchen-sink.md",
    "LABS.md",
    "SUBSCRIPTION_IMPLEMENTATION_PLAN.md",
    "AWS_LABS_TEST_PLAN.md",
  ]);
  const ignoreDirs = new Set([".git", "node_modules", "target", "vendor", "vendored", "archive", "releases", "results", ".devmap", ".devcouncil", ".gitpulse"]);
  const targets = [
    path.join(repoRoot, "README.md"),
    path.join(repoRoot, "constitution.md"),
    ...walkFiles(path.join(repoRoot, "docs"), { ignoreDirs, ext: ".md" }),
    ...walkFiles(path.join(repoRoot, "specs"), { ignoreDirs, ext: ".md" }),
  ].filter((p, idx, arr) => fs.existsSync(p) && arr.indexOf(p) === idx && !skipFiles.has(path.basename(p)));

  for (const file of targets) {
    const text = fs.readFileSync(file, "utf8");
    const rel = path.relative(repoRoot, file);
    if (/\bWayLode\b/.test(text)) {
      violations.push({ rule: "forbidden_brand_casing", detail: `${repoLabel}:${rel} contains 'WayLode'` });
    }
    if (/\]\([^)]*(?:LABS\.md|SUBSCRIPTION_IMPLEMENTATION_PLAN\.md|AWS_LABS_TEST_PLAN\.md)/i.test(text)) {
      violations.push({
        rule: "private_labs_doc_link",
        detail: `${repoLabel}:${rel} links to private Labs documentation`,
      });
    }
  }
  return { repo: repoLabel, status: "checked", scanned: targets.length };
}

function checkSkillsBoundary(roots, trackedOverlaps, violations) {
  const dcDevmapSkillsDir = path.join(roots.devcouncil, "rust", "devmap-cli", "skills");
  const dcLibSkillsDir = path.join(roots.devcouncil, "backend", "go_orchestrator", "devcouncil", "skills", "library");
  const waylodeSkillsDir = path.join(roots.waylode, "plugins", "waylode", "skills");

  const dcSkills = new Set();
  if (fs.existsSync(dcDevmapSkillsDir)) {
    for (const d of fs.readdirSync(dcDevmapSkillsDir, { withFileTypes: true })) {
      if (d.isDirectory()) dcSkills.add(d.name);
    }
  }
  if (fs.existsSync(dcLibSkillsDir)) {
    for (const f of fs.readdirSync(dcLibSkillsDir, { withFileTypes: true })) {
      if (f.isFile() && f.name.endsWith(".md")) dcSkills.add(f.name.replace(/\.md$/, ""));
    }
  }

  if (!fs.existsSync(roots.waylode)) {
    return { status: "absent", devcouncil_skills: dcSkills.size, waylode_plugin_skills: 0, overlapping_plugin_skills: [] };
  }

  const wlSkills = [];
  if (fs.existsSync(waylodeSkillsDir)) {
    for (const d of fs.readdirSync(waylodeSkillsDir, { withFileTypes: true })) {
      if (d.isDirectory()) wlSkills.push(d.name);
    }
  }
  const overlap = wlSkills.filter((name) => dcSkills.has(name));
  if (overlap.length > 0) {
    violations.push({
      rule: "cross_repo_skill_duplication",
      detail: `Waylode plugins/waylode/skills duplicates DevCouncil skill(s): ${overlap.join(", ")}`,
    });
  }
  const hasSkillsOverlapRecord = trackedOverlaps.some((o) => o.id === "OV-SKILLS-01");
  return {
    status: "checked",
    devcouncil_skills: dcSkills.size,
    waylode_plugin_skills: wlSkills.length,
    overlapping_plugin_skills: overlap,
    tracked_overlap_registered: hasSkillsOverlapRecord,
  };
}

function checkSharedDevCouncilTools(roots, trackedOverlaps, violations) {
  const dcToolspecs = path.join(roots.devcouncil, "backend", "go_orchestrator", "dc", "toolspecs.go");
  const dcRegistry = path.join(roots.devcouncil, "backend", "go_orchestrator", "devcouncil", "registry.go");
  const manviTools = path.join(roots.manvi, "manvi", "devcouncil", "tools.go");
  if (!fs.existsSync(dcRegistry) && !fs.existsSync(dcToolspecs)) {
    violations.push({ rule: "missing_registry", detail: `Missing DevCouncil registry at ${dcRegistry}` });
    return { status: "missing" };
  }
  let dcNames = [];
  if (fs.existsSync(dcToolspecs)) {
    const specSrc = fs.readFileSync(dcToolspecs, "utf8");
    dcNames = [...specSrc.matchAll(/Name:\s*"(devcouncil_[a-z0-9_]+)"/g)].map((m) => m[1]);
  }
  if (dcNames.length === 0 && fs.existsSync(dcRegistry)) {
    const dcSrc = fs.readFileSync(dcRegistry, "utf8");
    dcNames = [...dcSrc.matchAll(/Name:\s*"(devcouncil_[a-z0-9_]+)"/g)].map((m) => m[1]);
  }
  if (!fs.existsSync(manviTools)) {
    return { status: "absent", devcouncil_shared_tools: dcNames };
  }
  const mvSrc = fs.readFileSync(manviTools, "utf8");
  const mvShared = dcNames.filter((name) => mvSrc.includes(`"${name}"`));
  const mvDelegatesToDc = /\bdc\.SharedTool(?:SpecByName|Specs|Names)\b/.test(mvSrc);
  const ovToolsRegistered = trackedOverlaps.some((o) => o.id === "OV-TOOLS-01" && (o.status === "open" || o.status === "partial"));
  if (mvShared.length > 0 && !mvDelegatesToDc && !ovToolsRegistered) {
    violations.push({
      rule: "unregistered_shared_tool_overlap",
      detail: `Manvi manvi/devcouncil/tools.go re-declares shared tools (${mvShared.join(", ")}) without delegating to dc.SharedToolSpec* or active OV-TOOLS-01 registration`,
    });
  }
  return {
    status: "checked",
    devcouncil_shared_tools: dcNames,
    manvi_redeclared_tools: mvShared,
    manvi_delegates_to_dc: mvDelegatesToDc,
    tracked_overlap_id: "OV-TOOLS-01",
    tracked_overlap_registered: ovToolsRegistered,
  };
}

function checkForbiddenEngineAndBillingRules(roots, violations) {
  const ignoreDirs = new Set([".git", "target", "vendor", "vendored", "node_modules"]);
  const status = {};

  // 1. DevCouncil zero Stripe / billing / paywall check in non-test product code
  const dcFiles = [
    ...walkFiles(path.join(roots.devcouncil, "rust"), { ignoreDirs, ext: ".rs", ignoreTests: true }),
    ...walkFiles(path.join(roots.devcouncil, "backend", "go_orchestrator"), { ignoreDirs, ext: ".go", ignoreTests: true }),
  ];
  for (const f of dcFiles) {
    const rel = path.relative(roots.devcouncil, f);
    // dc-verify and dc-redact legitimately contain regex detectors for Stripe secret keys
    if (rel.startsWith("rust/dc-verify/") || rel.startsWith("rust/dc-redact/") || rel.startsWith("backend/go_orchestrator/devcouncil/dc/redact/")) {
      continue;
    }
    const text = fs.readFileSync(f, "utf8");
    if (/\b(stripe_secret|stripe::|paywalled|subscription_tier|requires_sign_in)\b/i.test(text)) {
      violations.push({
        rule: "devcouncil_forbidden_billing_or_auth_gate",
        detail: `DevCouncil:${rel} contains forbidden billing/subscription/sign-in identifier`,
      });
    }
  }
  status.DevCouncil = "checked";

  // 2. Waylode forbidden checks
  if (!fs.existsSync(roots.waylode)) {
    status.Waylode = "absent";
  } else {
    if (fs.existsSync(path.join(roots.waylode, "src-tauri", "vendored", "devmap-serve"))) {
      violations.push({
        rule: "waylode_vendored_devmap_serve",
        detail: "Waylode must not vendor devmap-serve (src-tauri/vendored/devmap-serve exists)",
      });
    }
    const wlFiles = walkFiles(path.join(roots.waylode, "src-tauri", "src"), { ignoreDirs, ext: ".rs", ignoreTests: true });
    for (const f of wlFiles) {
      const rel = path.relative(roots.waylode, f);
      const text = fs.readFileSync(f, "utf8");
      if (/(?:fs::write|OpenOptions|write_all)[^\n]*base_store_ref\.json/.test(text)) {
        violations.push({
          rule: "waylode_base_store_ref_writer",
          detail: `Waylode:${rel} must not write legacy base_store_ref.json`,
        });
      }
      if (/CREATE\s+TABLE(?:\s+IF\s+NOT\s+EXISTS)?\s+(?:task_leases|work_items|task_scope_appends)\b/i.test(text)) {
        violations.push({
          rule: "waylode_duplicate_dc_store_ddl",
          detail: `Waylode:${rel} defines forbidden duplicate dc-store DDL`,
        });
      }
    }
    status.Waylode = "checked";
  }

  // 3. Manvi forbidden checks
  if (!fs.existsSync(roots.manvi)) {
    status.Manvi = "absent";
  } else {
    const mvFiles = walkFiles(path.join(roots.manvi, "manvi"), { ignoreDirs, ext: ".go", ignoreTests: true });
    for (const f of mvFiles) {
      const rel = path.relative(roots.manvi, f);
      const text = fs.readFileSync(f, "utf8");
      if (/CREATE\s+TABLE(?:\s+IF\s+NOT\s+EXISTS)?\s+(?:task_leases|work_items|task_scope_appends)\b/i.test(text)) {
        violations.push({
          rule: "manvi_duplicate_dc_store_ddl",
          detail: `Manvi:${rel} defines forbidden duplicate dc-store DDL`,
        });
      }
      if (/\b(stripe_secret|paywalled|subscription_tier|requires_sign_in)\b/i.test(text)) {
        violations.push({
          rule: "manvi_forbidden_billing_or_auth_gate",
          detail: `Manvi:${rel} contains forbidden billing/subscription/sign-in identifier`,
        });
      }
    }
    status.Manvi = "checked";
  }
  return status;
}

export function loadMatrix(devcouncilRoot = DEFAULT_DEVCOUNCIL_ROOT) {
  const matrixPath = path.join(
    devcouncilRoot,
    "specs",
    "003-stack-boundaries-and-non-overlap",
    "contracts",
    "ownership-matrix.json",
  );
  return JSON.parse(fs.readFileSync(matrixPath, "utf8"));
}

export function runChecks(roots = defaultRoots()) {
  const matrix = loadMatrix(roots.devcouncil);
  const violations = [];
  const trackedOpenOverlaps = collectMatrixOverlaps(matrix).filter(
    (o) => o.status === "open" || o.status === "partial",
  );

  checkOwnershipAndLanes(matrix, violations);
  const seams = checkSeams(matrix, roots, violations);
  const docHygiene = [
    checkPublicDocHygiene("DevCouncil", roots.devcouncil, violations),
    checkPublicDocHygiene("Manvi", roots.manvi, violations),
    checkPublicDocHygiene("Waylode", roots.waylode, violations),
  ];
  const skillsBoundary = checkSkillsBoundary(roots, trackedOpenOverlaps, violations);
  const sharedTools = checkSharedDevCouncilTools(roots, trackedOpenOverlaps, violations);
  const forbiddenGuards = checkForbiddenEngineAndBillingRules(roots, violations);

  return {
    ok: violations.length === 0,
    matrix_updated_at: matrix.updated_at,
    capability_count: (matrix.capabilities || []).length,
    lane_count: (matrix.work_lanes || []).length,
    seams_summary: {
      ok: seams.filter((s) => s.status === "ok").length,
      absent: seams.filter((s) => s.status === "absent").length,
      missing: seams.filter((s) => s.status === "missing").length,
    },
    doc_hygiene: docHygiene,
    skills_boundary: skillsBoundary,
    shared_tools: sharedTools,
    forbidden_guards: forbiddenGuards,
    tracked_open_overlaps: trackedOpenOverlaps,
    violations,
  };
}

function matchLanePathPattern(relPath, rawPattern) {
  const pattern = String(rawPattern || "").trim();
  if (!pattern) return false;
  if (pattern === "*") return true;
  const clean = pattern.replace(/\/+$/, "");
  return relPath === clean || relPath.startsWith(`${clean}/`) || relPath.startsWith(pattern);
}

function findForbiddenMatch(lane, relPath, queryRepo) {
  for (const rawForbidden of lane.forbidden_paths || []) {
    const colonIdx = rawForbidden.indexOf(":");
    let targetRepo = lane.repo;
    let pattern = rawForbidden;
    if (colonIdx > 0) {
      targetRepo = normalizeRepoName(rawForbidden.slice(0, colonIdx));
      pattern = rawForbidden.slice(colonIdx + 1).trim();
    }
    if (targetRepo !== lane.repo && targetRepo !== queryRepo) {
      continue;
    }
    if (matchLanePathPattern(relPath, pattern)) {
      return rawForbidden;
    }
  }
  return null;
}

export function routePath(targetSpec, roots = defaultRoots()) {
  const idx = String(targetSpec || "").indexOf(":");
  if (idx <= 0 || idx === targetSpec.length - 1) {
    return { ok: false, error: "route target must have form <repo>:<path>" };
  }
  const repo = normalizeRepoName(targetSpec.slice(0, idx));
  const relPath = targetSpec.slice(idx + 1).replace(/^\/+/, "");
  const matrix = loadMatrix(roots.devcouncil);

  const candidates = (matrix.work_lanes || []).filter((l) => {
    if (l.repo === repo) return true;
    // Waylode Primary and Waylode Labs share a repository tree (.gitpulse/worktrees/waylode-labs),
    // so Waylode:<labs-path> matching lane-d-waylode-labs owned_paths routes to lane-d-waylode-labs.
    if (repo === "Waylode" && l.repo === "Waylode Labs") return true;
    return false;
  });
  let matchedLane = null;
  let bestLen = -1;
  for (const lane of candidates) {
    for (const ownedRaw of lane.owned_paths || []) {
      const owned = ownedRaw.replace(/\/+$/, "");
      if (relPath === owned || relPath.startsWith(`${owned}/`) || relPath.startsWith(ownedRaw)) {
        if (owned.length > bestLen) {
          bestLen = owned.length;
          matchedLane = lane;
        }
      }
    }
  }
  // Root-level files in a single-lane repo fall back to that repo's lane unless matched elsewhere.
  if (!matchedLane) {
    const exactRepoCandidates = candidates.filter((l) => l.repo === repo);
    if (exactRepoCandidates.length === 1) {
      matchedLane = exactRepoCandidates[0];
    }
  }
  if (!matchedLane) {
    return {
      ok: false,
      forbidden: false,
      repo,
      path: relPath,
      error: `no work lane owns ${repo}:${relPath}`,
    };
  }

  const owningRepoRoot = repoRootFor(matchedLane.repo, roots);
  const exists = owningRepoRoot && fs.existsSync(owningRepoRoot)
    ? fs.existsSync(path.join(owningRepoRoot, relPath))
    : null;

  const relatedCapabilities = [];
  for (const cap of matrix.capabilities || []) {
    const seamHits = [...(cap.owner_seams || []), ...(cap.adapters || []).flatMap((a) => a.seams || [])]
      .filter((s) => s.toLowerCase().includes(relPath.toLowerCase()));
    if (seamHits.length > 0) {
      relatedCapabilities.push({
        id: cap.id,
        canonical_owner: cap.canonical_owner,
        seams: seamHits,
        parity_checks: cap.parity_checks || [],
      });
    }
  }

  const forbiddenPattern = findForbiddenMatch(matchedLane, relPath, repo);
  if (forbiddenPattern) {
    return {
      ok: false,
      forbidden: true,
      repo: matchedLane.repo,
      path: relPath,
      path_status: exists === null ? "repo_absent" : exists ? "exists" : "new_or_missing",
      owning_lane: matchedLane.lane_id,
      lane_name: matchedLane.name,
      edition: matchedLane.edition,
      landing_order: matchedLane.landing_order,
      forbidden_paths: matchedLane.forbidden_paths,
      matched_forbidden_path: forbiddenPattern,
      handoff_upstream: matchedLane.handoff_upstream,
      required_checks: matchedLane.required_checks,
      related_capabilities: relatedCapabilities,
      error: `${repo}:${relPath} matches forbidden_paths '${forbiddenPattern}' on owning lane ${matchedLane.lane_id}`,
    };
  }

  return {
    ok: true,
    forbidden: false,
    repo: matchedLane.repo,
    path: relPath,
    path_status: exists === null ? "repo_absent" : exists ? "exists" : "new_or_missing",
    owning_lane: matchedLane.lane_id,
    lane_name: matchedLane.name,
    edition: matchedLane.edition,
    landing_order: matchedLane.landing_order,
    forbidden_paths: matchedLane.forbidden_paths,
    handoff_upstream: matchedLane.handoff_upstream,
    required_checks: matchedLane.required_checks,
    related_capabilities: relatedCapabilities,
  };
}

function main() {
  const args = process.argv.slice(2);
  let json = false;
  let lanesMode = false;
  let routeTarget = null;
  const roots = defaultRoots();

  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === "--json") json = true;
    else if (a === "--lanes") lanesMode = true;
    else if (a === "--route") routeTarget = args[++i] || "";
    else if (a.startsWith("--route=")) routeTarget = a.slice("--route=".length);
    else if (a === "--root") roots.devcouncil = path.resolve(args[++i] || ".");
    else if (a === "--manvi-root") roots.manvi = path.resolve(args[++i] || ".");
    else if (a === "--waylode-root") roots.waylode = path.resolve(args[++i] || ".");
    else if (a === "--labs-root") roots.labs = path.resolve(args[++i] || ".");
    else if (a === "--help" || a === "-h") {
      console.log("Usage: node scripts/check-stack-boundaries.mjs [--json] [--lanes] [--route <repo>:<path>]");
      process.exit(0);
    } else {
      console.error(`Unknown argument: ${a}`);
      process.exit(2);
    }
  }

  if (lanesMode) {
    const matrix = loadMatrix(roots.devcouncil);
    const out = {
      landing_order: matrix.landing_order,
      work_lanes: matrix.work_lanes,
    };
    if (json) {
      console.log(JSON.stringify(out, null, 2));
    } else {
      console.log(`Landing order: ${out.landing_order.join(" -> ")}`);
      for (const l of out.work_lanes) {
        console.log(`- [${l.landing_order}] ${l.lane_id} (${l.repo} / ${l.edition}): owns ${l.owned_paths.join(", ")}`);
      }
    }
    process.exit(0);
  }

  if (routeTarget !== null) {
    const routed = routePath(routeTarget, roots);
    if (json) {
      console.log(JSON.stringify(routed, null, 2));
    } else if (routed.forbidden) {
      console.error(`Forbidden route (${routed.owning_lane}): ${routed.error}`);
    } else if (!routed.ok) {
      console.error(`Route error: ${routed.error}`);
    } else {
      console.log(`${routed.repo}:${routed.path} -> ${routed.owning_lane} (${routed.edition}, landing #${routed.landing_order}, path: ${routed.path_status})`);
      console.log(`  Required checks: ${routed.required_checks.join(" | ")}`);
    }
    process.exit(routed.ok && !routed.forbidden ? 0 : 1);
  }

  const report = runChecks(roots);
  if (json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log(`Stack boundary check: ${report.ok ? "PASS" : "FAIL"} (${report.capability_count} capabilities, ${report.lane_count} lanes)`);
    console.log(`Seams: ${report.seams_summary.ok} ok, ${report.seams_summary.absent} absent, ${report.seams_summary.missing} missing`);
    console.log(`tracked_open_overlaps (${report.tracked_open_overlaps.length}): ${report.tracked_open_overlaps.map((o) => `${o.id}[${o.status}->${o.closure_task}]`).join(", ")}`);
    if (report.violations.length > 0) {
      for (const v of report.violations) {
        console.error(`  VIOLATION [${v.rule}]: ${v.detail}`);
      }
    }
  }
  process.exit(report.ok ? 0 : 1);
}

if (process.argv[1] && path.resolve(process.argv[1]) === __filename) {
  main();
}
