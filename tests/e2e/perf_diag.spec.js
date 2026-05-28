// @ts-check
/**
 * Diagnostic: capture PERF console logs to understand where time is spent.
 */
const { test } = require('@playwright/test');
const fs = require('fs');

const UI_URL = 'http://localhost:4200';

test('perf diagnostic - capture timing logs', async ({ page }) => {
  const consoleLogs = [];
  const wsEvents = [];

  page.on('console', msg => {
    const text = msg.text();
    consoleLogs.push({ type: msg.type(), text });
    if (text.includes('PERF') || text.includes('Connecting') || text.includes('VortexEvent')) {
      console.log(`[BROWSER] ${msg.type()}: ${text}`);
    }
  });

  page.on('pageerror', err => {
    console.log(`[PAGE ERROR]: ${err.message}`);
    consoleLogs.push({ type: 'pageerror', text: err.message });
  });

  page.on('websocket', ws => {
    let count = 0;
    ws.on('framereceived', frame => {
      count++;
      try {
        const ev = JSON.parse(frame.payload);
        wsEvents.push(ev.type);
        if (count <= 20 || count % 50 === 0) {
          console.log(`WS event #${count}: ${ev.type}`);
        }
      } catch { }
    });
  });

  await page.goto(UI_URL);

  // Wait for WS connection
  try {
    await page.locator('.conn-live').waitFor({ timeout: 12000 });
    console.log('CONN-LIVE visible');
  } catch (e) {
    console.log('CONN-LIVE NOT visible within 12s');
    return;
  }

  console.log('Starting 8s observation...');

  // Observe for 8 seconds with periodic checks
  for (let i = 0; i < 8; i++) {
    await page.waitForTimeout(1000);
    try {
      const tabCount = await page.evaluate(() => document.querySelectorAll('.tab').length, null, { timeout: 500 });
      console.log(`t=${i+1}s: tab count = ${tabCount}, ws events so far = ${wsEvents.length}`);
    } catch (e) {
      console.log(`t=${i+1}s: BROWSER FROZEN (${e.message.slice(0, 50)})`);
    }
  }

  const typeCounts = wsEvents.reduce((acc, t) => { acc[t] = (acc[t] || 0) + 1; return acc; }, {});
  const perfLogs = consoleLogs.filter(l => l.text.includes('PERF')).map(l => l.text);

  const report = [
    'WS event totals: ' + JSON.stringify(typeCounts),
    'Total WS events: ' + wsEvents.length,
    'PERF logs:',
    ...perfLogs,
    'All console:',
    ...consoleLogs.map(l => `  [${l.type}] ${l.text}`),
  ].join('\n');

  fs.writeFileSync('/tmp/vortex-perf-diag.txt', report);
  console.log('Diagnostic written to /tmp/vortex-perf-diag.txt');
}, { timeout: 60000 });
