import { test, expect } from "@playwright/test";

test("decoder badge uses actual reports and clears during recovery", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await expect(page.getByRole("status")).toHaveText("Connected");
  await expect(page.locator(".remote-desktop-decoder")).toHaveCount(0);
  for (const [kind, label] of [["d3d11_hardware", "Hardware decoding"], ["openh264_software", "Software decoding"], ["videotoolbox_hardware", "Hardware decoding"]]) {
    await page.evaluate((value) => (window as any).fixture.decoder(value), kind);
    await expect(page.locator(".remote-desktop-decoder")).toHaveText(label);
    expect(await page.locator(".remote-desktop-toolbar").evaluate((node) => node.getBoundingClientRect().height)).toBe(72);
  }
  await page.evaluate(() => (window as any).fixture.decoder("unknown_future_decoder"));
  await expect(page.locator(".remote-desktop-decoder")).toHaveCount(0);
  await page.evaluate(() => { (window as any).fixture.decoder("openh264_software"); (window as any).fixture.recover(); });
  await expect(page.getByRole("status")).toHaveText("Restoring desktop");
  await expect(page.locator(".remote-desktop-decoder")).toHaveCount(0);
});
test("viewer uses stable screen IDs and bounded controls without frame or input IPC", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await expect(page.getByRole("status")).toHaveText("Connected");
  await expect(page.getByText("View only", { exact: true })).toBeVisible();
  expect(await page.locator(".remote-desktop-toolbar").evaluate((node) => node.getBoundingClientRect().height)).toBe(72);
  await page.getByRole("combobox", { name: "Display" }).selectOption("adapter:left");
  await expect(page.getByRole("status")).toHaveText("Connected");
  await expect(page.getByRole("combobox", { name: "Display" })).toHaveValue("adapter:left");
  await page.getByRole("combobox", { name: "FPS" }).selectOption("30");
  await expect(page.getByRole("combobox", { name: "FPS" })).toHaveValue("30");
  const calls = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => !c.command.startsWith("plugin:")));
  expect(calls.filter((c: any) => c.command === "desktop_viewer_control").map((c: any) => c.args)).toEqual([{ screen: "adapter:left" }, { fps: 30 }]);
  expect(calls.every((c: any) => ["desktop_viewer_start", "desktop_viewer_control", "desktop_viewer_assistance"].includes(c.command))).toBe(true);
});
test("secure attention requires control and recovery never restores input automatically", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  const sas = page.getByRole("button", { name: "Ctrl+Alt+Delete", exact: true });
  await expect(sas).toBeDisabled();
  await page.getByRole("button", { name: "Enable control" }).click();
  await expect(sas).toBeEnabled();
  await sas.click();
  await expect(page.getByRole("status")).toHaveText("Connected");
  await expect(sas).toBeDisabled();
  const commands = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_control").map((c: any) => c.args));
  expect(commands).toEqual([{ inputEnabled: true }, { secureAttention: true }]);
  await page.evaluate(() => (window as any).fixture.recover());
  await expect(page.getByRole("status")).toHaveText("Restoring desktop");
  await expect(page.getByRole("button", { name: "Enable control" })).toBeDisabled();
  await expect(sas).toBeDisabled();
  await expect(page.getByText("View only", { exact: true })).toBeVisible();
});
test("control errors remain above native surface and capture failure can reconnect", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await expect(page.getByRole("status")).toHaveText("Connected");
  await page.evaluate(() => { (window as any).fixture.reject = true; });
  await page.getByRole("combobox", { name: "FPS" }).selectOption("30");
  await expect(page.locator("header").getByRole("alert")).toContainText("Renderer is busy");
  await page.evaluate(() => { (window as any).fixture.reject = false; (window as any).fixture.fail(); });
  await expect(page.getByRole("alert")).toContainText("Capture helper unavailable");
  await page.getByRole("button", { name: "Reconnect" }).click();
  await expect(page.getByRole("status")).toHaveText("Connected");
});
test("control requires the first frame and resets to viewing on a new screen", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/remote-desktop.html");
  await expect(page.getByRole("button", { name: "Enable control" })).toHaveAttribute("aria-pressed", "false");
  await page.evaluate(() => (window as any).fixture.ready(false));
  await expect(page.getByRole("button", { name: "Enable control" })).toBeDisabled();
  await page.evaluate(() => (window as any).fixture.ready(true));
  await page.getByRole("button", { name: "Enable control" }).click();
  await expect(page.getByRole("button", { name: "Stop control" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByText("Control enabled", { exact: true })).toBeVisible();
  await page.getByRole("combobox", { name: "Display" }).selectOption("adapter:left");
  await expect(page.getByRole("button", { name: "Enable control" })).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByText("View only", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "desktop_viewer_control").map((c: any) => c.args));
  expect(calls).toEqual([{ inputEnabled: true }, { screen: "adapter:left" }]);
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
  await expect(page.getByText("View only", { exact: true })).toBeVisible();
});
