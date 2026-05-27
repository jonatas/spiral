const { test, expect } = require('@playwright/test');

test('confirm storage dashboard is working', async ({ page }) => {
  await page.goto('http://localhost:8080');
  
  // Wait for the app to load
  await page.waitForSelector('.main-grid');

  // Check for the "STORAGE ANALYSIS" section
  const storageHdr = page.locator('.panel-hdr', { hasText: 'STORAGE ANALYSIS' });
  await expect(storageHdr).toBeVisible();

  // Check for "BYTES / ROW" bars
  await expect(page.locator('.cmp-section-label', { hasText: 'BYTES / ROW' })).toBeVisible();
  await expect(page.locator('.cmp-lbl', { hasText: 'Heap' })).toBeVisible();
  await expect(page.locator('.cmp-lbl', { hasText: 'Spiral' })).toBeVisible();
  await expect(page.locator('.cmp-lbl', { hasText: 'XOR' })).toBeVisible();

  // Check IO Tax
  await expect(page.locator('.cmp-section-label', { hasText: 'IO TAX — PAGES / 1K ROWS' })).toBeVisible();
  await expect(page.locator('.tg-label', { hasText: 'XOR-BK' })).toBeVisible();

  // Check Savings Calculator
  await expect(page.locator('.cmp-section-label', { hasText: 'SAVINGS CALCULATOR' })).toBeVisible();
  await expect(page.locator('.four-grid')).toBeVisible();
  // Using exact text to avoid matching 'XOR-BK'
  await expect(page.locator('.tg-label').filter({ hasText: /^XOR$/ })).toBeVisible();

  // Check 1 Billion ROWS PROJECTION
  await expect(page.locator('.cmp-section-label', { hasText: '1 Billion ROWS PROJECTION (SPIRAL)' })).toBeVisible();
  // Check that we have a high saving percentage (at least 90%)
  await expect(page.locator('.ls-green', { hasText: /9\d\.\d%/ })).toBeVisible();

  // Test slider interaction
  const slider = page.locator('input[type="range"]');
  await slider.fill('9'); // Set to 10^9 (1 Billion rows)
  
  const sliderLabel = page.locator('.slider-label');
  await expect(sliderLabel).toHaveText(/1\.0 Billion rows/);

  // Take a screenshot for visual confirmation
  await page.screenshot({ path: 'storage-dashboard.png' });
});
