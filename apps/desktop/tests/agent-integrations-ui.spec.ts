import { test, expect } from "@playwright/test";

for (const [language, theme, label, themeLabel] of [
  ["zh-CN", "light", "刷新全部信息", "切换为暗色"],
  ["en", "dark", "Refresh all information", "Switch to light theme"],
]) test(`global refresh sits before theme control (${language}, ${theme})`, async ({ page }, testInfo) => {
  await page.goto(`http://127.0.0.1:1429/tests/agent-integrations-ui.html?app&signedIn&language=${language}&theme=${theme}`);
  const controls = page.locator(".window-controls button");
  await expect(controls.nth(1)).toHaveAccessibleName(label);
  await expect(controls.nth(2)).toHaveAccessibleName(themeLabel);
  await expect(controls.nth(1)).toBeEnabled();
  await page.locator(".titlebar").screenshot({ path: testInfo.outputPath(`global-refresh-${language}-${theme}.png`) });
});

test("global refresh updates local status and saved devices while retaining inputs and connections", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app&signedIn");
  const refresh = page.getByRole("button", { name: "Refresh all information", exact: true });
  await expect(refresh).toBeEnabled();
  await page.evaluate(() => {
    const f = (window as any).fixture;
    f.devices = [{ deviceId: "remote-1", deviceCode: "987654321", alias: "Company", osFamily: "windows", osReminder: "", connected: true, connectionPath: "p2p" }];
    f.presence = true;
  });
  await refresh.click();
  const card = page.locator(".home-recent-device");
  await expect(card).toContainText("Company");
  await expect(card.locator(".home-recent-connection")).toContainText("P2P");
  await page.locator(".home-connect-form input").first().fill("111222333");
  await page.locator("#home-connect-password").fill("unsent password");
  await page.evaluate(() => {
    const f = (window as any).fixture;
    f.device = { ...f.device, device_code: "222333444", control_phase: "disconnected" };
    f.devices[0].alias = "Company updated";
    f.devices.push({ deviceId: "remote-2", deviceCode: "666777888", alias: "New device", osFamily: "linux", osReminder: "", connected: false });
    f.calls.length = 0;
  });
  await refresh.click();
  await expect(refresh).toBeEnabled();
  await expect(page.locator(".home-credential strong").first()).toHaveText("222 333 444");
  await expect(page.locator(".sidebar-status-group")).toContainText("Server disconnected");
  await expect(card).toHaveCount(2);
  await expect(card.filter({ hasText: "Company updated" }).locator(".home-recent-connection")).toContainText("P2P");
  await expect(card.filter({ hasText: "New device" })).toContainText("Online");
  await expect(page.locator(".home-connect-form input").first()).toHaveValue("111 222 333");
  await expect(page.locator("#home-connect-password")).toHaveValue("unsent password");
  const commands = await page.evaluate(() => (window as any).fixture.calls.map((c: any) => c.command));
  for (const command of ["operator_current_traffic_scope", "device_status", "operator_bootstrap", "operator_saved_device_presence", "operator_operations"]) expect(commands).toContain(command);
  expect(commands.filter((c: string) => /^(operator_connect|operator_connect_saved|operator_disconnect_device|refresh_device_password)$/.test(c))).toEqual([]);
});

test("global refresh joins an ongoing device refresh and prevents duplicate clicks", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app&signedIn");
  const refresh = page.getByRole("button", { name: "Refresh all information", exact: true });
  await expect(refresh).toBeEnabled();
  await page.locator(".sidebar-menu").getByRole("menuitem", { name: "Device list", exact: true }).click();
  await expect(page.getByRole("button", { name: "Refresh device list", exact: true })).toBeEnabled();
  await page.evaluate(() => { const f = (window as any).fixture; f.slowBootstrap = true; f.calls.length = 0; });
  await page.getByRole("button", { name: "Refresh device list", exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).fixture.release)).toBe("function");
  await refresh.click();
  await expect(refresh).toBeDisabled();
  await expect(refresh).toHaveAttribute("aria-busy", "true");
  await refresh.evaluate((button: HTMLButtonElement) => { button.click(); button.click(); });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "operator_current_traffic_scope").length)).toBe(1);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "operator_bootstrap").length)).toBe(1);
  await page.evaluate(() => { const f = (window as any).fixture; f.slowBootstrap = false; f.release(); });
  await expect(refresh).toBeEnabled();
  await expect(refresh).toHaveAttribute("aria-busy", "false");
});

test("global refresh reports partial failures, retains account identity and can retry", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app&signedIn");
  const refresh = page.getByRole("button", { name: "Refresh all information", exact: true });
  await expect(refresh).toBeEnabled();
  await page.evaluate(() => { (window as any).fixture.bootstrapError = true; });
  await refresh.click();
  await expect(page.locator(".toast.error")).toContainText("Some information could not be refreshed");
  await expect(page.locator(".sidebar-account")).toContainText("Pixels");
  await expect(page.locator(".home-credential strong").first()).toHaveText("123 456 789");
  await page.evaluate(() => { const f = (window as any).fixture; f.bootstrapError = false; f.scopeError = true; });
  await refresh.click();
  await expect(page.locator(".sidebar-account")).toContainText("Could not load account");
  await expect(page.locator(".home-connect-form button[type=submit]")).toBeDisabled();
  await page.evaluate(() => { (window as any).fixture.scopeError = false; });
  await refresh.click();
  await expect(refresh).toBeEnabled();
  await expect(page.locator(".toast.error")).toHaveCount(0);
  await expect(page.locator(".sidebar-account")).toContainText("Pixels");
});

test("global refresh discovers external login and logout without opening a login dialog", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app");
  const refresh = page.getByRole("button", { name: "Refresh all information", exact: true });
  await expect(page.locator(".sidebar-account")).toContainText("Not signed in");
  await page.evaluate(() => { (window as any).fixture.scope = { userId: "external-user", username: "External account", revision: 2, serverAdmin: false }; });
  await refresh.click();
  await expect(page.locator(".sidebar-account")).toContainText("External account");
  await page.locator(".sidebar-account").click();
  await expect(page.locator(".page-me")).toBeVisible();
  await page.evaluate(() => { (window as any).fixture.scope = null; });
  await refresh.click();
  await expect(page.locator(".sidebar-account")).toContainText("Not signed in");
  await expect(page.locator(".page-home")).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("global refresh reads profile panels and preserves unsaved server settings", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app&signedIn");
  const refresh = page.getByRole("button", { name: "Refresh all information", exact: true });
  await page.locator(".sidebar-account").click();
  await expect(page.locator(".page-me")).toBeVisible();
  await page.evaluate(() => { (window as any).fixture.calls.length = 0; });
  await refresh.click();
  await expect(refresh).toBeEnabled();
  const commands = await page.evaluate(() => (window as any).fixture.calls.map((c: any) => c.command));
  for (const command of ["device_account_status", "account_catalog_status", "github_status"]) expect(commands).toContain(command);
  expect(commands).not.toContain("github_login");
  await page.locator(".sidebar-menu").getByRole("menuitem", { name: "Settings", exact: true }).click();
  await page.locator(".settings-menu").getByRole("menuitem", { name: "Network connection", exact: true }).click();
  const input = page.locator(".settings-fields input").first();
  await expect(input).toBeEnabled();
  await input.fill("wss://my-unsaved-server.example/control");
  await refresh.click();
  await expect(refresh).toBeEnabled();
  await expect(input).toHaveValue("wss://my-unsaved-server.example/control");
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "save_operator_server_settings"))).toBe(false);
});

test("global refresh recovers MCP status and refreshes visible AI integration status", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app&signedIn");
  const refresh = page.getByRole("button", { name: "Refresh all information", exact: true });
  await expect(refresh).toBeEnabled();
  await page.evaluate(() => { (window as any).fixture.mcpError = true; });
  await page.locator(".sidebar-menu").getByRole("menuitem", { name: "MCP", exact: true }).click();
  await expect(page.locator(".mcp-reporting-panel").getByRole("alert")).toBeVisible();
  await page.evaluate(() => { (window as any).fixture.mcpError = false; });
  await refresh.click();
  await expect(page.locator(".mcp-reporting-panel").getByRole("alert")).toHaveCount(0);
  await expect(page.locator(".ant-collapse-header strong")).toHaveCount(9);
  await page.locator(".sidebar-menu").getByRole("menuitem", { name: "Settings", exact: true }).click();
  await page.locator(".settings-menu").getByRole("menuitem", { name: "AI Agent", exact: true }).click();
  const claude = page.getByTestId("agent-claude");
  await expect(claude.getByRole("button", { name: "Enable", exact: true })).toBeEnabled();
  await page.evaluate(() => { (window as any).fixture.states.claude.detected = false; });
  await refresh.click();
  await expect(claude.getByText("Client not detected", { exact: true })).toBeVisible();
});

test("global refresh loads new task history on the activity page", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app&signedIn");
  const refresh = page.getByRole("button", { name: "Refresh all information", exact: true });
  await expect(refresh).toBeEnabled();
  await page.locator(".sidebar-menu").getByRole("menuitem", { name: "Activity", exact: true }).click();
  await expect(page.locator(".page-activity")).toBeVisible();
  await page.evaluate(() => {
    (window as any).fixture.historyTasks = [{ id: "new-task", deviceCode: "987654321", initiatedBy: "fixture", program: "echo", args: ["Global refresh marker"], cwd: null,
      startedAtUnixMs: Date.now(), state: "succeeded", complete: true, stdout: "Global refresh marker", stderr: "", stdoutOffset: 21, stderrOffset: 0 }];
  });
  await refresh.click();
  await expect(refresh).toBeEnabled();
  await expect(page.locator(".page-activity")).toContainText("Global refresh marker");
  await expect(page.locator(".toast.error")).toHaveCount(0);
});

test("a stale global account refresh cannot overwrite a subsequent successful login", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app");
  const refresh = page.getByRole("button", { name: "Refresh all information", exact: true });
  await expect(refresh).toBeEnabled();
  await page.evaluate(() => { (window as any).fixture.slowScope = true; });
  await refresh.click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).fixture.release)).toBe("function");
  await page.locator(".sidebar-account").click();
  const dialog = page.getByRole("dialog");
  await dialog.locator("#account-login-username").fill("Pixels");
  await dialog.locator("#account-login-password").fill("fixture password");
  await dialog.locator("button[type=submit]").click();
  await expect(page.locator(".page-me")).toBeVisible();
  await page.evaluate(() => { const f = (window as any).fixture; f.slowScope = false; f.release(); });
  await expect(refresh).toBeEnabled();
  await expect(page.locator(".sidebar-account")).toContainText("Pixels");
  await expect(page.locator(".page-me")).toBeVisible();
});

for (const language of ['zh-CN','en']) for (const theme of ['light','dark']) test(`account page has clear sections and a native console action (${language}, ${theme})`,async({page},testInfo)=>{
  await page.setViewportSize({width:1000,height:640});
  await page.goto(`http://127.0.0.1:1429/tests/agent-integrations-ui.html?app=true&signedIn&longName=true&longId&language=${language}&theme=${theme}`);
  await page.locator('.sidebar-account').click();
  const account=page.locator('.account-page');
  await expect(account.locator('.account-profile-name strong')).toHaveText('Pixels-very-long-account-name-for-layout');
  await expect(account.locator('.account-profile-id')).toContainText('7dd29e34-8a55-415f-831c-1502271dcaf1');
  await expect(account.locator('.account-section')).toHaveCount(2);
  const open=account.getByRole('button',{name:language==='en'?'Open console':'打开后台',exact:true});
  await expect(open).toBeInViewport();
  await expect(account.getByRole('button',{name:language==='en'?'Sign out':'退出账号',exact:true})).toBeInViewport();
  expect(await account.evaluate(node=>node.scrollWidth-node.clientWidth)).toBeLessThanOrEqual(1);
  await open.click();
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.consoleOpens)).toBe(1);
  const call=await page.evaluate(()=>(window as any).fixture.calls.find((c:any)=>c.command==='open_account_console'));
  expect(call.args??{}).toEqual({});
  await account.screenshot({path:testInfo.outputPath(`account-${language}-${theme}.png`)});
});

for (const theme of ['light','dark']) test(`account page stacks narrow sections and allows scrolling (${theme})`,async({page},testInfo)=>{
  await page.setViewportSize({width:760,height:580});
  await page.goto(`http://127.0.0.1:1429/tests/agent-integrations-ui.html?app=true&signedIn&longName=true&longId&language=zh-CN&theme=${theme}`);
  await page.locator('.sidebar-account').click();
  const account=page.locator('.account-page');await expect(account).toBeVisible();
  const boxes=await account.locator('.account-section').evaluateAll(nodes=>nodes.map(n=>{const b=n.getBoundingClientRect();return {left:b.left,top:b.top,bottom:b.bottom}}));
  expect(Math.abs(boxes[0].left-boxes[1].left)).toBeLessThanOrEqual(1);expect(boxes[1].top).toBeGreaterThanOrEqual(boxes[0].bottom);
  expect(await account.evaluate(n=>n.scrollWidth-n.clientWidth)).toBeLessThanOrEqual(1);
  await account.getByRole('button',{name:'退出账号',exact:true}).scrollIntoViewIfNeeded();
  await expect(account.getByRole('button',{name:'退出账号',exact:true})).toBeInViewport();
  await page.screenshot({path:testInfo.outputPath(`account-narrow-${theme}.png`)});
});

test('console launch failure is readable and can retry without signing out',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/agent-integrations-ui.html?app=true&signedIn&consoleError&language=zh-CN');
  await page.locator('.sidebar-account').click();const account=page.locator('.account-page');
  await account.getByRole('button',{name:'打开后台',exact:true}).click();
  await expect(account.getByRole('alert')).toContainText('无法打开后台');
  await page.evaluate(()=>{(window as any).fixture.consoleError=false;});
  await account.getByRole('button',{name:'打开后台',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.consoleOpens)).toBe(1);
  await expect(account.getByRole('alert')).toHaveCount(0);await expect(account).toBeVisible();
});

test("removed Traditional Chinese preference migrates to Simplified Chinese", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app=true&language=zh-TW");
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  expect(await page.evaluate(() => localStorage.getItem("pab.language"))).toBe("zh-CN");
  await page.locator(".sidebar-menu").getByRole("menuitem", { name: "设置", exact: true }).click();
  await page.locator(".settings-language").click();
  await expect(page.locator(".ant-select-dropdown:visible").getByRole("option")).toHaveCount(2);
  await expect(page.locator(".ant-select-dropdown:visible")).toContainText("简体中文");
  await expect(page.locator(".ant-select-dropdown:visible")).toContainText("English");
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "set_tray_language" && c.args.language === "zh-CN"))).toBe(true);
});

for (const locale of ["zh-TW", "zh-HK", "zh-Hant"]) test(`Chinese system locale ${locale} selects Simplified Chinese`, async ({ browser }) => {
  const context = await browser.newContext({ locale });
  try {
    const page = await context.newPage();
    await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app=true&followSystem=true");
    await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
    await expect(page.locator(".sidebar-menu").getByRole("menuitem", { name: "设置", exact: true })).toBeVisible();
  } finally { await context.close(); }
});

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

for (const language of ["en", "zh-CN"]) test(`registration rejects invalid normalized account names before submitting (${language})`, async ({ page }) => {
  await page.goto(`${url}?app=true&language=${language}`);
  await page.locator(".sidebar-account").click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: language === "en" ? "Create account" : "注册账号", exact: true }).click();
  await dialog.locator("#account-login-password").fill("fixture password");
  await dialog.locator("#account-confirm-password").fill("fixture password");
  const username = dialog.locator("#account-login-username");
  for (const invalid of ["90", "９０", "x".repeat(65), "pc 90", "pc/90", "pc\\90"]) {
    await username.fill(invalid);
    await expect(username).toHaveAttribute("aria-invalid", "true");
    await expect(dialog.locator("#account-username-hint")).toContainText(language === "en" ? "3–64" : "3～64");
    await expect(dialog.locator('button[type="submit"]')).toBeDisabled();
    await dialog.locator("form").evaluate((form: HTMLFormElement) => form.requestSubmit());
  }
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((x: any) => x.command === "operator_register_account").length)).toBe(0);
  await username.fill("pc90");
  await expect(dialog.locator('button[type="submit"]')).toBeEnabled();
  await dialog.locator('button[type="submit"]').click();
  await expect(page.locator(".page-me")).toContainText("Pixels");
});

for (const language of ["en", "zh-CN"]) for (const [code, english, chinese] of [
  ["account_network_timeout", "request timed out", "账号请求超时"],
  ["account_dns_failed", "resolve the server or proxy hostname", "无法解析服务器或代理的域名"],
  ["account_tls_failed", "TLS verification or handshake failed", "TLS 验证或连接失败"],
  ["account_proxy_failed", "connect through the proxy", "代理连接失败"],
  ["account_proxy_authentication_required", "proxy requires authentication", "代理要求身份验证"],
  ["account_ca_invalid", "CA certificate file is unreadable", "CA 证书文件无法读取"],
  ["account_network_configuration_invalid", "prepare HTTPS", "无法初始化 HTTPS 连接"],
  ["account_connection_failed", "establish the account connection", "无法建立账号连接"],
]) test(`registration explains transport failure ${code} (${language})`, async ({ page }) => {
  await page.goto(`${url}?app=true&language=${language}&accountError=${code}`);
  await page.locator(".sidebar-account").click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: language === "en" ? "Create account" : "注册账号", exact: true }).click();
  await dialog.locator("#account-login-username").fill("proxy-fixture");
  await dialog.locator("#account-login-password").fill("fixture password");
  await dialog.locator("#account-confirm-password").fill("fixture password");
  await dialog.locator('button[type="submit"]').click();
  await expect(dialog.getByRole("alert")).toContainText(language === "en" ? english : chinese);
  await expect(dialog.locator('button[type="submit"]')).toBeEnabled();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "operator_register_account").length)).toBe(1);
});

for (const [code, text] of [
  ["account_username_taken", "该账号已存在，请更换账号或直接登录"],
  ["account_registration_disabled", "当前服务已关闭新账号注册，请联系管理员"],
  ["account_rate_limited", "操作过于频繁，请稍后重试"],
]) test(`registration explains server rejection ${code}`, async ({ page }) => {
  await page.goto(`${url}?app=true&language=zh-CN&accountError=${code}`);
  await page.locator(".sidebar-account").click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "注册账号", exact: true }).click();
  await dialog.locator("#account-login-username").fill("pc90");
  await dialog.locator("#account-login-password").fill("fixture password");
  await dialog.locator("#account-confirm-password").fill("fixture password");
  await dialog.locator('button[type="submit"]').click();
  await expect(dialog.getByRole("alert").filter({ hasText: text })).toBeVisible();
  await expect(dialog).not.toContainText("HTTP");
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
  ["en", "Device list", "MCP", "Settings"],
  ["zh-CN", "设备列表", "MCP 连接", "设置"],
]) for (const theme of ["light", "dark"]) {
  test(`MCP has a dedicated sidebar page in ${language}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 1100, height: 700 });
    await page.goto(`${url}?app=true&language=${language}&theme=${theme}`);
    const menu = page.locator(".sidebar-menu");
    await expect(menu).toHaveCSS("border-inline-end-width", "0px");
    await expect(page.locator(".sidebar")).toHaveCSS("border-right-width", "0px");
    const labels = await menu.getByRole("menuitem").allTextContents();
    expect(labels[labels.indexOf(devices) + 1]).toBe(mcp);
    await menu.getByRole("menuitem", { name: mcp, exact: true }).click();
    await expect(menu.getByRole("menuitem", { name: mcp, exact: true })).toHaveClass(/ant-menu-item-selected/);
    await expect(page.getByRole("heading", { name: language === "en" ? "MCP connections" : mcp, exact: true })).toBeVisible();
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
    await page.setViewportSize({ width: 1000, height: 700 });
    for (const label of await menu.locator(".ant-menu-title-content").all()) {
      expect(await label.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    }
    await page.screenshot({ path: test.info().outputPath(`mcp-sidebar-compact-${language}-${theme}.png`), fullPage: true });
  });
}

for (const language of ["en", "zh-CN",]) for (const theme of ["light", "dark"]) {
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
  await expect(claude.getByRole("alert")).toContainText("Settings could not be read or saved.");
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
  await expect(codex.getByRole("alert")).toContainText("Settings changed during this operation.");
  await expect(codex.getByText("Configured", { exact: true })).toHaveCount(0);
  await expect(codex.getByRole("button", { name: "Disable", exact: true })).toBeEnabled();
});

for (const [language, signIn, me, signOut, guest] of [
  ["en", "Sign in", "Me", "Sign out", "Not signed in"],
  ["zh-CN", "登录账号", "我的", "退出账号", "未登录"],
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
  await expect(dialog.getByRole("alert")).toContainText("The account or password is incorrect.");
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
  await expect(page.locator(".page-me [role='alert'].account-error")).toContainText("Could not sign out.");
  await expect(entry).toContainText("Pixels-very-long-account-name-for-layout");
  await page.evaluate(() => { (window as any).fixture.logoutError = false; });
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(entry).toContainText("Not signed in");
});


test("clear history removes all records and rejects a pre-clear refresh reply", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/agent-integrations-ui.html?app&signedIn");
  await page.locator(".sidebar-menu").getByRole("menuitem", { name:"Activity",exact:true }).click();
  const panel=page.locator('.page-activity');
  const clear=panel.getByRole('button',{name:'Clear history',exact:true});
  const refresh=panel.getByRole('button',{name:'Refresh',exact:true});
  await expect(clear).toBeDisabled();
  await page.evaluate(()=>{
    const f=(window as any).fixture;
    f.historyTasks=[{id:'clear-me',deviceCode:'987654321',initiatedBy:'fixture',program:'echo',args:['Clear marker'],cwd:null,startedAtUnixMs:Date.now(),state:'succeeded',complete:true,stdout:'Clear marker',stderr:'',stdoutOffset:12,stderrOffset:0}];
  });
  await refresh.click();
  await expect(panel.locator('.task-row')).toHaveCount(1);
  await page.evaluate(()=>(window as any).fixture.failClearHistory=true);
  await clear.click();
  await expect(panel.getByRole('alert').filter({hasText:'Could not clear history'})).toBeVisible();
  await expect(panel.locator('.task-row')).toHaveCount(1);
  await page.evaluate(()=>{const f=(window as any).fixture;f.failClearHistory=false;f.holdHistory=true;});
  await refresh.click();
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).fixture.releaseHistory)).toBe('function');
  await clear.click();
  await expect(panel.locator('.task-row')).toHaveCount(0);
  await page.evaluate(()=>{const f=(window as any).fixture;f.holdHistory=false;f.releaseHistory();});
  await expect(refresh).toBeEnabled();
  await refresh.click();
  await expect(refresh).toBeEnabled();
  await expect(panel.locator('.task-row')).toHaveCount(0);
  await expect(panel.locator('.count-badge')).toHaveText('0');
  await expect(clear).toBeDisabled();
});
