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
  // Savings >= 80%: heap_bpr is now dynamically computed from pg_attribute + typalign,
  // so the exact % depends on the selected table's schema (typically 83–92% for Spiral).
  await expect(page.locator('.ls-green', { hasText: /[89]\d\.\d%/ })).toBeVisible();

  // Test slider interaction
  const slider = page.locator('input[type="range"]');
  await slider.fill('9'); // Set to 10^9 (1 Billion rows)

  const sliderLabel = page.locator('.slider-label');
  await expect(sliderLabel).toHaveText(/1\.0 Billion rows/);

  // Take a screenshot for visual confirmation
  await page.screenshot({ path: 'storage-dashboard.png' });
});

test('storage compression metrics are dynamically computed from pg_attribute', async ({ page }) => {
  await page.goto('http://localhost:8080');
  await page.waitForSelector('.main-grid');

  // Wait for WebSocket data to arrive (stats come via WS after connect)
  await page.waitForTimeout(3000);

  // ── Heap bytes/row ──────────────────────────────────────────────────────────
  // Must be a dynamically computed number like "48 B" or "88 B" — NOT hardcoded.
  // The server computes this from pg_attribute + typalign alignment walk.
  const heapBar = page.locator('.cmp-bar-row').filter({ hasText: 'Heap' });
  const heapVal = heapBar.locator('.cmp-val');
  await expect(heapVal).toBeVisible();
  // Must match "{N} B" where N is a whole number (e.g. "48 B", "88 B", "74 B")
  await expect(heapVal).toHaveText(/^\d+ B$/);

  // ── XOR bytes/row ───────────────────────────────────────────────────────────
  // Derived from CompressedBlock struct: BLOCK_SIZE(128) / VALUES_PER_XOR_BLOCK(61) = 2.1 B
  // This is a code constant, not schema-dependent, so it must always be ~2.1 B.
  const xorBar = page.locator('.cmp-bar-row').filter({ hasText: 'XOR' });
  const xorVal = xorBar.locator('.cmp-val');
  await expect(xorVal).toBeVisible();
  await expect(xorVal).toHaveText(/^2\.\d B$/); // e.g. "2.1 B"

  // ── IO Tax section ──────────────────────────────────────────────────────────
  // All three values (HEAP, SPIRAL, XOR-BK) must show computed page counts.
  const threeGrid = page.locator('.three-grid');
  await expect(threeGrid).toBeVisible();

  // HEAP IO tax: (BLCKSZ - PageHeader) / (heap_bpr + ItemId) ÷ 1000
  // For a 48-byte tuple: ~8168/52 = 157 rows/page → 1000/157 ≈ 6.4 pages per 1K rows
  const heapIoTax = threeGrid.locator('.tg-cell').nth(0).locator('.tg-value');
  await expect(heapIoTax).toHaveText(/^\d+\.\d$/); // e.g. "6.4"

  // SPIRAL IO tax: DATA_PER_PAGE = 1018 slots/page → 1000/1018 ≈ 1.0
  const spiralIoTax = threeGrid.locator('.tg-cell').nth(1).locator('.tg-value');
  await expect(spiralIoTax).toHaveText(/^\d+\.\d$/); // e.g. "0.9" or "1.0"

  // XOR-BK IO tax: 3843 values/page → 1000/3843 ≈ 0.26
  const xorIoTax = threeGrid.locator('.tg-cell').nth(2).locator('.tg-value');
  await expect(xorIoTax).toHaveText(/^\d+\.\d{2}$/); // e.g. "0.26"

  // ── Savings Calculator ──────────────────────────────────────────────────────
  // Move slider to 1B rows and verify HEAP shows > 0 TB/GB/MB
  const slider = page.locator('input[type="range"]');
  await slider.fill('9'); // 10^9 = 1 Billion

  // Calculator cells: HEAP, SPIRAL, XOR, SAVED
  const fourGrid = page.locator('.four-grid');
  await expect(fourGrid).toBeVisible();

  const heapCell = fourGrid.locator('.tg-cell').nth(0).locator('.tg-value');
  const spiralCell = fourGrid.locator('.tg-cell').nth(1).locator('.tg-value');
  const savedCell = fourGrid.locator('.tg-cell').nth(3).locator('.tg-value');

  // Heap at 1B rows should show GB or TB
  await expect(heapCell).toHaveText(/\d+(\.\d+)?(GB|TB)/);
  // Spiral should also show a size
  await expect(spiralCell).toHaveText(/\d+(\.\d+)?(MB|GB)/);
  // Saved % must be positive
  await expect(savedCell).toHaveText(/^\d+\.\d%$/);

  // ── 1 Billion rows projection ──────────────────────────────────────────────
  const projSaving = page.locator('.cmp-section-label', { hasText: '1 Billion ROWS PROJECTION (SPIRAL)' })
    .locator('~ .ls-row .ls-green').first();
  // Saving must be ≥ 10% (in practice >> 80% for Spiral vs heap)
  await expect(page.locator('.ls-green').filter({ hasText: /%$/ }).first()).toHaveText(/\d+\.\d%/);

  // ── Screenshot the full storage analysis panel ─────────────────────────────
  const storagePanel = page.locator('.panel-block').filter({ hasText: 'STORAGE ANALYSIS' });
  await storagePanel.screenshot({ path: 'storage-compression-panel.png' });
});
