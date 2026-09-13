// Uses an already-installed browser automation tool; never installs packages.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const { chromium } = require(process.env.DEVMAP_PLAYWRIGHT_MODULE);

(async () => {
  const browser = await chromium.launch({headless: true, executablePath: process.env.DEVMAP_BROWSER_EXECUTABLE});
  const results = [];
  try {
    for (const file of process.argv.slice(2)) {
      const page = await browser.newPage({viewport: {width: 1280, height: 900}});
      try {
        await page.route('**/*', route => route.request().url().startsWith('file:') ? route.continue() : route.abort());
        await page.addInitScript(() => {
          let renderer;
          Object.defineProperty(window, 'ForceGraph', {
            configurable: true,
            get: () => renderer,
            set: original => {
              renderer = (...args) => {
                const instance = original(...args);
                window.__testGraph = instance;
                return instance;
              };
            }
          });
        });
        await page.goto(pathToFileURL(file).href);
        await page.waitForFunction(() => window.__testGraph?.graphData().nodes.length === 1, null, {timeout: 5000});
        assert.equal(await page.evaluate(() => !!window.__tooltipExecuted), false, 'page load executed repository markup');
        await page.evaluate(() => {
          const graph = window.__testGraph;
          const node = graph.graphData().nodes[0];
          node.fx = 0; node.fy = 0;
          graph.centerAt(0, 0, 0).zoom(1, 0);
        });
        await page.waitForFunction(() => {
          const node = window.__testGraph.graphData().nodes[0];
          return node.x === 0 && node.y === 0;
        }, null, {timeout: 5000});
        const point = await page.evaluate(() => window.__testGraph.graph2ScreenCoords(0, 0));
        const box = await page.locator('canvas').first().boundingBox();
        assert.ok(box, 'rendered graph canvas missing');
        await page.mouse.move(box.x + point.x, box.y + point.y);
        await page.waitForFunction(() => {
          const tip = document.querySelector('.float-tooltip-kap');
          return tip && tip.innerHTML.length > 0;
        }, null, {timeout: 5000});
        const state = await page.evaluate(() => {
          const tip = document.querySelector('.float-tooltip-kap');
          const node = window.__testGraph.graphData().nodes[0];
          return {
            executed: !!window.__tooltipExecuted,
            markup: tip.querySelectorAll('img,svg,script,iframe').length,
            text: tip.textContent,
            expected: node.area || node.path,
          };
        });
        await page.screenshot({path: file + '.png'});
        assert.equal(state.executed, false, 'hover executed repository markup');
        assert.equal(state.markup, 0, 'hover created active HTML elements');
        assert.ok(state.text.includes(state.expected), 'tooltip did not preserve the literal repository name');
        results.push({file: path.basename(file), passed: true, tooltip: state.text});
      } catch (error) {
        results.push({file: path.basename(file), passed: false, error: String(error)});
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
  }
  const report = {browser: 'installed Chromium', results};
  fs.writeFileSync(path.join(path.dirname(process.argv[2]), 'browser-results.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
  if (results.length !== 4 || results.some(result => !result.passed)) process.exitCode = 1;
})().catch(error => { console.error(error); process.exitCode = 1; });
