import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const npmCommand = process.platform === "win32" ? "cmd.exe" : "npm";
const npmPrefix = process.platform === "win32" ? ["/d", "/s", "/c", "npm"] : [];

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? process.env,
    encoding: "utf-8",
    shell: options.shell ?? false,
    stdio: options.stdio ?? "pipe",
  });
  if (result.error) {
    throw new Error(`${command} failed to start: ${result.error.message}`);
  }
  return result;
}

function assertOk(result, label) {
  if (result.status !== 0) {
    throw new Error(
      `${label} failed with exit ${result.status}\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
    );
  }
}

function assertIncludes(text, expected, label) {
  if (!text.includes(expected)) {
    throw new Error(`${label} did not include ${JSON.stringify(expected)}\n${text}`);
  }
}

function runNpm(args, options = {}) {
  return run(npmCommand, [...npmPrefix, ...args], options);
}

function assertPackedFile(packMetadata, pathName) {
  const files = packMetadata.files ?? [];
  if (!files.some((file) => file.path === pathName)) {
    throw new Error(`npm package is missing required file ${pathName}`);
  }
}

const workspace = mkdtempSync(path.join(tmpdir(), "devcouncil-npm-smoke-"));
let packedTarball;

try {
  const pack = runNpm(["pack", "--json"]);
  assertOk(pack, "npm pack");
  const packed = JSON.parse(pack.stdout);
  const packMetadata = packed[0];
  assertPackedFile(packMetadata, "src/devcouncil/assets/devcouncil-logo.svg");
  assertPackedFile(packMetadata, "src/devcouncil/assets/devcouncil_logo_premium.png");
  assertPackedFile(packMetadata, "src/devcouncil/llm/model_defaults.yaml");
  assertPackedFile(packMetadata, "src/devcouncil/telemetry/model_pricing.yaml");
  assertPackedFile(packMetadata, "src/devcouncil/integrations/opencode_devcouncil_plugin.mjs");
  packedTarball = path.join(repoRoot, packMetadata.filename);

  assertOk(runNpm(["init", "-y"], { cwd: workspace }), "npm init");
  assertOk(runNpm(["install", packedTarball], { cwd: workspace }), "npm install packed devcouncil");

  const installedBin = path.join(workspace, "node_modules", "devcouncil", "bin", "devcouncil.js");
  const help = run(process.execPath, [installedBin, "--help"], { cwd: workspace, shell: false });
  assertOk(help, "installed devcouncil --help");
  assertIncludes(help.stdout, "DevCouncil", "installed devcouncil --help");

  const doctor = run(process.execPath, [installedBin, "doctor"], { cwd: workspace, shell: false });
  assertOk(doctor, "installed devcouncil doctor");
  assertIncludes(doctor.stdout, "DevCouncil Doctor Check", "installed devcouncil doctor");

  const integrationProject = mkdtempSync(path.join(workspace, "integration-project-"));
  const nodeBin = path.join(workspace, "node_modules", ".bin");
  const integrationEnv = {
    ...process.env,
    PATH: `${nodeBin}${path.delimiter}${process.env.PATH ?? ""}`,
  };
  const init = run(process.execPath, [installedBin, "init"], {
    cwd: integrationProject,
    shell: false,
    env: integrationEnv,
  });
  assertOk(init, "installed devcouncil init for integration smoke");

  const hooks = run(
    process.execPath,
    [installedBin, "integrate", "hooks", "--apply"],
    { cwd: integrationProject, shell: false, env: integrationEnv },
  );
  assertOk(hooks, "installed devcouncil integrate hooks --apply");
  assertIncludes(hooks.stdout, "cursor native hooks configured", "integrate hooks cursor");
  assertIncludes(hooks.stdout, "opencode native hooks configured", "integrate hooks opencode");

  const integrateCheck = run(process.execPath, [installedBin, "integrate", "check"], {
    cwd: integrationProject,
    shell: false,
    env: integrationEnv,
  });
  assertOk(integrateCheck, "installed devcouncil integrate check");
  assertIncludes(integrateCheck.stdout, "Bundled OpenCode hook plugin", "integrate check bundled plugin");
  assertIncludes(integrateCheck.stdout, "Ready.", "integrate check ready");
  assertIncludes(integrateCheck.stdout, "Recommended coding CLI", "integrate check recommended executor");

  const recommend = run(process.execPath, [installedBin, "integrate", "recommend"], {
    cwd: integrationProject,
    shell: false,
    env: integrationEnv,
  });
  assertOk(recommend, "installed devcouncil integrate recommend");
  assertIncludes(recommend.stdout, "Integration Recommendations", "integrate recommend");

  const integrateStatus = run(process.execPath, [installedBin, "integrate", "status"], {
    cwd: integrationProject,
    shell: false,
    env: integrationEnv,
  });
  assertOk(integrateStatus, "installed devcouncil integrate status");
  assertIncludes(integrateStatus.stdout, "Integration Status", "integrate status");

  const matrix = run(process.execPath, [installedBin, "integrate", "matrix"], {
    cwd: integrationProject,
    shell: false,
    env: integrationEnv,
  });
  assertOk(matrix, "installed devcouncil integrate matrix");
  assertIncludes(matrix.stdout, "Integration Matrix", "integrate matrix");

  const integrateJson = run(
    process.execPath,
    [installedBin, "integrate", "check", "--json"],
    { cwd: integrationProject, shell: false, env: integrationEnv },
  );
  assertOk(integrateJson, "installed devcouncil integrate check --json");
  const report = JSON.parse(integrateJson.stdout);
  if (typeof report.ok !== "boolean" || !Array.isArray(report.checks)) {
    throw new Error("integrate check --json did not return expected shape");
  }

  const statusJson = run(
    process.execPath,
    [installedBin, "integrate", "status", "--json"],
    { cwd: integrationProject, shell: false, env: integrationEnv },
  );
  assertOk(statusJson, "installed devcouncil integrate status --json");
  const parsedStatus = JSON.parse(statusJson.stdout);
  if (!Array.isArray(parsedStatus.capabilities)) {
    throw new Error("integrate status --json did not include capabilities");
  }

  const reportPath = path.join(integrationProject, "integration-report.json");
  const integrateReportFile = run(
    process.execPath,
    [installedBin, "integrate", "check", "-o", reportPath],
    { cwd: integrationProject, shell: false, env: integrationEnv },
  );
  assertOk(integrateReportFile, "installed devcouncil integrate check -o");

  const emptyPath = mkdtempSync(path.join(workspace, "empty-path-"));
  const missingUv = run(process.execPath, [installedBin, "--help"], {
    cwd: workspace,
    shell: false,
    env: {
      SystemRoot: process.env.SystemRoot,
      WINDIR: process.env.WINDIR,
      PATH: emptyPath,
    },
  });
  if (missingUv.status === 0) {
    throw new Error("missing-uv smoke unexpectedly succeeded");
  }
  assertIncludes(
    `${missingUv.stdout}\n${missingUv.stderr}`,
    "DevCouncil requires uv to run from the npm package.",
    "missing-uv smoke",
  );
} finally {
  if (packedTarball) {
    rmSync(packedTarball, { force: true });
  }
  rmSync(workspace, { recursive: true, force: true });
}
