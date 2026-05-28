// @ts-check
const { defineConfig, devices } = require('@playwright/test');

module.exports = defineConfig({
  testDir: './tests/e2e',
  timeout: 30000,
  expect: { timeout: 5000 },
  fullyParallel: false, // sequential: tests share live server state
  retries: 1,
  reporter: 'list',
  use: {
    baseURL: 'http://localhost:4200',
    trace: 'on-first-retry',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  // Assumes vortex-server (3010) and vortex-ui (4200) are already running
  // Run: DATABASE_URL=postgres://... PORT=3010 ./target/debug/vortex-server &
  //      cd vortex-ui/dist && python3 -m http.server 4200 &
});
