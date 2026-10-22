import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';

export const root = resolve(import.meta.dirname, '../../../..');
export const fixtureConfig = process.env.PAB_ADMIN_FIXTURE_CONFIG;

let cached: { config: string; container: string; database: string; origin: string; cert: string; executable: string } | undefined;

export function isolatedFixture() {
  if (!fixtureConfig) throw new Error('Select the task-owned HTTPS fixture with PAB_ADMIN_FIXTURE_CONFIG');
  cached ??= JSON.parse(execFileSync('python', [resolve(root, 'scripts/admin_console_fixture_info.py'), '--config', fixtureConfig], { encoding: 'utf8', windowsHide: true }));
  return cached!;
}

export function screenshotPath(name: string, legacyDirectory: string) {
  return resolve(process.env.PAB_WEB_ARTIFACT_DIR ?? resolve(root, legacyDirectory), name);
}
