import { expect, test } from "@playwright/test";

test("Chinese confirmation starts on cancel and Escape never accepts", async ({ page }) => {
  await page.setViewportSize({ width: 628, height: 280 });
  await page.goto("http://127.0.0.1:1429/tests/confirmation.html");
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await expect(page.getByRole("button", { name: /取\s*消/ })).toBeFocused();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((c: any) => c.command === "confirmation_ready" && c.args.height > 100))).toBe(true);
  await page.keyboard.press("Escape");
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "confirmation_answer").map((c: any) => c.args.accepted))).toEqual([false]);
});

test("English dark confirmation accepts only once while awaiting native action", async ({ page }) => {
  await page.setViewportSize({ width: 628, height: 300 });
  await page.goto("http://127.0.0.1:1429/tests/confirmation.html?language=en&theme=dark");
  await expect(page.getByRole("dialog")).toContainText("Taking over will disconnect");
  await page.evaluate(() => { (window as any).fixture.hold = true; });
  await page.getByRole("button", { name: "Take over", exact: true }).dblclick();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "confirmation_answer").map((c: any) => c.args.accepted))).toEqual([true]);
  await page.screenshot({ path: "../../.build/confirmation-dark.png" });
  await page.evaluate(() => (window as any).fixture.release());
});

test("Chinese light confirmation matches app styling and cancel preserves operation", async ({ page }) => {
  await page.setViewportSize({ width: 628, height: 240 });
  await page.goto("http://127.0.0.1:1429/tests/confirmation.html");
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.getByRole("button", { name: "关闭连接", exact: true })).toHaveClass(/danger/);
  await page.screenshot({ path: "../../.build/confirmation-light.png" });
  await page.getByRole("button", { name: /取\s*消/ }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "confirmation_answer").map((c: any) => c.args.accepted))).toEqual([false]);
});
