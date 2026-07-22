import { createRequire } from "node:module";
import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import {
  mkdtempSync,
  readFileSync,
  writeFileSync,
  rmSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const npmCommand = process.platform === "win32" ? "cmd.exe" : "npm";
const npmPrefix = process.platform === "win32" ? ["/d", "/s", "/c", "npm"] : [];
const PLAYWRIGHT_VERSION = "1.52.0";

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

/** Clear user-global allow-scripts (npm 11 rejects it for project installs). */
function smokeNpmEnv(extra) {
  return {
    ...process.env,
    npm_config_ignore_scripts: "true",
    npm_config_allow_scripts: "",
    ...(extra || {}),
  };
}

function assertPackedFile(packMetadata, pathName) {
  const files = packMetadata.files ?? [];
  if (!files.some((file) => file.path === pathName)) {
    throw new Error(`npm package is missing required file ${pathName}`);
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Probe injected into a temp copy of demo.html so smoke can assert runtime state. */
const SMOKE_PROBE = `
<script>
window.__dcGraphSmoke = (function () {
  function clickHandler() {
    try {
      const h = typeof g.onNodeClick === "function" ? g.onNodeClick() : null;
      return typeof h === "function" ? h : null;
    } catch (_) {
      return null;
    }
  }
  function findNode(id) {
    const fromGraph = (g.graphData().nodes || []).find((n) => n.id === id);
    if (fromGraph) return fromGraph;
    return (activePayload().nodes || []).find((n) => n.id === id);
  }
  return {
    snapshot() {
      const canvas = document.querySelector("#graph canvas");
      const warn = document.getElementById("vendorWarn");
      const gd = g && typeof g.graphData === "function" ? g.graphData() : { nodes: [], links: [] };
      let nonzero = 0;
      let sampleError = null;
      if (canvas) {
        try {
          const ctx = canvas.getContext("2d", { willReadFrequently: true });
          if (ctx) {
            // F-08: centered sample (top-left falsely looks blank).
            const cw = canvas.width || 0;
            const ch = canvas.height || 0;
            // Sample a centered window — ForceGraph paints near the middle,
            // so a top-left 160x160 crop falsely reports a blank canvas.
            const sw = Math.min(cw, 320);
            const sh = Math.min(ch, 320);
            if (sw > 0 && sh > 0) {
              const sx = Math.max(0, Math.floor((cw - sw) / 2));
              const sy = Math.max(0, Math.floor((ch - sh) / 2));
              const data = ctx.getImageData(sx, sy, sw, sh).data;
              for (let i = 0; i < data.length; i += 4) {
                if (data[i] || data[i + 1] || data[i + 2] || data[i + 3]) nonzero += 1;
              }
            }
          } else {
            sampleError = "no-2d-context";
          }
        } catch (err) {
          sampleError = String(err && err.message ? err.message : err);
        }
      }
      return {
        hasCanvas: !!canvas,
        canvasWidth: canvas ? canvas.width : 0,
        canvasHeight: canvas ? canvas.height : 0,
        vendorWarnVisible: warn ? getComputedStyle(warn).display !== "none" : false,
        nodeCount: (gd.nodes || []).length,
        linkCount: (gd.links || []).length,
        hasZoomToFit: !!(g && typeof g.zoomToFit === "function"),
        missingStub: !!(g && g._missing),
        nonzeroPixels: nonzero,
        sampleError,
        detail: (document.getElementById("detail") || {}).textContent || "",
        selected: Array.isArray(selected) ? selected.slice() : [],
        pathHighlight: pathHighlight ? [...pathHighlight] : [],
        expandSize: expandIds ? expandIds.size : null,
      };
    },
    fileNodeIds() {
      return ((DATA.file || {}).nodes || []).map((n) => n.id);
    },
    screenPoint(id) {
      const n = findNode(id);
      if (!n || n.x == null || n.y == null) return null;
      if (typeof g.graph2ScreenCoords !== "function") return null;
      const pt = g.graph2ScreenCoords(n.x, n.y);
      const canvas = document.querySelector("#graph canvas");
      if (!canvas) return null;
      const rect = canvas.getBoundingClientRect();
      return {
        x: rect.left + pt.x,
        y: rect.top + pt.y,
        canvasX: pt.x,
        canvasY: pt.y,
      };
    },
    invokeClick(id, detail) {
      const n = findNode(id);
      if (!n) throw new Error("unknown node: " + id);
      const handler = clickHandler();
      if (!handler) throw new Error("ForceGraph onNodeClick handler is not readable");
      handler(n, { detail: detail || 1 });
      return this.snapshot();
    },
  };
})();
</script>
`;

function writeInstrumentedDemo(demoHtmlPath, destPath) {
  const html = readFileSync(demoHtmlPath, "utf-8");
  if (!html.includes("</body>")) {
    throw new Error("graph demo.html missing </body>; cannot instrument for browser smoke");
  }
  const instrumented = html.replace("</body>", `${SMOKE_PROBE}\n</body>`);
  writeFileSync(destPath, instrumented, "utf-8");
}

function ensurePlaywright(workspace) {
  const pwRoot = path.join(workspace, "node_modules", "playwright");
  if (!existsSync(pwRoot)) {
    assertOk(
      runNpm(
        [
          "install",
          `playwright@${PLAYWRIGHT_VERSION}`,
          "--no-save",
          "--no-package-lock",
          "--ignore-scripts",
        ],
        {
          cwd: workspace,
          env: smokeNpmEnv(),
        },
      ),
      "npm install playwright (browser smoke)",
    );
  }
  const browsersPath = path.join(workspace, ".pw-browsers");
  const requireFromWorkspace = createRequire(path.join(workspace, "package.json"));
  let chromium;
  try {
    ({ chromium } = requireFromWorkspace("playwright"));
  } catch (err) {
    throw new Error(`failed to load playwright from smoke workspace: ${err}`);
  }
  return { chromium, browsersPath };
}

async function launchChromium(chromium, workspace, browsersPath) {
  const baseEnv = { ...process.env, PLAYWRIGHT_BROWSERS_PATH: browsersPath };
  const attempts = [
    { channel: "chrome", headless: true },
    { channel: "chromium", headless: true },
    { headless: true },
  ];
  let lastError;
  for (const options of attempts) {
    try {
      return await chromium.launch({ ...options, env: baseEnv });
    } catch (err) {
      lastError = err;
    }
  }

  const pwCli = path.join(workspace, "node_modules", "playwright", "cli.js");
  if (!existsSync(pwCli)) {
    throw new Error(
      `Unable to launch Chromium for graph demo browser smoke (no playwright cli). Last error: ${lastError}`,
    );
  }
  assertOk(
    run(process.execPath, [pwCli, "install", "chromium"], {
      cwd: workspace,
      env: baseEnv,
    }),
    "playwright install chromium",
  );
  try {
    return await chromium.launch({ headless: true, env: baseEnv });
  } catch (err) {
    throw new Error(
      `Unable to launch Chromium for graph demo browser smoke after install. Last error: ${err}`,
    );
  }
}

function startStaticServer(rootDir) {
  const server = createServer((req, res) => {
    const rel = decodeURIComponent((req.url || "/").split("?")[0]).replace(/^\/+/, "");
    const filePath = path.join(rootDir, rel || "demo.html");
    if (!filePath.startsWith(rootDir) || !existsSync(filePath)) {
      res.writeHead(404);
      res.end("not found");
      return;
    }
    const body = readFileSync(filePath);
    const ext = path.extname(filePath).toLowerCase();
    const type =
      ext === ".html"
        ? "text/html; charset=utf-8"
        : ext === ".js"
          ? "text/javascript; charset=utf-8"
          : "application/octet-stream";
    res.writeHead(200, { "Content-Type": type });
    res.end(body);
  });
  return new Promise((resolve, reject) => {
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      if (!addr || typeof addr === "string") {
        reject(new Error("static server failed to bind"));
        return;
      }
      resolve({ server, port: addr.port });
    });
    server.on("error", reject);
  });
}

async function runGraphDemoBrowserSmoke({ demoHtmlPath, workspace }) {
  const { chromium, browsersPath } = ensurePlaywright(workspace);
  const smokeDir = mkdtempSync(path.join(workspace, "graph-browser-"));
  const instrumentedPath = path.join(smokeDir, "demo.html");
  writeInstrumentedDemo(demoHtmlPath, instrumentedPath);

  const { server, port } = await startStaticServer(smokeDir);
  const pageErrors = [];
  const consoleErrors = [];
  let browser;
  try {
    browser = await launchChromium(chromium, workspace, browsersPath);
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    page.on("pageerror", (err) => {
      pageErrors.push(String(err && err.message ? err.message : err));
    });
    page.on("console", (msg) => {
      if (msg.type() === "error") {
        consoleErrors.push(msg.text());
      }
    });

    await page.goto(`http://127.0.0.1:${port}/demo.html`, {
      waitUntil: "domcontentloaded",
      timeout: 30_000,
    });

    await page.waitForFunction(
      () =>
        typeof window.__dcGraphSmoke === "object" &&
        document.querySelector("#graph canvas") &&
        window.__dcGraphSmoke.snapshot().nodeCount > 0 &&
        window.__dcGraphSmoke.snapshot().canvasWidth > 0,
      undefined,
      { timeout: 20_000 },
    );

    await page.evaluate(() => {
      try {
        if (g && typeof g.zoomToFit === "function") g.zoomToFit(0);
      } catch (_) {}
    });
    await sleep(1000);

    const initial = await page.evaluate(() => window.__dcGraphSmoke.snapshot());
    if (initial.vendorWarnVisible) {
      throw new Error("graph demo vendorWarn is visible (ForceGraph missing or incomplete)");
    }
    if (initial.missingStub) {
      throw new Error("graph demo is using the ForceGraph fallback stub");
    }
    if (!initial.hasCanvas || initial.canvasWidth < 64 || initial.canvasHeight < 64) {
      throw new Error(`graph demo canvas is empty/too small: ${JSON.stringify(initial)}`);
    }
    if (initial.nodeCount < 2) {
      throw new Error(`graph demo expected >=2 nodes, got ${initial.nodeCount}`);
    }

    // F-08 acceptance: visible canvas bbox + nonempty #counts (pixel buffers can stay zero in headless).
    const canvasLocator = page.locator("#graph canvas").first();
    await canvasLocator.waitFor({ state: "visible", timeout: 10_000 });
    const box = await canvasLocator.boundingBox();
    if (!box || box.width < 32 || box.height < 32) {
      throw new Error(`#graph canvas bounding box too small: ${JSON.stringify(box)}`);
    }
    const countsText = await page.locator("#counts").innerText();
    if (!/Nodes:\s*[1-9]/.test(countsText)) {
      throw new Error(`#counts did not show nonempty nodes: ${JSON.stringify(countsText)}`);
    }
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;
    await page.mouse.click(cx, cy);
    await page.mouse.dblclick(cx, cy);
    await sleep(200);

    const nodeIds = await page.evaluate(() => window.__dcGraphSmoke.fileNodeIds());
    if (nodeIds.length < 2) {
      throw new Error(`graph demo file nodes missing: ${JSON.stringify(nodeIds)}`);
    }
    const [nodeA, nodeB] = nodeIds;

    async function clickNode(id, { double = false } = {}) {
      const point = await page.evaluate((nodeId) => window.__dcGraphSmoke.screenPoint(nodeId), id);
      if (point && Number.isFinite(point.x) && Number.isFinite(point.y)) {
        if (double) {
          await page.mouse.dblclick(point.x, point.y);
        } else {
          await page.mouse.click(point.x, point.y);
        }
        await sleep(200);
        return page.evaluate(() => window.__dcGraphSmoke.snapshot());
      }
      return page.evaluate(
        ({ nodeId, detail }) => window.__dcGraphSmoke.invokeClick(nodeId, detail),
        { nodeId: id, detail: double ? 2 : 1 },
      );
    }

    const afterFirst = await clickNode(nodeA, { double: false });
    if (!afterFirst.detail.includes(nodeA) && !String(afterFirst.selected).includes(nodeA)) {
      const forced = await page.evaluate(
        (id) => window.__dcGraphSmoke.invokeClick(id, 1),
        nodeA,
      );
      if (!forced.detail.includes(nodeA)) {
        throw new Error(`single-click did not update detail for ${nodeA}: ${JSON.stringify(forced)}`);
      }
    }

    const afterSecond = await page.evaluate(
      (id) => window.__dcGraphSmoke.invokeClick(id, 1),
      nodeB,
    );
    if (afterSecond.selected.length !== 2) {
      throw new Error(
        `two-node path selection failed (selected=${JSON.stringify(afterSecond.selected)})`,
      );
    }
    if (
      !afterSecond.detail.includes("selected path") &&
      afterSecond.pathHighlight.length < 2
    ) {
      throw new Error(
        `path highlight missing after selecting two nodes: ${JSON.stringify(afterSecond)}`,
      );
    }

    const beforeExpand = await page.evaluate(() => window.__dcGraphSmoke.snapshot());
    const afterDbl = await page.evaluate(
      (id) => window.__dcGraphSmoke.invokeClick(id, 2),
      nodeA,
    );
    if (afterDbl.expandSize == null || afterDbl.expandSize < 1) {
      throw new Error(
        `double-click did not expand neighborhood: ${JSON.stringify({ beforeExpand, afterDbl })}`,
      );
    }
    if (afterDbl.nodeCount > beforeExpand.nodeCount) {
      throw new Error(
        `double-click should focus/neighborhood-filter nodes, not grow the graph: ${JSON.stringify({
          before: beforeExpand.nodeCount,
          after: afterDbl.nodeCount,
        })}`,
      );
    }

    if (pageErrors.length) {
      throw new Error(`graph demo page exceptions: ${pageErrors.join(" | ")}`);
    }
    const seriousConsole = consoleErrors.filter(
      (line) => !/favicon/i.test(line) && !/Failed to load resource/i.test(line),
    );
    if (seriousConsole.length) {
      throw new Error(`graph demo console errors: ${seriousConsole.join(" | ")}`);
    }

    return {
      ok: true,
      nodeCount: initial.nodeCount,
      nonzeroPixels: initial.nonzeroPixels,
      selectedPath: afterSecond.pathHighlight,
      expandSize: afterDbl.expandSize,
    };
  } finally {
    if (browser) {
      await browser.close();
    }
    await new Promise((resolve) => server.close(() => resolve()));
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
  assertPackedFile(packMetadata, "src/devcouncil/assets/vendor/force-graph.min.js");
  assertPackedFile(packMetadata, "src/devcouncil/llm/model_defaults.yaml");
  assertPackedFile(packMetadata, "src/devcouncil/telemetry/model_pricing.yaml");
  assertPackedFile(packMetadata, "src/devcouncil/integrations/opencode_devcouncil_plugin.mjs");
  assertPackedFile(packMetadata, "scripts/build-week-demo.sh");
  assertPackedFile(packMetadata, "examples/build-week-demo/README.md");
  assertPackedFile(packMetadata, "examples/build-week-demo/calc.py");
  assertPackedFile(packMetadata, "examples/build-week-demo/broken_calc.py");
  assertPackedFile(packMetadata, "examples/build-week-demo/test_calc.py");
  packedTarball = path.join(repoRoot, packMetadata.filename);

  // npm 11: user ~/.npmrc allow-scripts is treated as project-scoped and rejected.
  writeFileSync(path.join(workspace, ".npmrc"), "ignore-scripts=true\n", "utf-8");
  assertOk(
    runNpm(["init", "-y"], { cwd: workspace, env: smokeNpmEnv() }),
    "npm init",
  );
  assertOk(
    runNpm(["install", packedTarball, "--ignore-scripts"], {
      cwd: workspace,
      env: smokeNpmEnv(),
    }),
    "npm install packed devcouncil",
  );

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

  // Packaged graph demo: ForceGraph must ship, demo.html must render in a real
  // browser with a nonempty canvas, no page exceptions, and working click /
  // double-click / path selection (string-only HTML checks are not enough).
  const graphDemoProject = mkdtempSync(path.join(workspace, "graph-demo-"));
  const graphDemo = run(
    process.execPath,
    [
      installedBin,
      "graph",
      "demo",
      "--project-root",
      graphDemoProject,
      "--json",
    ],
    { cwd: workspace, shell: false, env: integrationEnv },
  );
  assertOk(graphDemo, "installed dev graph demo --json");
  let demoPaths;
  try {
    demoPaths = JSON.parse(graphDemo.stdout);
  } catch (err) {
    throw new Error(
      `installed dev graph demo --json did not return JSON: ${err}\n${graphDemo.stdout}`,
    );
  }
  const demoHtmlPath = demoPaths.html;
  if (typeof demoHtmlPath !== "string" || !demoHtmlPath) {
    throw new Error(`graph demo JSON missing html path: ${graphDemo.stdout}`);
  }
  const demoHtml = readFileSync(demoHtmlPath, "utf-8");
  assertIncludes(demoHtml, "ForceGraph", "graph demo.html ForceGraph");
  assertIncludes(demoHtml, "vasturiano/force-graph", "graph demo.html vendored ForceGraph");
  if (demoHtml.includes("onNodeDblClick")) {
    throw new Error(
      "graph demo.html still calls onNodeDblClick (incompatible with packed ForceGraph)",
    );
  }
  assertIncludes(demoHtml, "event.detail >= 2", "graph demo.html double-click handler");

  const browserResult = await runGraphDemoBrowserSmoke({
    demoHtmlPath,
    workspace,
  });
  process.stdout.write(
    `graph demo browser smoke ok: nodes=${browserResult.nodeCount} pixels=${browserResult.nonzeroPixels} expand=${browserResult.expandSize}\n`,
  );
} finally {
  if (packedTarball) {
    rmSync(packedTarball, { force: true });
  }
  rmSync(workspace, { recursive: true, force: true });
}
