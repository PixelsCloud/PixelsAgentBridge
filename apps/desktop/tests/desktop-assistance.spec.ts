import { expect, test } from "@playwright/test";
test("prompt acknowledges display, opens only on user action, and never auto-completes", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/desktop-assistance.html");
  await expect(page.getByRole("dialog", { name: "AI requests your help" })).toBeVisible();
  await expect(page.getByText("Inspect the generated dialog")).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "desktop_assistance_displayed"))).toBe(true);
  expect(await page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "desktop_assistance_open"))).toBe(false);
  await page.getByRole("button", { name: "Open remote desktop" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => /complete/.test(c.command)).length)).toBe(0);
});
test("decline affects the request and does not issue a viewer close command", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/desktop-assistance.html");
  await page.getByRole("button", { name: "Decline" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => /cancel|close|complete|open/.test(c.command)).map((c: any) => c.command))).toEqual(["desktop_assistance_cancel"]);
});
