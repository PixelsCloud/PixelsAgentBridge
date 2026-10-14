import { test, expect, type Page } from "@playwright/test";

async function open(page: Page) {
  await page.goto("http://127.0.0.1:1429/tests/desktop-terminal.html");
  await expect(page.getByRole("button", { name: "Open Shell" })).toBeDisabled();
  await page.getByRole("combobox", { name: "Execution account" }).click();
  await page.locator(".ant-select-dropdown:visible").getByText("Administrator · 2", { exact: true }).click();
  await page.getByRole("button", { name: "Open Shell" }).click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
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
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).fixture.end());
  await expect(page.getByText(/Shell exited · 7/)).toBeVisible();
  await page.getByRole("button", { name: "Reopen" }).click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
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
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
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
