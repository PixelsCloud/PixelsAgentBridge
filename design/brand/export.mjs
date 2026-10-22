/** Export the existing Pixels mark without a backing plate. Run from any cwd. */
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const require = createRequire(path.join(root, 'apps/web/package.json'));
const { chromium } = require('@playwright/test');
const svg = await fs.readFile(path.join(root, 'design/brand/pixels-mark.svg'), 'utf8');
await fs.writeFile(path.join(root, 'apps/web/src/brand.svg'), svg);

const browser = await chromium.launch({ channel: process.env.PAB_BRAND_BROWSER ?? 'chrome', headless: true });
try {
  for (const size of [512, 1024]) {
    const page = await browser.newPage({ viewport: { width: size, height: size }, deviceScaleFactor: 1 });
    await page.setContent(`<html><head><style>html,body{margin:0;background:transparent}svg{display:block;width:100vw;height:100vh}</style></head><body>${svg}</body></html>`);
    const png = await page.screenshot({ omitBackground: true });
    await fs.writeFile(path.join(root, `design/brand/pixels-mark-${size}.png`), png);
    if (size === 512) {
      await fs.writeFile(path.join(root, 'apps/android/app/src/main/res/drawable-nodpi/brand.png'), png);
    }
    await page.close();
  }
} finally {
  await browser.close();
}
console.log('Exported transparent SVG for Web and PNG for Android (512 / 1024 px).');
