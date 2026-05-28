const { test, expect } = require('@playwright/test');

test('debug - inspect DOM after 5s', async ({ page }) => {
  await page.goto('http://localhost:4200');

  // Log all console messages
  page.on('console', msg => console.log('BROWSER:', msg.type(), msg.text()));
  page.on('pageerror', err => console.log('PAGE ERROR:', err.message));

  // Wait for connection
  await page.locator('.conn-live').waitFor({ timeout: 10000 });

  await page.waitForTimeout(8000);

  // Take screenshot
  await page.screenshot({ path: '/tmp/vortex-debug.png', fullPage: true });

  // Log DOM structure
  const html = await page.content();
  console.log('TOPBAR HTML:', (await page.locator('.topbar').innerHTML()).slice(0, 2000));

  // Check for tabs
  const tabs = await page.locator('.tab').count();
  console.log('Tab count:', tabs);

  const tabTexts = await page.locator('.tab').allTextContents();
  console.log('Tab texts:', tabTexts);

  // Check for no-tables span
  const noTables = await page.locator('.no-tables').count();
  console.log('.no-tables count:', noTables);
});
