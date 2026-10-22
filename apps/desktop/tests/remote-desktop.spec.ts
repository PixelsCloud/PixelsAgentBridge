import { test, expect } from "@playwright/test";

const viewer = "http://127.0.0.1:1429/tests/remote-desktop.html";

test("text clipboard sync toggles independently without restarting video or input", async ({ page }) => {
  await page.goto(viewer);
  const toggle = page.getByRole("button", { name: "Disable text clipboard sync", exact: true });
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await toggle.click();
  await expect(page.getByRole("button", { name: "Enable text clipboard sync", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByRole("button", { name: "Stop control", exact: true })).toBeEnabled();
  await page.getByRole("button", { name: "Enable text clipboard sync", exact: true }).click();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  const calls = await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "desktop_viewer_control"));
  expect(calls.map((call: any) => call.args)).toEqual([{ clipboardEnabled: false }, { clipboardEnabled: true }]);
});

test("clipboard failures and old peers remain visible without failing the desktop", async ({ page }) => {
  await page.goto(viewer + "?language=zh-CN");
  const toggle = page.getByRole("button", { name: "关闭文字剪贴板同步", exact: true });
  await page.evaluate(() => (window as any).fixture.clipboard("unsupported"));
  await expect(toggle).toHaveAttribute("title", "文字剪贴板：请升级远端程序");
  await expect(page.getByRole("button", { name: "停止操作", exact: true })).toBeEnabled();
  await page.evaluate(() => (window as any).fixture.clipboard("paste_failed"));
  await expect(page.locator("header").getByRole("alert")).toContainText("未发送粘贴");
  await page.evaluate(() => (window as any).fixture.clipboard("active"));
  await expect(page.locator("header").getByRole("alert")).toHaveCount(0);
  await expect(toggle).toHaveAttribute("title", "文字剪贴板：同步中");
});

for (const language of ["en", "zh-CN"]) test(`viewer close asks natively, cancellation keeps control and confirmation closes (${language})`, async ({ page }) => {
  await page.goto(viewer + `?language=${language}`);
  const close = page.getByRole("button", { name: language === "en" ? "Close window" : "关闭窗口", exact: true });
  await close.click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.closeDialogs)).toBe(1);
  expect(await page.evaluate(() => (window as any).fixture.closed)).toBe(false);
  await expect(page.locator(".remote-desktop-control-toggle")).toHaveAttribute("aria-pressed", "true");
  await page.evaluate(() => { (window as any).fixture.closeAnswers = [true]; });
  await close.click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.closed)).toBe(true);
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  expect(calls.filter((c: any) => c.command === "desktop_viewer_close").map((c: any) => c.args)).toEqual([{ english: language === "en" }, { english: language === "en" }]);
  expect(calls.filter((c: any) => ["plugin:window|close", "desktop_viewer_control"].includes(c.command))).toEqual([]);
});

test("native close and repeated clicks share one pending confirmation and can retry a failed dialog", async ({ page }) => {
  await page.goto(viewer);
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "desktop_viewer_start"))).toBe(true);
  await page.evaluate(() => { (window as any).fixture.holdClose = true; (window as any).fixture.nativeClose(); });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.closeDialogs)).toBe(1);
  await page.getByRole("button", { name: "Close window", exact: true }).click();
  await page.evaluate(() => (window as any).fixture.nativeClose());
  expect(await page.evaluate(() => (window as any).fixture.closeDialogs)).toBe(1);
  await page.evaluate(() => { const f = (window as any).fixture; f.rejectClose = true; f.holdClose = false; f.releaseClose(); });
  await expect(page.locator("header").getByRole("alert")).toHaveText("Could not show close confirmation");
  await page.evaluate(() => { const f = (window as any).fixture; f.rejectClose = false; f.closeAnswers = [true]; f.nativeClose(); });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.closed)).toBe(true);
  await expect(page.locator("header").getByRole("alert")).toHaveCount(0);
});

for (const theme of ["light", "dark"]) test(`Chinese toolbar fits the minimum window with Windows actions (${theme})`, async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 640, height: 400 });
  await page.goto(viewer + `?language=zh-CN&theme=${theme}`);
  await expect(page.getByRole("button", { name: "停止操作", exact: true })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Ctrl + Alt + Delete", exact: true })).toBeInViewport();
  await expect(page.getByRole("button", { name: "关闭窗口", exact: true })).toBeInViewport();
  await page.getByRole("button", { name: "调试信息", exact: true }).click();
  await expect(page.getByRole("complementary", { name: "调试信息" })).toBeVisible();
  expect(await page.getByRole("complementary", { name: "调试信息" }).evaluate(node => node.getBoundingClientRect().top)).toBeGreaterThanOrEqual(42);
  expect(await page.locator("html").evaluate(node => node.scrollWidth)).toBe(640);
  await page.screenshot({ path: testInfo.outputPath(`toolbar-${theme}.png`) });
});

test("control starts enabled and Debug is a titlebar toggle with detailed local information", async ({ page }) => {
  await page.goto(viewer);
  const header = page.locator(".titlebar");
  await expect(header.getByRole("button", { name: "Stop control" })).toHaveAttribute("aria-pressed", "true");
  const toggle = header.getByRole("button", { name: "Debug information", exact: true });
  const panel = page.getByRole("complementary", { name: "Debug information" });
  await expect(toggle.locator("svg.lucide-bug")).toBeVisible();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(panel).toBeHidden();
  await toggle.click(); await expect(panel).toBeVisible();
  await expect(panel).toContainText("viewer-session-fixture");
  await expect(panel).toContainText("Connection path"); await expect(panel).toContainText("P2P");
  await expect(panel).toContainText("Encoder target (not measured)");
  await toggle.click(); await expect(panel).toBeHidden();
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  expect(calls.filter((call: any) => call.command === "desktop_viewer_control")).toEqual([]);
  expect(calls.filter((call: any) => call.command === "desktop_viewer_overlay").at(-1).args.text).toBe("");
  await header.getByRole("button", { name: "Maximize", exact: true }).click();
  await header.getByRole("button", { name: "Minimize", exact: true }).click();
  await page.setViewportSize({ width: 640, height: 400 });
  await expect(header.getByRole("button", { name: "Close window", exact: true })).toBeInViewport();
  await expect(header.getByRole("combobox", { name: "Display" })).toBeInViewport();
  expect(await page.locator("html").evaluate(node => node.scrollWidth)).toBe(640);
});

test("fullscreen hides the titlebar, changes its icon and Esc restores normal layout", async ({ page }) => {
  await page.goto(viewer);
  await page.getByRole("button", { name: "Fullscreen", exact: true }).click();
  await expect(page.locator("header")).toBeHidden();
  await expect(page.locator(".remote-desktop")).toHaveClass(/remote-desktop-fullscreen/);
  await expect(page.locator('button[aria-label="Exit fullscreen (Esc)"] svg.lucide-shrink')).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(page.locator("header")).toBeVisible();
  await expect(page.getByRole("button", { name: "Fullscreen", exact: true }).locator("svg.lucide-expand")).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_control"))).toEqual([]);
});

test("native-focus Escape and fullscreen failure synchronize the same local state", async ({ page }) => {
  await page.goto(viewer); await page.getByRole("button", { name: "Fullscreen", exact: true }).click();
  await expect(page.locator("header")).toBeHidden();
  await page.evaluate(() => (window as any).fixture.nativeEscape());
  await expect(page.locator("header")).toBeVisible();
  await page.evaluate(() => { (window as any).fixture.rejectFullscreen = true; });
  await page.getByRole("button", { name: "Fullscreen", exact: true }).click();
  await expect(page.locator("header")).toBeVisible();
  await expect(page.locator("header").getByRole("alert")).toContainText("Fullscreen unavailable");
});

for (const native of [false, true]) test(`Esc wins over a delayed fullscreen reply (native focus: ${native})`, async ({ page }) => {
  await page.goto(viewer);
  await expect(page.getByRole("button", { name: "Stop control" })).toBeEnabled();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "desktop_viewer_fullscreen"))).toBe(true);
  await page.evaluate(() => { (window as any).fixture.holdFullscreen = true; });
  await page.getByRole("button", { name: "Fullscreen", exact: true }).click();
  await expect(page.locator("header")).toBeHidden();
  if (native) await page.evaluate(() => (window as any).fixture.nativeEscape());
  else await page.keyboard.press("Escape");
  await page.evaluate(() => {
    const f = (window as any).fixture;
    f.holdFullscreen = false; f.releaseFullscreen();
  });
  await expect(page.locator("header")).toBeVisible();
  await expect(page.locator(".remote-desktop")).not.toHaveClass(/remote-desktop-fullscreen/);
});

for (const os of ["windows", "linux", "macos", "android", "unknown"]) test(`secure attention is literal text and Windows-only (${os})`, async ({ page }) => {
  await page.goto(viewer + `?os=${os}`);
  const sas = page.getByRole("button", { name: "Ctrl + Alt + Delete", exact: true });
  if (os !== "windows") { await expect(sas).toHaveCount(0); return; }
  await expect(sas).toBeEnabled(); await expect(sas.locator("svg")).toHaveCount(0);
  await sas.click();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_control").map((c: any) => c.args))).toEqual([{ secureAttention: true }]);
});

test("decoder and capture target details use reported values without inventing measured rates", async ({ page }) => {
  await page.goto(viewer); await page.getByRole("button", { name: "Debug information" }).click();
  for (const [kind, label] of [["d3d11_hardware", "Hardware decoding"], ["openh264_software", "Software decoding"], ["videotoolbox_hardware", "Hardware decoding"]]) {
    await page.evaluate(value => (window as any).fixture.decoder(value), kind);
    await expect(page.locator(".remote-desktop-decoder")).toContainText(label);
    expect(await page.locator("header").evaluate(node => node.getBoundingClientRect().height)).toBe(42);
  }
  await page.evaluate(() => (window as any).fixture.recover());
  await expect(page.locator(".remote-desktop-decoder")).toHaveText("Not reported");
  await expect(page.getByRole("status")).toHaveText("Restoring desktop");
});

test("controls keep stable screen IDs and the user can leave input stopped across screen changes", async ({ page }) => {
  await page.goto(viewer);
  await page.getByRole("button", { name: "Stop control" }).click();
  await expect(page.getByRole("button", { name: "Enable control" })).toHaveAttribute("aria-pressed", "false");
  await page.getByRole("combobox", { name: "Display" }).selectOption("adapter:left");
  await expect(page.getByRole("status")).toHaveText("Connected");
  await expect(page.getByRole("button", { name: "Enable control" })).toHaveAttribute("aria-pressed", "false");
  await page.getByRole("combobox", { name: "FPS" }).selectOption("30");
  await expect(page.getByRole("combobox", { name: "FPS" })).toHaveValue("30");
  const calls = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_control").map((c: any) => c.args));
  expect(calls).toEqual([{ inputEnabled: false }, { screen: "adapter:left" }, { fps: 30 }]);
});

test("input remains gated by the first presented frame and unsupported peers", async ({ page }) => {
  await page.goto(viewer + "?ready=0");
  await expect(page.getByRole("button", { name: "Enable control" })).toBeDisabled();
  await page.evaluate(() => (window as any).fixture.ready(true));
  await expect(page.getByRole("button", { name: "Stop control" })).toHaveAttribute("aria-pressed", "true");
  await page.evaluate(() => (window as any).fixture.ready(true, false));
  await expect(page.getByRole("button", { name: "Enable control" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Ctrl + Alt + Delete" })).toBeDisabled();
});

test("control errors remain above native surface and capture failure can reconnect", async ({ page }) => {
  await page.goto(viewer); await page.evaluate(() => { (window as any).fixture.reject = true; });
  await page.getByRole("combobox", { name: "FPS" }).selectOption("30");
  await expect(page.locator("header").getByRole("alert")).toContainText("Renderer is busy");
  await page.evaluate(() => { (window as any).fixture.reject = false; (window as any).fixture.fail(); });
  await expect(page.getByRole("alert")).toContainText("Capture helper unavailable");
  await page.getByRole("button", { name: "Reconnect" }).click();
  await expect(page.getByRole("button", { name: "Stop control" })).toBeEnabled();
});

test("assistance requires explicit completion and cannot complete during recovery", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await page.evaluate(() => { (window as any).fixture.assistance = [{ assistance_id: "first", status: "active", request: { expected_action: "Inspect generated UI" } }]; });
  const complete = page.getByRole("button", { name: "Complete assistance: Inspect generated UI" });
  await expect(complete).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_assistance_complete").length)).toBe(0);
  await page.evaluate(() => (window as any).fixture.recover());
  await expect(complete).toBeDisabled();
  await page.evaluate(() => { (window as any).fixture.fail(); });
  await page.getByRole("button", { name: "Reconnect" }).click();
  await expect(complete).toBeEnabled();
  await complete.click();
  await expect(complete).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_assistance_complete").map((c: any) => c.args))).toEqual([{ id: "first" }]);
});
test("shared viewer identifies the request being completed and preserves the next request", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await page.evaluate(() => { (window as any).fixture.assistance = [
    { assistance_id: "first", status: "active", request: { expected_action: "Inspect first dialog" } },
    { assistance_id: "second", status: "active", request: { expected_action: "Inspect second dialog" } },
  ]; });
  await page.getByRole("button", { name: "Complete assistance (2): Inspect first dialog" }).click();
  await expect(page.getByRole("button", { name: "Complete assistance: Inspect second dialog" })).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_assistance_complete").map((c: any) => c.args))).toEqual([{ id: "first" }]);
});

test("replacement assistance can complete while an old completion is pending", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await page.evaluate(() => {
    const f = (window as any).fixture;
    f.assistance = [{ assistance_id: "old", status: "active", request: { expected_action: "Old request" } }];
    f.holdCompletion = ["old"];
  });
  await page.getByRole("button", { name: "Complete assistance: Old request" }).click();
  await page.evaluate(() => { (window as any).fixture.assistance = [{ assistance_id: "new", status: "active", request: { expected_action: "New request" } }]; });
  const replacement = page.getByRole("button", { name: "Complete assistance: New request" });
  await expect(replacement).toBeEnabled();
  await replacement.click();
  await expect(replacement).toHaveCount(0);
  await page.evaluate(() => (window as any).fixture.deferredCompletion.old.resolve());
  await expect(page.locator(".remote-desktop-assistance")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_assistance_complete").map((c: any) => c.args.id))).toEqual(["old", "new"]);
});

test("late failure of an old completion cannot replace the new request status", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await page.evaluate(() => {
    const f = (window as any).fixture;
    f.assistance = [{ assistance_id: "old", status: "active", request: { expected_action: "Old request" } }];
    f.holdCompletion = ["old"];
  });
  await page.getByRole("button", { name: "Complete assistance: Old request" }).click();
  await page.evaluate(() => { (window as any).fixture.assistance = [{ assistance_id: "new", status: "active", request: { expected_action: "New request" } }]; });
  const replacement = page.getByRole("button", { name: "Complete assistance: New request" });
  await expect(replacement).toBeVisible();
  await page.evaluate(() => (window as any).fixture.deferredCompletion.old.reject("Old request expired"));
  await expect(replacement).toBeEnabled();
  await expect(page.locator("header").getByRole("alert")).toHaveCount(0);
  await expect(page.getByRole("status")).toHaveText("Connected");
});

test("current completion failure stays visible above the native surface and can retry", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await page.evaluate(() => {
    const f = (window as any).fixture;
    f.assistance = [{ assistance_id: "current", status: "active", request: { expected_action: "Current request" } }];
    f.holdCompletion = ["current"];
  });
  const complete = page.getByRole("button", { name: "Complete assistance: Current request" });
  await complete.click();
  await expect(complete).toBeDisabled();
  await page.evaluate(() => (window as any).fixture.deferredCompletion.current.reject("Could not complete current request"));
  await expect(page.locator("header").getByRole("alert")).toHaveText("Could not complete current request");
  await expect(complete).toBeEnabled();
  await page.evaluate(() => { (window as any).fixture.holdCompletion = []; });
  await complete.click();
  await expect(complete).toHaveCount(0);
  await expect(page.locator("header").getByRole("alert")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Stop control" })).toHaveAttribute("aria-pressed", "true");
});
