// @ts-check
/**
 * Quick responsiveness check: page must remain interactive after WS events arrive.
 * If the reactive storm bug is present, this test hangs/fails.
 */
const { test, expect } = require('@playwright/test');

const UI_URL = 'http://localhost:4200';

test('page stays interactive under WS event load', async ({ page }) => {
  const errors = [];
  const wsEventCounts = { StorageStats: 0, ChangelogUpdate: 0, SystemConfig: 0 };

  page.on('pageerror', e => errors.push(e.message));
  page.on('websocket', ws => {
    ws.on('framereceived', frame => {
      try {
        const ev = JSON.parse(frame.payload);
        if (ev.type in wsEventCounts) wsEventCounts[ev.type]++;
      } catch { /* binary frame */ }
    });
  });

  await page.goto(UI_URL);

  // WS connects → LIVE indicator
  await expect(page.locator('.conn-live')).toBeVisible({ timeout: 12000 });

  // Wait one full server poll cycle (5s) for StorageStats to arrive
  await page.waitForTimeout(6000);

  console.log('WS events after 6s:', JSON.stringify(wsEventCounts));

  // Page must still respond to basic queries — if frozen, these will time out
  const tabCount = await page.locator('.tab').count();
  console.log('Tab count:', tabCount);

  // Screenshot must complete — freezes if JS loop is jammed
  await page.screenshot({ path: '/tmp/vortex-responsive-check.png' });

  // Tabs should be clickable
  if (tabCount >= 1) {
    await page.locator('.tab').first().click();
    await expect(page.locator('.tab').first()).toHaveClass(/tab-active/, { timeout: 500 });
  }

  expect(errors).toHaveLength(0);
  expect(tabCount).toBeGreaterThan(0);
});
