const { test, expect } = require('@playwright/test');

test('capture blog screenshots', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1200 });

  await page.goto('http://localhost:8080');
  
  await page.waitForSelector('.main-grid');
  await page.waitForTimeout(5000); // let data load

  await page.screenshot({ path: 'images/vortex-full.png' });

  const storageAnalysis = page.locator('.panel-block').filter({ hasText: 'STORAGE ANALYSIS' });
  await storageAnalysis.screenshot({ path: 'images/vortex-storage-analysis.png' });

  const hierarchyPanel = page.locator('.panel-block').filter({ hasText: 'HIERARCHY' });
  await hierarchyPanel.screenshot({ path: 'images/vortex-hierarchy.png' });

  const pageMap = page.locator('.center-panel');
  await pageMap.screenshot({ path: 'images/vortex-pagemap.png' });

  // Click on a block in the page map
  const blocks = page.locator('.page-cell');
  if (await blocks.count() > 0) {
    await blocks.first().click();
    await page.waitForSelector('.block-inspector');
    await page.waitForTimeout(2000);
    await page.locator('.block-inspector').screenshot({ path: 'images/vortex-page-inspector.png' });
  }

  // Switch to financial
  const financialTab = page.locator('.tab', { hasText: 'FINANCIAL_TICKS' });
  if (await financialTab.count() > 0) {
    await financialTab.click();
    await page.waitForTimeout(2000);
    await page.screenshot({ path: 'images/vortex-finance.png' });
  }
});
