import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  testMatch: "execution-ui.spec.ts",
  workers: 1,
  use: { browserName: "chromium", channel: process.platform === "win32" ? "msedge" : undefined, headless: true, viewport: { width: 1100, height: 950 } },
  webServer: { command: "npm run dev -- --host 127.0.0.1 --port 1429 --strictPort", url: "http://127.0.0.1:1429/tests/execution-ui.html", reuseExistingServer: false },
});
