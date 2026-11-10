// Real-browser regression for the shared code-map page.
//
// Uses an already-installed browser automation tool; never installs packages.
// For every page file it loads the file offline (any non-file request aborts the
// run), then asserts:
//   * nothing threw and nothing was logged as an error or warning;
//   * a repository name that is markup stays text in every place the page shows
//     it (title, node list, inspector, navigator, detail pane) and executes
//     nothing;
//   * the page actually drew: one canvas, with painted pixels.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const { chromium } = require(process.env.DEVMAP_PLAYWRIGHT_MODULE);

const ACTIVE = ['img', 'svg', 'script', 'iframe', 'object', 'embed', 'style', 'link'];
// A scoped selector list: a bare `.a img,svg` would scope only the first tag.
const within = (...scopes) => scopes.flatMap((scope) => ACTIVE.map((tag) => `${scope} ${tag}`)).join(',');

async function paintedPixels(page) {
  return page.evaluate(() => {
    const canvas = document.querySelector('canvas');
    if (!canvas) return -1;
    const ctx = canvas.getContext('2d');
    const { data } = ctx.getImageData(0, 0, canvas.width, canvas.height);
    const first = [data[0], data[1], data[2], data[3]].join();
    let differing = 0;
    for (let i = 0; i < data.length; i += 4) {
      if ([data[i], data[i + 1], data[i + 2], data[i + 3]].join() !== first) differing++;
    }
    return differing;
  });
}

(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.DEVMAP_BROWSER_EXECUTABLE });
  const results = [];
  try {
    for (const file of process.argv.slice(2)) {
      const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
      const problems = [];
      page.on('console', (m) => { if (m.type() === 'error' || m.type() === 'warning') problems.push(`${m.type()}: ${m.text()}`); });
      page.on('pageerror', (e) => problems.push(`pageerror: ${e.message}`));
      try {
        await page.route('**/*', (route) => (route.request().url().startsWith('file:') ? route.continue() : route.abort()));
        await page.goto(pathToFileURL(file).href);
        await page.waitForSelector('canvas', { timeout: 5000 });
        await page.waitForTimeout(300);
        const isMap = path.basename(file).endsWith('-map.html');

        // Everything the page shows a name in, before any interaction.
        assert.equal(await page.evaluate(() => !!window.__markupExecuted), false, 'page load executed repository markup');
        assert.equal(await page.locator(within('.cm-title', '.cm-counts')).count(), 0, 'markup in the title or counts');
        assert.ok(await paintedPixels(page) > 50, 'the canvas has nothing drawn on it');

        // The node list, then select the first node from it.
        await page.getByRole('button', { name: 'Nodes' }).click();
        const row = page.locator('.cm-results .cm-row').first();
        await row.waitFor({ timeout: 5000 });
        const label = await row.locator('strong').innerText();
        assert.ok(label.length > 0, 'node row has no label');
        assert.equal(await page.locator(within('.cm-results')).count(), 0, 'markup in the node list');
        await row.click();
        await page.waitForSelector('.cm-inspector-copy', { timeout: 5000 });
        assert.equal(await page.locator(within('.cm-inspector')).count(), 0, 'markup in the inspector');
        const inspected = await page.locator('.cm-inspector-copy strong').innerText();
        assert.equal(inspected, label, 'the inspector does not show the literal label');

        if (path.basename(file) === 'ordinary-graph.html') {
          // Hide notes & Markdown drops the document node and nothing else.
          await page.getByRole('button', { name: 'Filters' }).click();
          await page.getByLabel('Hide notes & Markdown').check();
          const filtered = await page.locator('[data-testid=graph-filter-count]').innerText();
          assert.match(filtered, /1 of 2 match filters/, 'Hide notes & Markdown did not hide the Markdown node');
        }
        assert.equal(await page.evaluate(() => document.documentElement.dataset.renderer), 'devmap-code-map', 'not the shared renderer');

        let navigatorText = null;
        if (isMap) {
          const navRow = page.locator('.nav-row[role=treeitem]').first();
          await navRow.click();
          assert.equal(await page.locator(within('.nav', '.page-side')).count(), 0, 'markup in the navigator or sidebar');
          navigatorText = await page.locator('.nav-detail-title h2').innerText();
          assert.ok(navigatorText.length > 0, 'detail pane shows no area');
        }
        assert.equal(await page.evaluate(() => !!window.__markupExecuted), false, 'interaction executed repository markup');
        assert.deepEqual(problems, [], `console problems: ${problems.join(' | ')}`);
        await page.screenshot({ path: file + '.png' });
        results.push({ file: path.basename(file), passed: true, label, navigatorText });
      } catch (error) {
        results.push({ file: path.basename(file), passed: false, error: String(error) });
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
  }
  const report = { browser: 'installed Chromium', results };
  fs.writeFileSync(path.join(path.dirname(process.argv[2]), 'browser-results.json'), JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
  if (results.length !== process.argv.length - 2 || results.some((result) => !result.passed)) process.exitCode = 1;
})().catch((error) => { console.error(error); process.exitCode = 1; });
