import { test, expect, type Page } from "@playwright/test";

async function open(page: Page) {
  await page.goto("http://127.0.0.1:1429/tests/desktop-terminal.html");
  await expect(page.getByRole("button", { name: "Open Shell" })).toBeDisabled();
  await page.getByRole("combobox", { name: "Execution account" }).click();
  await page.locator(".ant-select-dropdown:visible").getByText("Administrator · 2", { exact: true }).click();
  await page.getByRole("button", { name: "Open Shell" }).click();
  await expect(page.getByText("Connected (P2P)", { exact: true })).toBeVisible();
}
test("explicit identity, renderer consumption, resize and bounded input use only new APIs", async ({ page }) => {
  await open(page);
  await page.evaluate(() => { for (let i = 0; i < 500; i++) (window as any).fixture.output("中文 🌍 " + "X".repeat(240) + "\r\n"); });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_consume").at(-1)?.args.offset)).toBe(500 * (new TextEncoder().encode("中文 🌍 " + "X".repeat(240) + "\r\n").length));
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("echo hello"); await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_input").length)).toBeGreaterThan(0);
  await page.setViewportSize({ width: 900, height: 620 });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_resize").length)).toBeGreaterThan(0);
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command.startsWith("operator_terminal") || c.command === "operator_open_terminal"))).toBe(false);
  await page.screenshot({ path: "../../.build/desktop-terminal-ui.png" });
});
test("cancel closing keeps Shell, confirmed close ends only once, natural exit preserves reason", async ({ page }) => {
  await open(page);
  const ends = () => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_end").length);
  const before = await ends();
  await page.getByRole("button", { name: "End session", exact: true }).click();
  await page.getByRole("button", { name: "Keep using" }).click();
  expect(await ends()).toBe(before);
  await expect(page.getByText("Connected (P2P)", { exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).fixture.end());
  await expect(page.getByText(/Shell exited · 7/)).toBeVisible();
  await page.getByRole("button", { name: "Reopen" }).click();
  await expect(page.getByText("Connected (P2P)", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "End session", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "End session", exact: true }).click();
  await expect(page.getByText("Ended", { exact: true }).first()).toBeVisible();
  expect(await ends()).toBe(before + 2);
});
test("unconfirmed input is never replayed and close failure remains visible", async ({ page }) => {
  await open(page);
  await page.evaluate(() => { (window as any).fixture.failInput = true; });
  await page.locator(".xterm-helper-textarea").focus(); await page.keyboard.type("x");
  await expect(page.getByText("input result unconfirmed; not replayed", { exact: true })).toBeVisible();
  await page.waitForTimeout(200);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_input").length)).toBe(1);
  await page.evaluate(() => { (window as any).fixture.failClose = true; (window as any).fixture.closeWindow(); });
  await page.getByRole("dialog").getByRole("button", { name: "End session", exact: true }).click();
  await expect(page.getByText("worker termination not confirmed", { exact: true })).toBeVisible();
});
test("multi-line paste requires confirmation and oversized paste is refused", async ({ page }) => {
  await open(page);
  await page.evaluate(() => (window as any).fixture.paste("echo first\necho second"));
  await expect(page.getByRole("dialog")).toContainText("Multiple lines may execute immediately");
  await page.getByRole("button", { name: "Keep using" }).click();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_input").length)).toBe(0);
  await page.evaluate(() => (window as any).fixture.paste("X".repeat(262145)));
  await expect(page.getByText("A single paste is limited to 256 KiB.")).toBeVisible();
});
test("a delayed consumption ACK cannot stall or act on a reopened Shell", async ({ page }) => {
  await open(page);
  await page.evaluate(() => { const f = (window as any).fixture; f.delayConsume = true; f.output("old-long-output"); });
  await expect.poll(() => page.evaluate(() => !!(window as any).fixture.releaseConsume)).toBe(true);
  await page.evaluate(() => (window as any).fixture.end());
  await page.getByRole("button", { name: "Reopen" }).click();
  await expect(page.getByText("Connected (P2P)", { exact: true })).toBeVisible();
  await page.evaluate(() => { const f = (window as any).fixture; f.output("new"); f.releaseConsume(); });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_consume").at(-1)?.args.offset)).toBe(3);
  const calls = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_consume"));
  expect(calls.at(-1).args.sessionId).not.toBe(calls[0].args.sessionId);
});

test("an output gap latches failure and closes the owned Shell without accepting later input", async ({ page }) => {
  await open(page);
  const before = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_end").length);
  await page.evaluate(() => { const f = (window as any).fixture; f.raw([0,0,0,0,0,0,0,0,4,88]); f.output("later"); });
  await expect(page.getByText("terminal output gap or duplicate detected; no data was silently skipped", { exact: true })).toBeVisible();
  await page.locator(".xterm-helper-textarea").focus(); await page.keyboard.type("never");
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_input").length)).toBe(0);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_end").length)).toBe(before + 1);
});

test("confirmed native window closing uses the caller-bound backend command", async ({ page }) => {
  await open(page); await page.evaluate(() => (window as any).fixture.closeWindow());
  await page.getByRole("dialog").getByRole("button", { name: "End session", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "desktop_terminal_destroy"))).toBe(true);
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "plugin:window|destroy"))).toBe(false);
});

test("compact titlebar shows the path and keeps window controls independent of dragging and session cancellation", async ({ page }) => {
  await open(page);
  const header = page.locator(".desktop-terminal-header");
  await expect(header).toContainText("428 358 431");
  await expect(header).toContainText("P2P");
  await expect(page.getByText("powershell.exe", { exact: true })).toHaveCount(0);
  await expect(page.locator(".desktop-terminal-details")).toHaveCount(0);
  await expect(header).toHaveCSS("height", "42px");
  await header.getByRole("button", { name: "Minimize", exact: true }).click();
  await header.getByRole("button", { name: "Maximize / restore", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "plugin:window|toggle_maximize").length)).toBe(1);
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "plugin:window|minimize"))).toBe(true);
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "plugin:window|start_dragging"))).toBe(false);
  await header.getByText("Remote terminal", { exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "plugin:window|start_dragging").length)).toBe(1);
  await header.getByText("Remote terminal", { exact: true }).dblclick();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "plugin:window|toggle_maximize").length)).toBe(2);
  await header.getByRole("button", { name: "Close window", exact: true }).click();
  await page.getByRole("button", { name: "Keep using" }).click();
  await expect(page.getByRole("dialog")).toBeHidden();
  await expect(header.getByText("Connected (P2P)", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "desktop_terminal_destroy"))).toBe(false);
  await page.evaluate(() => (window as any).fixture.output("PS C:\\Users\\Administrator> Write-Output 'Pixels remote terminal'\r\nPixels remote terminal\r\nPS C:\\Users\\Administrator> "));
  await page.screenshot({ path: "../../.build/desktop-terminal-titlebar-1100.png" });
  await page.setViewportSize({ width: 640, height: 400 });
  await expect(header).toContainText("P2P");
  const controls = await header.locator(".window-controls").boundingBox();
  const identity = await header.locator(".desktop-terminal-title").boundingBox();
  expect(controls!.x + controls!.width).toBeLessThanOrEqual(640);
  expect(identity!.x + identity!.width).toBeLessThanOrEqual(controls!.x);
  expect(await header.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
  expect(await header.locator(".desktop-terminal-title").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
  await page.screenshot({ path: "../../.build/desktop-terminal-titlebar-640.png" });
  await header.getByRole("button", { name: "Close window", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "End session", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "desktop_terminal_destroy"))).toBe(true);
});

for (const theme of ["light", "dark"]) {
  test(`Chinese titlebar fits the minimum window in ${theme} mode`, async ({ page }) => {
    await page.setViewportSize({ width: 640, height: 400 });
    await page.goto(`http://127.0.0.1:1429/tests/desktop-terminal.html?language=zh-CN&theme=${theme}`);
    await page.getByRole("combobox", { name: "执行账户" }).click();
    await page.locator(".ant-select-dropdown:visible").getByText("Administrator · 2", { exact: true }).click();
    await page.getByRole("button", { name: "打开 Shell" }).click();
    const header = page.locator(".desktop-terminal-header");
    await expect(header.getByText("已连接（P2P）", { exact: true })).toBeVisible();
    await expect(header).toContainText("P2P");
    expect(await header.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
    expect(await header.locator(".desktop-terminal-title").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
    await page.evaluate(() => (window as any).fixture.output("PS C:\\Users\\Administrator> Write-Output '中文测试'\r\n中文测试\r\nPS C:\\Users\\Administrator> "));
    await page.screenshot({ path: `../../.build/desktop-terminal-titlebar-zh-${theme}.png` });
  });
}

for (const mode of ["console", "system"]) {
  test(`the ${mode} menu mode opens its Shell automatically without an account chooser`, async ({ page }) => {
    await page.setViewportSize({ width: 640, height: 400 });
    await page.goto(`http://127.0.0.1:1429/tests/desktop-terminal.html?launch=${mode}`);
    await expect(page.getByText("Connected (P2P)", { exact: true })).toBeVisible();
    await expect(page.getByRole("combobox", { name: "Execution account" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Open Shell", exact: true })).toHaveCount(0);
    const starts = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_start"));
    expect(starts).toHaveLength(1);
    expect(starts[0].args.execution).toEqual(mode === "system" ? { mode: "service" } : { mode: "user", context_ref: "fixture-console" });
    expect(await page.locator(".desktop-terminal-title").evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
    if (mode === "system") await expect(page.locator(".desktop-terminal-header").getByText("System", { exact: true })).toBeVisible();
    await page.evaluate(() => (window as any).fixture.end());
    await page.getByRole("button", { name: "Reopen", exact: true }).click();
    await expect(page.getByText("Connected (P2P)", { exact: true })).toBeVisible();
    const selections = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_selection"));
    expect(selections).toHaveLength(2);
  });
}

test("an unavailable console does not start a service Shell; retry obtains a fresh context", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/desktop-terminal.html?launch=console&no-console");
  await expect(page.getByText("no active local console login was found", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "desktop_terminal_start"))).toBe(false);
  await expect(page.getByRole("combobox", { name: "Execution account" })).toHaveCount(0);
  await page.evaluate(() => { (window as any).fixture.failSelection = false; });
  await page.getByRole("button", { name: "Reopen", exact: true }).click();
  await expect(page.getByText("Connected (P2P)", { exact: true })).toBeVisible();
  const starts = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_terminal_start"));
  expect(starts).toHaveLength(1);
  expect(starts[0].args.execution.mode).toBe("user");
});

test("Mac terminals explain remote file privacy prompts while input remains usable", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/desktop-terminal.html?launch=console&os=macos&language=zh-CN");
  await expect(page.getByText("已连接（P2P）", { exact: true })).toBeVisible();
  const notice = page.getByRole("alert");
  await expect(notice).toContainText("macOS 文件访问权限");
  await expect(notice).toContainText("/Library/Application Support/PixelsAgentBridge/pab-executor");
  await expect(notice).toContainText("远端 Mac");
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type("pwd");
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "desktop_terminal_input"))).toBe(true);
  await notice.getByRole("button", { name: "Close" }).click();
  await expect(notice).toHaveCount(0);
  await page.evaluate(() => (window as any).fixture.end());
  await page.getByRole("button", { name: "重新打开", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("macOS 文件访问权限");
});

for (const os of ["windows", "linux"]) {
  test(`${os} does not receive macOS-specific permission guidance`, async ({ page }) => {
    await page.goto(`http://127.0.0.1:1429/tests/desktop-terminal.html?launch=system&os=${os}`);
    await expect(page.getByText("Connected (P2P)", { exact: true })).toBeVisible();
    await expect(page.getByText("macOS file access", { exact: true })).toHaveCount(0);
  });
}


test("Android unsupported privileged terminal has a plain feature message", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/desktop-terminal.html?launch=system&android-system=1&language=zh-CN");
  await expect(page.getByText("此 Android 设备暂无此功能。")).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c:any) => c.command === "desktop_terminal_start"))).toBe(false);
});

test("terminal connected status follows the actual connection path", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/desktop-terminal.html?launch=console");
  await expect(page.locator(".desktop-terminal-title .ant-tag")).toHaveText("Connected (P2P)");
  await page.evaluate(() => { (window as any).fixture.connectionPath = "Relay"; });
  await expect(page.locator(".desktop-terminal-title .ant-tag")).toHaveText("Connected (Relay)");
});
