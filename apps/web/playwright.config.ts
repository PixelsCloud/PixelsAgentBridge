import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './e2e', fullyParallel: false, workers: 1, timeout: 30000,
  use: { baseURL: process.env.PAB_WEB_TEST_URL ?? 'https://localhost:38443', ignoreHTTPSErrors: true, channel: 'chrome', locale: 'en-US', trace: 'retain-on-failure' },
  reporter: [['list'], ['html', { open: 'never' }]],
});
