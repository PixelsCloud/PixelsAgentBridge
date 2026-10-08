import { test, expect } from "@playwright/test";

test("connection names identify agents while preserving separate sessions and raw details", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?connections=true");
  const names = page.locator(".ant-collapse-header strong");
  await expect(names).toHaveText(["Kimi Code", "Kimi Code", "Claude Code", "Codex", "DeepSeek Harness", "Cursor", "OpenCode", "Custom Agent", "Unknown"]);
  await expect(page.getByText("PID 1000", { exact: true })).toBeVisible();
  await expect(page.getByText("PID 1001", { exact: true })).toBeVisible();
  await page.locator(".ant-collapse-header").filter({ hasText: "Claude Code" }).click();
  await expect(page.getByText("claude-code · client-version", { exact: true })).toBeVisible();
});

test("refresh detects a newly available client without reopening settings", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?missing=true");
  const claude = page.getByTestId("agent-claude");
  await expect(claude.getByText("Client not detected", { exact: true })).toBeVisible();
  await page.evaluate(() => { (window as any).fixture.states.claude.detected = true; });
  await claude.getByRole("button", { name: "Refresh Claude Code" }).click();
  await expect(claude.getByRole("button", { name: "Enable", exact: true })).toBeEnabled();
});
const url = "http://127.0.0.1:1429/tests/agent-integrations-ui.html";

test("registration validates confirmation and uses the native HTTP registration command", async ({ page }) => {
  await page.goto(`${url}?app=true`);
  await page.locator(".sidebar-account").click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Create account", exact: true }).click();
  await dialog.locator("#account-login-username").fill("Pixels");
  await dialog.locator("#account-login-password").fill("fixture password");
  await dialog.locator("#account-confirm-password").fill("different");
  await expect(dialog.locator('button[type="submit"]')).toBeDisabled();
  await dialog.locator("#account-confirm-password").fill("fixture password");
  await dialog.locator('button[type="submit"]').click();
  await expect(page.locator(".page-me")).toContainText("Pixels");
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((x: any) => x.command === "operator_register_account").length)).toBe(1);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((x: any) => x.command === "operator_login_account").length)).toBe(0);
});

test("titlebar close uses the native close handler and tray language follows preferences", async ({ page }) => {
  await page.goto(`${url}?app=true&language=en`);
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "set_tray_language" && call.args.language === "en"))).toBe(true);
  await page.getByRole("button", { name: "Hide to tray", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "plugin:window|close").length)).toBe(1);
  await page.locator(".sidebar-menu").getByRole("menuitem", { name: "Settings", exact: true }).click();
  await page.locator(".settings-language").click();
  await page.locator(".ant-select-dropdown:visible").getByText("简体中文", { exact: true }).click();
  await expect(page.getByRole("button", { name: "隐藏到托盘", exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "set_tray_language" && call.args.language === "zh-CN"))).toBe(true);
});

for (const [language, devices, mcp, settings] of [
  ["en", "Device list", "MCP connections", "Settings"],
  ["zh-CN", "设备列表", "MCP 连接", "设置"],
  ["zh-TW", "裝置列表", "MCP 連線", "設定"],
]) for (const theme of ["light", "dark"]) {
  test(`MCP has a dedicated sidebar page in ${language}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1100, height: 700 });
    await page.goto(`${url}?app=true&language=${language}&theme=${theme}`);
    const menu = page.locator(".sidebar-menu");
    const labels = await menu.getByRole("menuitem").allTextContents();
    expect(labels[labels.indexOf(devices) + 1]).toBe(mcp);
    await menu.getByRole("menuitem", { name: mcp, exact: true }).click();
    await expect(menu.getByRole("menuitem", { name: mcp, exact: true })).toHaveClass(/ant-menu-item-selected/);
    await expect(page.getByRole("heading", { name: mcp, exact: true })).toBeVisible();
    await expect(page.locator(".page-mcp .ant-collapse-header")).toHaveCount(9);
    await page.locator(".page-mcp .ant-collapse-header").filter({ hasText: "DeepSeek Harness" }).click();
    await expect(page.getByText("dsh-mcp-client · client-version", { exact: true })).toBeVisible();
    const bounds = await page.locator(".mcp-connections-page").evaluate(element => {
      const rect = element.getBoundingClientRect();
      return { right: rect.right, bottom: rect.bottom, overflow: element.scrollWidth - element.clientWidth };
    });
    expect(bounds.right).toBe(1090);
    expect(bounds.bottom).toBe(690);
    expect(bounds.overflow).toBeLessThanOrEqual(1);
    await page.evaluate(() => (window as any).fixture.emit("mcp-reporting-changed", { revision: 2, running: true, count: 0, error: null, clients: [] }));
    await expect(page.locator(".page-mcp .ant-collapse-header")).toHaveCount(0);
    await expect(page.locator(".page-mcp .ant-empty")).toBeVisible();
    await menu.getByRole("menuitem", { name: settings, exact: true }).click();
    await page.locator(".settings-menu").getByRole("menuitem", { name: "AI Agent", exact: true }).click();
    await expect(page.locator(".settings-agent")).toHaveCount(5);
    await expect(page.getByTestId("agent-cursor")).toHaveCount(0);
    await expect(page.locator(".mcp-reporting-panel")).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => [...(window as any).fixture.listeners.values()].filter((listener: any) => listener.event === "mcp-reporting-changed").length)).toBe(0);
    await menu.getByRole("menuitem", { name: mcp, exact: true }).click();
    await expect(menu.getByRole("menuitem", { name: mcp, exact: true })).toHaveClass(/ant-menu-item-selected/);
    await expect(page.locator(".page-mcp .ant-collapse-header")).toHaveCount(9);
    await page.screenshot({ path: test.info().outputPath(`mcp-page-${language}-${theme}.png`), fullPage: true });
  });
}

for (const language of ["en", "zh-CN", "zh-TW"]) for (const theme of ["light", "dark"]) {
  test(`agent rows stay inside a narrow panel in ${language}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 420, height: 700 });
    await page.goto(`${url}?language=${language}&theme=${theme}`);
    for (const name of ["Codex", "Kimi Code", "Claude Code", "DeepSeek Harness", "OpenCode"]) await expect(page.getByText(name, { exact: true })).toBeVisible();
    await expect(page.getByTestId("agent-kimi").locator("button").first()).toBeEnabled();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const bounds = await page.getByTestId("agent-deepseek").locator("button").first().boundingBox();
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(420);
    await page.screenshot({ path: test.info().outputPath(`${language}-${theme}.png`), fullPage: true, animations: "disabled" });
  });
}
test("each client enables and disables independently", async ({ page }) => {
  await page.goto(url);
  const kimi = page.getByTestId("agent-kimi");
  await kimi.getByRole("button", { name: "Enable", exact: true }).click();
  await expect(kimi.getByText("Configured", { exact: true })).toBeVisible();
  await expect(page.getByTestId("agent-claude").getByText("Disabled", { exact: true })).toBeVisible();
  await kimi.getByRole("button", { name: "Disable", exact: true }).click();
  await expect(kimi.getByText("Disabled", { exact: true })).toBeVisible();
});
test("a broken client does not block others and refresh recovers it", async ({ page }) => {
  await page.goto(`${url}?error=true`);
  const claude = page.getByTestId("agent-claude");
  await expect(claude.getByRole("alert")).toContainText("Invalid JSON");
  await expect(page.getByTestId("agent-kimi").getByRole("button", { name: "Enable", exact: true })).toBeEnabled();
  await page.evaluate(() => { (window as any).fixture.states.claude.error = null; });
  await claude.getByRole("button", { name: "Refresh Claude Code" }).click();
  await expect(claude.getByRole("alert")).toHaveCount(0);
  await expect(claude.getByRole("button", { name: "Enable", exact: true })).toBeEnabled();
});
test("loading one agent leaves other controls usable", async ({ page }) => {
  await page.goto(`${url}?slow=true`);
  await page.getByTestId("agent-kimi").getByRole("button", { name: "Enable", exact: true }).click();
  await expect(page.getByTestId("agent-kimi").getByRole("button", { name: "Refresh Kimi Code" })).toBeDisabled();
  await page.getByTestId("agent-claude").getByRole("button", { name: "Enable", exact: true }).click();
  await expect(page.getByTestId("agent-claude").getByText("Configured", { exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).fixture.release());
  await expect(page.getByTestId("agent-kimi").getByText("Configured", { exact: true })).toBeVisible();
});
test("failed writes do not show configured and stale entries can be repaired", async ({ page }) => {
  await page.goto(`${url}?fail=true&repair=true`);
  const codex = page.getByTestId("agent-codex");
  await codex.getByRole("button", { name: "Update configuration" }).click();
  await expect(codex.getByRole("alert")).toContainText("config changed");
  await expect(codex.getByText("Configured", { exact: true })).toHaveCount(0);
  await expect(codex.getByRole("button", { name: "Disable", exact: true })).toBeEnabled();
});

for (const [language, signIn, me, signOut, guest] of [
  ["en", "Sign in", "Me", "Sign out", "Not signed in"],
  ["zh-CN", "登录账号", "我的", "退出账号", "未登录"],
  ["zh-TW", "登入賬號", "我的", "退出賬號", "未登入"],
]) for (const theme of ["light", "dark"]) {
  test(`sidebar account signs in and shows profile in ${language}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1100, height: 700 });
    await page.goto(`${url}?app=true&language=${language}&theme=${theme}`);
    const entry = page.locator(".sidebar-account");
    await expect(entry).toContainText(guest);
    const entryBox = await entry.boundingBox();
    const navBox = await page.locator(".sidebar-menu").boundingBox();
    expect(entryBox!.y + entryBox!.height).toBeLessThanOrEqual(navBox!.y);
    await entry.click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await page.mouse.click(1050, 600);
    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await dialog.locator("#account-login-username").fill(" Pixels ");
    await dialog.locator("#account-login-password").fill("fixture-password");
    await dialog.getByRole("button", { name: signIn, exact: true }).click();
    await expect(dialog).not.toBeVisible();
    await expect(entry).toContainText("Pixels");
    await expect(page.locator(".page-me")).toContainText("user-123");
    await expect(page.locator(".page-me")).not.toContainText("Pixels Cloud");
    await expect(page.locator(".page-me input")).toHaveCount(0);
    await expect(page.locator(".sidebar-menu .ant-menu-item-selected")).toHaveText(me);
    await page.locator(".sidebar-menu .ant-menu-item").first().click();
    await entry.click();
    await expect(page.locator(".page-me")).toBeVisible();
    await page.screenshot({ path: test.info().outputPath(`account-${language}-${theme}.png`), animations: "disabled" });
    await page.getByRole("button", { name: signOut, exact: true }).click();
    await expect(page.locator(".page-home")).toBeVisible();
    await expect(entry).toContainText(guest);
    await page.locator(".sidebar-menu").getByRole("menuitem", { name: me, exact: true }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.locator("#account-login-password")).toHaveValue("");
  });
}

test("sidebar account login failure, busy state, cancel and retry", async ({ page }) => {
  await page.goto(`${url}?app=true&loginError=true&slowLogin=true`);
  await page.locator(".sidebar-account").click();
  const dialog = page.getByRole("dialog");
  await dialog.locator("#account-login-username").fill("Pixels");
  await dialog.locator("#account-login-password").fill("wrong");
  await dialog.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(dialog.locator("#account-login-password")).toBeDisabled();
  await expect(dialog.locator(".ant-modal-close")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => typeof (window as any).fixture.release)).toBe("function");
  await page.evaluate(() => (window as any).fixture.release());
  await expect(dialog.getByRole("alert")).toContainText("Invalid credentials");
  await expect(page.locator(".sidebar-account")).toContainText("Not signed in");
  await dialog.locator(".ant-modal-close").click();
  await expect(dialog).not.toBeVisible();
  await page.locator(".sidebar-account").click();
  await expect(dialog.locator("#account-login-password")).toHaveValue("");
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await page.evaluate(() => { (window as any).fixture.loginError = false; (window as any).fixture.release = null; });
  await dialog.locator("#account-login-username").fill("Pixels");
  await dialog.locator("#account-login-password").fill("correct");
  await dialog.locator("#account-login-password").press("Enter");
  await expect.poll(() => page.evaluate(() => typeof (window as any).fixture.release)).toBe("function");
  await page.evaluate(() => (window as any).fixture.release());
  await expect(page.locator(".page-me")).toBeVisible();
});

test("sidebar account restores existing identity, handles read and logout errors", async ({ page }) => {
  await page.goto(`${url}?app=true&signedIn=true&longName=true&scopeError=true&logoutError=true`);
  const entry = page.locator(".sidebar-account");
  await expect(entry).toContainText("Could not load account");
  await entry.click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.evaluate(() => { (window as any).fixture.scopeError = false; });
  await entry.click();
  await expect(page.locator(".page-me")).toContainText("Pixels-very-long-account-name-for-layout");
  expect(await entry.evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page.locator(".page-me").getByRole("alert")).toContainText("Sign out failed");
  await expect(entry).toContainText("Pixels-very-long-account-name-for-layout");
  await page.evaluate(() => { (window as any).fixture.logoutError = false; });
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(entry).toContainText("Not signed in");
});
