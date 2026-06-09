const { test, expect } = require('@playwright/test');

test('worker queueing and performance visualization', async ({ page }) => {
  await page.goto('http://localhost:4201');
  await page.waitForSelector('.main-grid');

  const workerStrip = page.locator('.worker-strip');
  await expect(workerStrip).toBeVisible();
  
  const enableBtn = page.locator('.hb-btn', { hasText: 'ENABLE HEARTBEAT' });
  if (await enableBtn.count() > 0) {
    await enableBtn.click();
  }
  
  // Need to wait long enough for data to accumulate and UI to redraw
  await page.waitForTimeout(6000);
  
  // Either we have no pending entries message, or the timeline renders
  const noEntriesMsg = page.getByText('no pending changelog entries');
  const clList = page.locator('svg').filter({ has: page.locator('rect') }); // Look for SVG with rects since changelog-timeline wraps everything

  // Wait for either one to be visible
  await expect(noEntriesMsg.or(clList)).toBeVisible();

  const isNoEntriesVisible = await noEntriesMsg.isVisible();
  if (!isNoEntriesVisible) {
      const count = await clList.locator('rect').count();
      console.log(`Pending changelog buckets: ${count}`);
      // Assert we don't have massive backlogs (should be less than 50)
      expect(count).toBeLessThan(50);
  } else {
      console.log('No pending changelog entries');
  }

  await page.screenshot({ path: 'images/vortex-worker-load.png' });
});
