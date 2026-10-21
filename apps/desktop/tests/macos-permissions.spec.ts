import { test, expect } from "@playwright/test";
const url = "http://127.0.0.1:1429/tests/macos-permissions.html";
const app = (page: any) => page.getByRole("region", { name: "Pixels Agent Bridge", exact: true });
const service = (page: any) => page.getByRole("region", { name: "pab-executor", exact: true });
async function openAddGuide(region: any, page: any) {
  await region.getByRole("button", { name: /More options/ }).click();
  await page.getByRole("menuitem", { name: "Program not listed in System Settings?", exact: true }).click();
}

test("compact statuses show normal permissions without request buttons and identify the service", async ({ page }) => {
  await page.goto(url + "?allowed");
  for (const name of ["Screen Recording", "Accessibility"]) {
    const row = app(page).getByRole("group", { name, exact: true });
    await expect(row).toContainText("Granted");
    await expect(row.getByRole("button")).toHaveCount(0);
  }
  await expect(service(page)).toContainText("also our program");
  await expect(page.getByText("/Applications/Pixels Agent Bridge.app", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => /^(request_|open_|restart_)/.test(c.command)).length)).toBe(0);
});

test("denied permissions open system settings directly; only observed grants become normal", async ({ page }) => {
  await page.goto(url);
  const row = app(page).getByRole("group", { name: "Screen Recording", exact: true });
  await row.getByRole("button", { name: "Request access" }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "open_macos_permission_settings" && c.args.permission === "screen"))).toBe(true);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(row.getByRole("button")).toBeEnabled();
  await expect(row).toContainText("Not granted");
  await page.evaluate(() => { (window as any).fixture.accept = true; });
  await row.getByRole("button", { name: "Request access" }).click();
  await expect(row).toContainText("Granted");
  await expect(row.getByRole("button")).toHaveCount(0);
});

test("disk requests open settings only; Finder opens only after an explicit reveal", async ({ page }) => {
  await page.goto(url);
  for (const [region, target, path] of [[app(page), "desktop", "/Applications/Pixels Agent Bridge.app"], [service(page), "executor", "/Library/Application Support/PixelsAgentBridge/pab-executor"]] as const) {
    await page.evaluate(() => { (window as any).fixture.calls.length = 0; });
    await region.getByRole("group", { name: "File access", exact: true }).getByRole("button").click();
    await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => ["reveal_macos_permission_target", "open_macos_permission_settings"].includes(c.command)))).toEqual([
      { command: "open_macos_permission_settings", args: { permission: "full_disk" } },
    ]);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByRole("status")).toContainText(`enable: ${target === "desktop" ? "Pixels Agent Bridge" : "pab-executor"}`);
    await expect(region.getByRole("group", { name: "File access", exact: true }).getByRole("button")).toBeEnabled();
    await openAddGuide(region, page);
    const dialog = page.getByRole("dialog");
    await expect(dialog.locator("code")).toHaveText(path);
    expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "reveal_macos_permission_target"))).toBe(false);
    await dialog.getByRole("button", { name: "Reveal in Finder", exact: true }).click();
    expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "reveal_macos_permission_target"))).toEqual([{ command: "reveal_macos_permission_target", args: { target } }]);
    await dialog.getByRole("button", { name: "Copy path", exact: true }).click();
    expect(await page.evaluate(() => (window as any).fixture.clipboard)).toBe(path);
    await dialog.getByRole("button", { name: "Done", exact: true }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(region.getByRole("group", { name: "File access", exact: true })).toContainText("Access restricted");
  }
});

test("missing service files disable the corresponding disk request", async ({ page }) => {
  await page.goto(url + "?missing-service");
  await expect(service(page).getByRole("button", { name: "Set permissions" })).toBeDisabled();
  await expect(app(page).getByRole("group", { name: "File access", exact: true }).getByRole("button")).toBeEnabled();
});

test("settings errors survive permission polling and never fall back to Finder", async ({ page }) => {
  await page.goto(url);
  await page.evaluate(() => { (window as any).fixture.fail = "open_macos_permission_settings"; });
  await service(page).getByRole("button", { name: "Set permissions" }).click();
  await expect(page.getByRole("alert")).toContainText("Native action failed");
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "macos_permissions").length)).toBeGreaterThan(3);
  await expect(page.getByRole("alert")).toContainText("Native action failed");
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "reveal_macos_permission_target"))).toBe(false);
});

test("refresh failures remain visible without claiming success", async ({ page }) => {
  await page.goto(url);
  await expect(page.getByRole("button", { name: "Refresh status" })).toBeVisible();
  await page.evaluate(() => { (window as any).fixture.fail = "macos_permissions"; });
  await page.getByRole("button", { name: "Refresh status" }).click();
  await expect(page.getByRole("alert")).toContainText("Could not confirm");
  await expect(page.getByRole("status")).toHaveCount(0);
});

test("repair stays inside the guide and restarts require a separate confirmation", async ({ page }) => {
  await page.goto(url);
  await openAddGuide(service(page), page);
  const guide = page.getByRole("dialog").filter({ hasText: "Enable this program’s switch" });
  await guide.locator("summary").click();
  for (const [label, command] of [["Restart app", "restart_macos_permission_processes"], ["Restart background service", "restart_macos_executor_for_permissions"]]) {
    await guide.getByRole("button", { name: label, exact: true }).click();
    let confirm = page.getByRole("dialog").filter({ hasText: "This interrupts" });
    await confirm.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(confirm).toHaveCount(0);
    expect(await page.evaluate(command => (window as any).fixture.calls.filter((c: any) => c.command === command).length, command)).toBe(0);
    await guide.getByRole("button", { name: label, exact: true }).click();
    confirm = page.getByRole("dialog").filter({ hasText: "This interrupts" });
    await confirm.getByRole("button", { name: /^(loading )?Restart$/ }).click();
    await expect(confirm).toHaveCount(0);
    expect(await page.evaluate(command => (window as any).fixture.calls.filter((c: any) => c.command === command).length, command)).toBe(1);
  }
});

for (const language of ["zh-CN", "en"]) {
  for (const style of ["light", "dark"]) {
    test(`compact guide fits without horizontal overflow in ${language}/${style}`, async ({ page }) => {
      await page.setViewportSize({ width: 950, height: 640 });
      await page.goto(`${url}?language=${language}&theme=${style}&allowed`);
      await expect(page.locator(".macos-permission-program")).toHaveCount(2);
      await expect(page.locator(".macos-permission-row").last().getByRole("button")).toBeEnabled();
      expect(await page.locator(".settings-detail").evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
      expect(await page.locator(".settings-detail").evaluate(el => el.scrollHeight <= el.clientHeight + 1)).toBe(true);
      if (language === "zh-CN") await page.screenshot({ path: `../../.build/remote-terminal-rollout/mac-guide-${style}.png` });
    });
  }
}
test("Windows has no macOS permission page", async ({ page }) => {
  await page.goto(url + "?windows");
  await expect(page.getByRole("menuitem", { name: "Preferences", exact: true })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "macOS permissions", exact: true })).toHaveCount(0);
});

test("file access reflects each program independently and a later denied check revokes normal", async ({ page }) => {
  await page.goto(url + "?disk-allowed");
  const appRow = app(page).getByRole("group", { name: "File access", exact: true });
  const serviceRow = service(page).getByRole("group", { name: "File access", exact: true });
  await expect(appRow).toContainText("Normal");
  await expect(serviceRow).toContainText("Normal");
  await expect(serviceRow.getByRole("button")).toHaveCount(0);
  await page.evaluate(() => { (window as any).fixture.disk.executor.state = "restricted"; });
  await page.getByRole("button", { name: "Refresh status" }).click();
  await expect(serviceRow).toContainText("Access restricted");
  await expect(appRow).toContainText("Normal");
});

test("the returning program is checked again and opening settings never claims a grant", async ({ page }) => {
  await page.goto(url);
  const row = service(page).getByRole("group", { name: "File access", exact: true });
  await row.getByRole("button", { name: "Set permissions" }).click();
  await expect(page.getByRole("status")).toContainText("enable: pab-executor");
  await expect(row).toContainText("Access restricted");
  await page.evaluate(() => { (window as any).fixture.disk.executor.state = "normal"; window.dispatchEvent(new Event("focus")); });
  await expect(row).toContainText("Granted");
  await expect(app(page).getByRole("group", { name: "File access", exact: true })).toContainText("Access restricted");
});

test("timeout and unavailable service are failures, not grants or authorization denials", async ({ page }) => {
  await page.goto(url);
  const row = service(page).getByRole("group", { name: "File access", exact: true });
  await expect(row).toContainText("Access restricted");
  await page.evaluate(() => { const f = (window as any).fixture; f.disk.executor.state = "failed"; f.disk.executor.folders[0].state = "failed"; f.disk.executor.folders[0].reason = "timeout"; });
  await page.getByRole("button", { name: "Refresh status" }).click();
  await expect(row).toContainText("Check failed");
  await row.locator(".ant-tag").hover();
  await expect(page.getByRole("tooltip")).toContainText("timed out");
  await page.evaluate(() => { (window as any).fixture.fail = "macos_file_access_check"; });
  await page.getByRole("button", { name: "Refresh status" }).click();
  await expect(row).toContainText("Check failed");
});

test("a pending file check leaves the page responsive and queues one recheck on return", async ({ page }) => {
  await page.goto(url);
  const row = service(page).getByRole("group", { name: "File access", exact: true });
  await expect(row).toContainText("Access restricted");
  await row.getByRole("button", { name: "Set permissions" }).click();
  await expect(row.getByRole("button")).toBeEnabled();
  await page.evaluate(() => { const f = (window as any).fixture; f.holdDisk = "executor"; window.dispatchEvent(new Event("focus")); });
  await expect(row).toContainText("Checking");
  await expect(page.getByRole("menuitem", { name: "Preferences", exact: true })).toBeEnabled();
  await page.evaluate(() => { const f = (window as any).fixture; f.disk.executor.state = "normal"; f.holdDisk = ""; f.resolveDisk(); });
  await expect(row).toContainText("Granted");
});
