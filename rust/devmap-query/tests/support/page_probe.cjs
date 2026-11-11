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
// The `fit-*` and `scene-graph.html` pages additionally run, on a fresh load and
// through the page's read-only `canvas.cmProbe()` geometry hook:
//   * fit-to-view: finite, in-range scale and every shown node inside the stage
//     (one node, coincident nodes, after zoom/pan, small viewport, filtered and
//     empty sets);
//   * filters: after back-to-back changes the shown set and count are the last
//     change's (checked against an oracle read from the payload), and Clear /
//     Escape restore everything;
//   * globe: toggling repaints, arrow keys rotate, a far-side node is neither
//     hoverable nor clickable while a near-side one is.
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

// ---- camera, filters and globe ------------------------------------------------
// These run on a freshly loaded page (no panels left open by the generic checks)
// and read geometry through the page's read-only `canvas.cmProbe()` hook, never
// the pixels, so they fail on a wrong camera rather than on a wrong colour.

const frames = (page) => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));

async function snap(page) {
  await frames(page);
  return page.evaluate(() => document.querySelector('canvas').cmProbe());
}

// The payload the page was handed: an oracle for what a filter must leave, read
// independently of the page's own filtering code.
const payloadOf = (page) => page.evaluate(() => JSON.parse(document.getElementById('devmap-payload').textContent));

function expectedIds(nodes, { query = '', community = null, hideNotes = false } = {}) {
  const q = query.trim().toLowerCase();
  return nodes
    .filter((n) => !community || (n.community || n.area || '').trim() === community)
    .filter((n) => !hideNotes || n.documentation !== true)
    .filter((n) => !q || `${n.name} ${n.path} ${n.id}`.toLowerCase().includes(q))
    .map((n) => n.id)
    .sort();
}

const idsOf = (s) => s.nodes.map((n) => n.id).sort();

// (a) Fitting is bounded: a finite scale inside the page's own limits, and every
// shown node, with its radius, inside the stage.
function assertFits(s, what) {
  assert.ok(!s.globe, `${what}: expected the flat view`);
  assert.ok(Number.isFinite(s.scale), `${what}: fit produced a non-finite scale (${s.scale})`);
  assert.ok(s.scale >= s.flatZoomMin && s.scale <= s.flatZoomMax, `${what}: scale ${s.scale} is outside [${s.flatZoomMin}, ${s.flatZoomMax}]`);
  assert.ok(Number.isFinite(s.panX) && Number.isFinite(s.panY), `${what}: fit produced a non-finite pan (${s.panX}, ${s.panY})`);
  const slack = 0.5;
  for (const n of s.nodes) {
    assert.ok(Number.isFinite(n.x) && Number.isFinite(n.y) && Number.isFinite(n.r), `${what}: node ${n.id} has a non-finite position`);
    assert.ok(n.x - n.r >= -slack && n.x + n.r <= s.width + slack && n.y - n.r >= -slack && n.y + n.r <= s.height + slack,
      `${what}: node ${n.id} at (${n.x.toFixed(1)}, ${n.y.toFixed(1)}) r=${n.r.toFixed(1)} is outside the ${s.width}x${s.height} view`);
  }
}

async function fitScenario(page) {
  const total = (await payloadOf(page)).nodes.length;
  let s = await snap(page);
  assert.equal(s.nodes.length, total, 'fit: not every node is shown on load');
  assertFits(s, 'initial fit');
  const fitted = s.scale;

  // Throw the camera away, then ask for a fit again.
  await page.locator('canvas').focus();
  for (let i = 0; i < 6; i++) await page.keyboard.press('-');
  for (let i = 0; i < 5; i++) await page.keyboard.press('ArrowLeft');
  s = await snap(page);
  assert.notEqual(s.scale, fitted, 'fit: the zoom keys did not move the camera, so the fit below proves nothing');
  await page.getByRole('button', { name: 'Fit view' }).click();
  s = await snap(page);
  assertFits(s, 'fit after zoom and pan');
  assert.ok(Math.abs(s.scale - fitted) < 1e-9, `fit after zoom and pan: scale ${s.scale} differs from the first fit ${fitted}`);

  // A smaller stage, then a fit.
  await page.setViewportSize({ width: 520, height: 420 });
  await page.waitForTimeout(150);
  await page.getByRole('button', { name: 'Fit view' }).click();
  assertFits(await snap(page), 'fit in a small viewport');
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.waitForTimeout(150);

  // Fit follows the filtered set; an empty set still yields a finite view.
  if (total > 1) {
    const nodes = (await payloadOf(page)).nodes;
    await page.locator('.cm-search-input').fill(nodes[0].path);
    await page.getByRole('button', { name: 'Fit view' }).click();
    s = await snap(page);
    assert.deepEqual(idsOf(s), expectedIds(nodes, { query: nodes[0].path }), 'fit: the filtered set differs from the expected one');
    assertFits(s, 'fit of a filtered set');
  }
  await page.locator('.cm-search-input').fill('no-node-has-this-text');
  await page.getByRole('button', { name: 'Fit view' }).click();
  s = await snap(page);
  assert.equal(s.nodes.length, 0, 'fit: an unmatched search still shows nodes');
  assert.ok(Number.isFinite(s.scale) && s.scale >= s.flatZoomMin && s.scale <= s.flatZoomMax, `fit of an empty set: scale ${s.scale}`);
}

// (b) The last filter change wins and clearing restores everything. Changes are
// fired back to back inside one task so nothing can settle between them.
async function filterScenario(page) {
  const nodes = (await payloadOf(page)).nodes;
  assert.ok(nodes.some((n) => n.documentation === true), 'filters: the fixture has no document node for Hide notes to hide');
  const total = nodes.length;
  const settle = () => page.waitForTimeout(250);
  const check = async (what, want) => {
    await settle();
    const s = await snap(page);
    assert.deepEqual(idsOf(s), want, `${what}: the shown set is not the last change's`);
    assert.equal(await page.locator('[data-testid=graph-filter-count]').innerText(), `· ${want.length} of ${total} match filters`, `${what}: the count is stale`);
  };
  assert.ok(expectedIds(nodes, { query: 'core/f' }).length > 0 && expectedIds(nodes, { query: 'core/f' }).length < total, 'filters: the fixture cannot tell a query from no query');

  await page.getByRole('button', { name: 'Filters' }).click();
  await page.evaluate(() => {
    const type = (v) => { const i = document.querySelector('.cm-search-input'); i.value = v; i.dispatchEvent(new Event('input', { bubbles: true })); };
    for (const v of ['c', 'co', 'cor', 'ui/f', 'core/f1', 'core/f']) type(v);
  });
  await check('a burst of queries', expectedIds(nodes, { query: 'core/f' }));

  await page.evaluate(() => {
    const type = (v) => { const i = document.querySelector('.cm-search-input'); i.value = v; i.dispatchEvent(new Event('input', { bubbles: true })); };
    type('ui');
    document.querySelector('input[aria-label="Hide notes & Markdown"]').click();
    type('io/');
    document.querySelector('[data-map-group="io"]').click();
    type('f2');
  });
  await check('queries mixed with a toggle and a group', expectedIds(nodes, { query: 'f2', community: 'io', hideNotes: true }));

  await page.evaluate(() => {
    document.querySelector('input[aria-label="Hide notes & Markdown"]').click();
    document.querySelector('[data-map-group=""]').click();
  });
  await check('toggle and group undone', expectedIds(nodes, { query: 'f2' }));

  await page.getByTestId('map-clear-filters').click();
  await check('Clear filters', idsOf({ nodes }));
  assert.equal(await page.locator('.cm-search-input').inputValue(), '', 'Clear filters left the query in the box');
  // Clicking Clear closed the popover (a press outside it does); reopen to read the box.
  await page.getByRole('button', { name: 'Filters' }).click();
  assert.equal(await page.getByLabel('Hide notes & Markdown').isChecked(), false, 'Clear filters left Hide notes on');

  // Real key events, then Escape.
  await page.locator('.cm-search-input').click();
  await page.keyboard.type('ui/f1', { delay: 0 });
  await check('typed query', expectedIds(nodes, { query: 'ui/f1' }));
  await page.locator('canvas').focus();
  await page.keyboard.press('Escape');
  await check('Escape', idsOf({ nodes }));
}

async function globeScenario(page) {
  // toDataURL, not getImageData: repeated readbacks make Chrome warn, and the
  // probe fails any run that logs a warning.
  const signature = () => page.evaluate(() => {
    const url = document.querySelector('canvas').toDataURL();
    let h = 2166136261;
    for (let i = 0; i < url.length; i++) h = Math.imul(h ^ url.charCodeAt(i), 16777619) >>> 0;
    return h;
  });
  const nodes = (await payloadOf(page)).nodes;
  const flat = await snap(page);
  assert.equal(flat.globe, false, 'globe: the page starts in the globe view');
  const flatPixels = await signature();
  await page.getByRole('button', { name: 'Globe' }).click();
  let s = await snap(page);
  assert.equal(await page.getByRole('button', { name: 'Globe' }).getAttribute('aria-pressed'), 'true', 'Globe toggle is not pressed');
  assert.equal(s.globe, true, 'Globe did not switch the page to the globe projection');
  assert.ok(s.nodes.some((n) => n.front) && s.nodes.some((n) => !n.front), 'the globe has no near side and far side');
  assert.notEqual(await signature(), flatPixels, 'toggling Globe did not repaint the canvas');
  assert.ok(await paintedPixels(page) > 50, 'the globe canvas has nothing drawn on it');

  // Arrow keys rotate the sphere: yaw/pitch move and a visible node moves with them.
  const KEY = (10 * Math.PI) / 180;
  const before = s;
  await page.locator('canvas').focus();
  await page.keyboard.press('ArrowRight');
  s = await snap(page);
  assert.ok(Math.abs(s.yaw - before.yaw + KEY) < 1e-9, `ArrowRight turned yaw by ${s.yaw - before.yaw}, not ${-KEY}`);
  const moved = before.nodes.filter((n) => n.front).map((n) => Math.hypot(n.x - s.nodes.find((m) => m.id === n.id).x, n.y - s.nodes.find((m) => m.id === n.id).y));
  assert.ok(Math.max(...moved) > 1, `ArrowRight moved no node on screen (largest move ${Math.max(...moved)}px)`);
  assert.equal(s.panX, before.panX, 'a globe arrow key panned the flat camera');
  await page.keyboard.press('ArrowLeft');
  assert.ok(Math.abs((await snap(page)).yaw - before.yaw) < 1e-9, 'ArrowLeft did not undo ArrowRight');
  await page.keyboard.press('ArrowUp');
  s = await snap(page);
  assert.ok(Math.abs(s.pitch - before.pitch - KEY) < 1e-9, `ArrowUp tipped pitch by ${s.pitch - before.pitch}, not ${KEY}`);
  await page.keyboard.press('0');
  s = await snap(page);
  assert.ok(Math.abs(s.yaw) < 1e-9 && Math.abs(s.pitch) < 1e-9, 'the 0 key did not face the globe front-on');

  // A node on the far side cannot be hovered or clicked; a near-side one can.
  const rect = await page.locator('canvas').boundingBox();
  const open = (n) => page.evaluate(([x, y]) => document.elementFromPoint(x, y) === document.querySelector('canvas'), [rect.x + n.x, rect.y + n.y]);
  const alone = (n, others) => others.every((o) => o.id === n.id || Math.hypot(o.x - n.x, o.y - n.y) > 24);
  const near = s.nodes.filter((n) => n.front), far = s.nodes.filter((n) => !n.front);
  let back = null;
  for (const n of far) if (alone(n, near) && alone(n, far) && n.x > 24 && n.y > 24 && n.x < s.width - 24 && n.y < s.height - 24 && await open(n)) { back = n; break; }
  assert.ok(back, 'no isolated far-side node to probe');
  const inspected = page.locator('.cm-inspector-copy');
  await page.mouse.move(rect.x + back.x, rect.y + back.y);
  await frames(page);
  assert.equal(await inspected.count(), 0, `hovering the far-side node ${back.id} inspected it`);
  await page.mouse.click(rect.x + back.x, rect.y + back.y);
  await frames(page);
  assert.equal(await inspected.count(), 0, `clicking the far-side node ${back.id} selected it`);
  let front = null;
  for (const n of near) if (alone(n, near) && alone(n, far) && n.x > 24 && n.y > 24 && n.x < s.width - 24 && n.y < s.height - 24 && await open(n)) { front = n; break; }
  assert.ok(front, 'no isolated near-side node to probe');
  await page.mouse.click(rect.x + front.x, rect.y + front.y);
  await page.waitForSelector('.cm-inspector-copy', { timeout: 2000 });
  assert.equal(await page.locator('.cm-inspector-copy span').innerText(), nodes.find((n) => n.id === front.id).path, 'clicking a near-side node selected another');
}

async function fresh(page) {
  await page.reload();
  await page.waitForSelector('canvas', { timeout: 5000 });
  await page.waitForTimeout(300);
}

async function camera(page, name) {
  if (!name.startsWith('fit-') && name !== 'scene-graph.html') return;
  await fresh(page);
  await fitScenario(page);
  if (name !== 'scene-graph.html') return;
  await fresh(page);
  await filterScenario(page);
  await fresh(page);
  await globeScenario(page);
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
        await camera(page, path.basename(file));
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
