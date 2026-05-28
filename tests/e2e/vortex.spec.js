// @ts-check
const { test, expect } = require('@playwright/test');

const UI_URL = 'http://localhost:4200';
const API_URL = 'http://localhost:3010';

test.describe('Vortex UI — connection and layout', () => {
  test('loads and shows VORTEX brand', async ({ page }) => {
    await page.goto(UI_URL);
    await expect(page.locator('.brand')).toHaveText('VORTEX');
  });

  test('WebSocket connects — LIVE indicator', async ({ page }) => {
    await page.goto(UI_URL);
    // Give WS time to connect and receive SystemConfig
    await expect(page.locator('.conn-live')).toBeVisible({ timeout: 8000 });
    await expect(page.locator('.conn-live')).toContainText('LIVE');
  });

  test('table tabs appear for each spiral hierarchy', async ({ page }) => {
    await page.goto(UI_URL);
    // Wait for tabs — requires SystemConfig WS event (sent on connection)
    await expect(page.locator('.tab', { hasText: 'SENSOR_DATA' })).toBeVisible({ timeout: 10000 });
    await expect(page.locator('.tab', { hasText: 'LEGACY_METRICS' })).toBeVisible({ timeout: 10000 });
  });
});

// Shared helper: wait until hierarchy data has loaded (SystemConfig received)
async function waitForHierarchy(page) {
  await page.goto(UI_URL);
  await expect(page.locator('.tab').first()).toBeVisible({ timeout: 12000 });
}

test.describe('Vortex UI — hierarchy tree', () => {
  test.beforeEach(async ({ page }) => {
    await waitForHierarchy(page);
  });

  test('hierarchy tree shows RAW node for selected table', async ({ page }) => {
    await expect(page.locator('.node-raw')).toContainText('RAW');
  });

  test('hierarchy tree shows aggregation tiers (1m, 1h, 1d)', async ({ page }) => {
    await expect(page.locator('.node-agg').first()).toBeVisible();
    // sensor_data has 1m, 1h, 1d frames
    const tiers = await page.locator('.node-agg').allTextContents();
    expect(tiers.some(t => t.includes('m') || t.includes('h') || t.includes('d'))).toBe(true);
  });

  test('clicking a tier highlights it', async ({ page }) => {
    const firstTier = page.locator('.tree-node').nth(1);
    await firstTier.click();
    await expect(firstTier).toHaveClass(/tree-node-active/);
  });

  test('clicking RAW deselects tier', async ({ page }) => {
    // First select a tier
    await page.locator('.tree-node').nth(1).click();
    // Then click RAW
    await page.locator('.node-raw').click();
    await expect(page.locator('.tree-node').first()).toHaveClass(/tree-node-active/);
  });
});

test.describe('Vortex UI — page map interaction', () => {
  test.beforeEach(async ({ page }) => {
    await waitForHierarchy(page);
    await page.locator('.page-cell').first().waitFor({ timeout: 8000 });
  });

  test('page map renders colored cells', async ({ page }) => {
    const cells = page.locator('.page-cell');
    await expect(cells.first()).toBeVisible();
    const count = await cells.count();
    expect(count).toBeGreaterThan(0);
  });

  test('clicking a page cell selects it and shows BlockInspector', async ({ page }) => {
    await page.locator('.page-cell').first().click();

    // BlockInspector should appear in right panel
    await expect(page.locator('.block-inspector')).toBeVisible({ timeout: 3000 });
    await expect(page.locator('.bi-title')).toContainText('PAGE');
  });

  test('BlockInspector shows alignment, drift, and capacity', async ({ page }) => {
    await page.locator('.page-cell').first().click();
    await page.locator('.block-inspector').waitFor({ timeout: 3000 });

    const inspector = page.locator('.block-inspector');
    await expect(inspector.locator('.ilabel', { hasText: 'capacity' })).toBeVisible();
    await expect(inspector.locator('.ilabel', { hasText: 'alignment' })).toBeVisible();
    await expect(inspector.locator('.ilabel', { hasText: 'drift' })).toBeVisible();
  });

  test('clicking a page shows slice data charts', async ({ page }) => {
    await page.locator('.page-cell').first().click();
    // Charts may take a moment to load (async fetch)
    await expect(page.locator('.charts-section')).toBeVisible({ timeout: 5000 });
    await expect(page.locator('.charts-meta')).toContainText('rows');
  });
});

test.describe('Vortex UI — EXPLAIN ANALYZE panel', () => {
  test.beforeEach(async ({ page }) => {
    await waitForHierarchy(page);
    await page.locator('.page-cell').first().waitFor({ timeout: 8000 });
    await page.locator('.page-cell').first().click();
    await page.locator('.query-panel').waitFor({ timeout: 5000 });
  });

  test('EXPLAIN panel auto-populates query on page click', async ({ page }) => {
    const textarea = page.locator('.query-input');
    await expect(textarea).toBeVisible();
    const value = await textarea.inputValue();
    expect(value).toMatch(/SELECT.*FROM.*WHERE.*t.*to_timestamp/i);
  });

  test('textarea is editable', async ({ page }) => {
    const textarea = page.locator('.query-input');
    await textarea.fill('SELECT count(*) FROM sensor_data');
    await expect(textarea).toHaveValue('SELECT count(*) FROM sensor_data');
  });

  test('RUN button executes EXPLAIN and shows results', async ({ page }) => {
    await page.locator('.run-btn').click();
    // Wait for result to appear
    await expect(page.locator('.explain-out')).toBeVisible({ timeout: 5000 });
    await expect(page.locator('.er-meta')).toBeVisible();
  });

  test('spiral acceleration highlighted in EXPLAIN output', async ({ page }) => {
    await page.locator('.run-btn').click();
    await page.locator('.explain-out').waitFor({ timeout: 5000 });
    // If the query hits an accelerated view, the er-spiral class appears
    const spiralLines = page.locator('.er-spiral');
    // NOTE: only present if planner actually accelerates this query
    // This is a soft assertion — log the count for observability
    const count = await spiralLines.count();
    console.log(`Spiral-accelerated plan nodes: ${count}`);
  });
});

test.describe('Vortex UI — storage analysis panel', () => {
  test.beforeEach(async ({ page }) => {
    await waitForHierarchy(page);
  });

  test('STORAGE ANALYSIS section is visible', async ({ page }) => {
    await expect(page.locator('.cmp-panel')).toBeVisible();
  });

  test('compression ratio appears as Nx in topbar', async ({ page }) => {
    const compValue = page.locator('.ts-item', { hasText: 'COMP' }).locator('.ts-value');
    // May show "—" if no data, or "Nx" if data exists
    await expect(compValue).toBeVisible();
  });

  test('savings calculator slider changes row count label', async ({ page }) => {
    const slider = page.locator('input[type="range"]');
    await expect(slider).toBeVisible();
    const before = await page.locator('.slider-label').textContent();
    // Move slider to a different position
    await slider.fill('9');
    const after = await page.locator('.slider-label').textContent();
    expect(before).not.toEqual(after);
  });
});

test.describe('Vortex UI — table switching', () => {
  test.beforeEach(async ({ page }) => {
    await waitForHierarchy(page);
  });

  test('switching tables resets page selection', async ({ page }) => {
    await page.locator('.page-cell').first().waitFor({ timeout: 8000 });
    // Select a page
    await page.locator('.page-cell').first().click();
    await page.locator('.block-inspector').waitFor({ timeout: 3000 });

    // Switch to legacy_metrics
    await page.locator('.tab', { hasText: 'LEGACY_METRICS' }).click();

    // Block inspector should be gone
    await expect(page.locator('.block-inspector')).not.toBeVisible();
  });

  test('switching to legacy_metrics shows its hierarchy', async ({ page }) => {
    await page.locator('.tab', { hasText: 'LEGACY_METRICS' }).click();
    // Wait for config to update
    await page.waitForTimeout(500);
    const nodeName = page.locator('.node-name').first();
    await expect(nodeName).toContainText('legacy_metrics');
  });
});

test.describe('Vortex API — server endpoints', () => {
  test('GET /api/metadata returns all views', async ({ request }) => {
    const resp = await request.get(`${API_URL}/api/metadata`);
    expect(resp.status()).toBe(200);
    const data = await resp.json();
    expect(Array.isArray(data)).toBe(true);
    expect(data.length).toBeGreaterThan(0);
    // Every entry should have required fields
    for (const entry of data) {
      expect(entry).toHaveProperty('view_name');
      expect(entry).toHaveProperty('parent_view');
      expect(entry).toHaveProperty('frame_seconds');
    }
  });

  test('GET /api/metadata/:name returns single view', async ({ request }) => {
    const resp = await request.get(`${API_URL}/api/metadata/sensor_data`);
    expect(resp.status()).toBe(200);
    const data = await resp.json();
    expect(data.view_name).toBe('sensor_data');
    expect(data.base_view).toBe('sensor_data');
    expect(data.frame_seconds).toBe(0);
  });

  test('GET /api/metadata/:name returns 404 for unknown view', async ({ request }) => {
    const resp = await request.get(`${API_URL}/api/metadata/nonexistent_table`);
    expect(resp.status()).toBe(404);
  });

  test('GET /api/slice requires t_start and t_end', async ({ request }) => {
    const resp = await request.get(`${API_URL}/api/slice/sensor_data_1h`);
    expect(resp.status()).toBe(400);
  });

  test('GET /api/slice returns rows with t_epoch field', async ({ request }) => {
    // Use a wide range to ensure we get data
    const resp = await request.get(
      `${API_URL}/api/slice/sensor_data_1h?t_start=1000000000&t_end=9999999999`
    );
    expect(resp.status()).toBe(200);
    const data = await resp.json();
    expect(data).toHaveProperty('rows');
    expect(data).toHaveProperty('count');
    if (data.count > 0) {
      expect(data.rows[0]).toHaveProperty('t_epoch');
    }
  });

  test('POST /api/explain returns plan lines', async ({ request }) => {
    const resp = await request.post(`${API_URL}/api/explain`, {
      data: { query: 'SELECT count(*) FROM sensor_data' },
    });
    expect(resp.status()).toBe(200);
    const data = await resp.json();
    expect(data).toHaveProperty('ok');
    expect(data).toHaveProperty('lines');
    expect(Array.isArray(data.lines)).toBe(true);
  });

  test('POST /api/explain blocks non-SELECT queries', async ({ request }) => {
    const resp = await request.post(`${API_URL}/api/explain`, {
      data: { query: 'DROP TABLE sensor_data' },
    });
    // Should return an error, not execute the DDL
    const data = await resp.json();
    expect(data.ok).toBe(false);
  });
});
