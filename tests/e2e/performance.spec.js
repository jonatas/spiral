// @ts-check
/**
 * Performance diagnostics: detect reactive event storms and UI blocking.
 *
 * Run with: npx playwright test tests/e2e/performance.spec.js --reporter=list
 */
const { test, expect } = require('@playwright/test');

const UI_URL = 'http://localhost:4200';
const SERVER_URL = 'http://localhost:3001';

/**
 * Intercept all fetch requests and count /api/slice calls over a window.
 * Returns a getter for the count.
 */
async function trackSliceRequests(page) {
  let sliceCount = 0;
  page.on('request', req => {
    if (req.url().includes('/api/slice')) sliceCount++;
  });
  return () => sliceCount;
}

/**
 * Intercept WebSocket frames and count by type.
 * Returns a getter for counts.
 */
async function trackWsEvents(page) {
  const counts = { StorageStats: 0, ChangelogUpdate: 0, SystemConfig: 0, unknown: 0 };

  page.on('websocket', ws => {
    ws.on('framereceived', frame => {
      try {
        const ev = JSON.parse(frame.payload);
        if (ev.type in counts) counts[ev.type]++;
        else counts.unknown++;
      } catch {
        counts.unknown++;
      }
    });
  });

  return () => ({ ...counts });
}

test.describe('Vortex UI — performance / responsiveness', () => {
  test('slice requests not fired on every StorageStats WS event', async ({ page }) => {
    const getSliceCount = await trackSliceRequests(page);
    const getWsEvents = await trackWsEvents(page);

    await page.goto(UI_URL);
    await page.locator('.conn-live').waitFor({ timeout: 10000 });

    // Wait for hierarchy and page map
    await page.locator('.tab').first().waitFor({ timeout: 12000 });
    await page.locator('.page-cell').first().waitFor({ timeout: 10000 });

    // Click first page — triggers one slice request
    await page.locator('.page-cell').first().click();
    await page.locator('.block-inspector').waitFor({ timeout: 5000 });

    const sliceAfterClick = getSliceCount();

    // Wait 12s — server polls every 5s, so we get >=2 StorageStats batches
    await page.waitForTimeout(12000);

    const wsFinal = getWsEvents();
    const sliceFinal = getSliceCount();
    const extraSlice = sliceFinal - sliceAfterClick;

    console.log('WS events:', JSON.stringify(wsFinal));
    console.log(`Slice requests: ${sliceAfterClick} after click, ${sliceFinal} total (+${extraSlice} extra)`);
    console.log(`StorageStats events: ${wsFinal.StorageStats}`);

    // Extra slice requests beyond the initial one should be 0 (or very few if block changes)
    // Before the fix: extraSlice ≈ wsFinal.StorageStats (one per event)
    // After the fix: extraSlice ≈ 0
    expect(extraSlice).toBeLessThanOrEqual(2);
  });

  test('tab click responds within 500ms even under WS load', async ({ page }) => {
    await page.goto(UI_URL);
    await page.locator('.conn-live').waitFor({ timeout: 10000 });
    await page.locator('.tab').first().waitFor({ timeout: 12000 });

    const tabs = page.locator('.tab');
    const tabCount = await tabs.count();
    if (tabCount < 2) {
      test.skip();
      return;
    }

    // Click second tab and measure response time
    const t0 = Date.now();
    await tabs.nth(1).click();

    // Tab should become active quickly
    await expect(tabs.nth(1)).toHaveClass(/tab-active/, { timeout: 500 });
    const elapsed = Date.now() - t0;

    console.log(`Tab switch latency: ${elapsed}ms`);
    expect(elapsed).toBeLessThan(500);
  });

  test('dirty page cells cap — dirty_page_nos does not grow unboundedly', async ({ page }) => {
    const getWsEvents = await trackWsEvents(page);

    await page.goto(UI_URL);
    await page.locator('.conn-live').waitFor({ timeout: 10000 });
    await page.locator('.tab').first().waitFor({ timeout: 12000 });

    // Wait for changelog events
    await page.waitForTimeout(15000);

    const wsFinal = getWsEvents();
    const changelogEvents = wsFinal.ChangelogUpdate;
    console.log(`ChangelogUpdate events: ${changelogEvents}`);

    // Dirty cells should be bounded — at most MAX_DIRTY in the page map
    const dirtyCells = await page.locator('.page-cell.dirty').count();
    console.log(`Dirty page cells rendered: ${dirtyCells}`);

    // If unbounded: dirtyCells grows with every changelog event
    // After fix: dirtyCells <= MAX_DIRTY (200)
    expect(dirtyCells).toBeLessThanOrEqual(200);
  });

  test('WS events processed without blocking main thread', async ({ page }) => {
    // Measure long tasks (JS >50ms) during WS activity using Performance API
    await page.goto(UI_URL);
    await page.locator('.conn-live').waitFor({ timeout: 10000 });
    await page.locator('.tab').first().waitFor({ timeout: 12000 });

    // Inject long task observer
    await page.evaluate(() => {
      window.__longTasks = [];
      const obs = new PerformanceObserver(list => {
        list.getEntries().forEach(e => window.__longTasks.push(e.duration));
      });
      obs.observe({ entryTypes: ['longtask'] });
    });

    // Wait for a full poll cycle (5s server + margin)
    await page.locator('.page-cell').first().waitFor({ timeout: 8000 });
    await page.locator('.page-cell').first().click();
    await page.waitForTimeout(10000);

    const longTasks = await page.evaluate(() => window.__longTasks || []);
    const maxDuration = longTasks.length > 0 ? Math.max(...longTasks) : 0;
    const totalLong = longTasks.length;

    console.log(`Long tasks (>50ms): ${totalLong}, max duration: ${maxDuration.toFixed(0)}ms`);
    console.log('Durations:', longTasks.map(d => d.toFixed(0) + 'ms').join(', '));

    // After fix: no long tasks from reactive storms
    // Before fix: may see 50-200ms blocks per StorageStats batch
    expect(totalLong).toBeLessThan(5);
  });
});
