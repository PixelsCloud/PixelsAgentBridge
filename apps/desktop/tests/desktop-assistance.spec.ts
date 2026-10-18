import { expect, test } from "@playwright/test";

for (const oldCommand of ["desktop_assistance_open", "desktop_assistance_cancel", "desktop_assistance_displayed"]) {
  test(`late ${oldCommand} failure cannot change a replacement prompt`, async ({ page }) => {
    await page.goto("http://127.0.0.1:1429/tests/desktop-assistance.html");
    await expect(page.getByRole("dialog")).toBeVisible();
    // Use a fresh request so its display acknowledgment can also be delayed.
    await page.evaluate((command) => {
      const fixture = (window as any).fixture;
      fixture.hold = [`${command}:old`, "desktop_assistance_open:new"];
      fixture.requests = [{ ...fixture.requests[0], assistance_id: "old", request: { ...fixture.requests[0].request, reason: "Old request" } }];
    }, oldCommand);
    await expect(page.getByText("Old request", { exact: true })).toBeVisible();
    if (oldCommand !== "desktop_assistance_displayed") {
      await page.getByRole("button", { name: oldCommand === "desktop_assistance_open" ? "Open remote desktop" : "Decline" }).click();
    }
    await expect.poll(() => page.evaluate((command) => Boolean((window as any).fixture.deferred[`${command}:old`]), oldCommand)).toBe(true);
    await page.evaluate(() => {
      const fixture = (window as any).fixture;
      fixture.requests = [{ ...fixture.requests[0], assistance_id: "new", request: { ...fixture.requests[0].request, reason: "New request" } }];
    });
    await expect(page.getByText("New request", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Open remote desktop" }).click();
    await expect(page.getByRole("button", { name: "Decline" })).toBeDisabled();
    await page.evaluate((command) => (window as any).fixture.deferred[`${command}:old`].reject("Old request failed"), oldCommand);
    // Drain the old rejection and React's resulting render before asserting absence.
    await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    await expect(page.getByRole("alert")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Decline" })).toBeDisabled();
    await page.evaluate(() => (window as any).fixture.deferred["desktop_assistance_open:new"].reject("New request failed"));
    await expect(page.getByRole("alert")).toHaveText("New request failed");
    await expect(page.getByRole("button", { name: "Decline" })).toBeEnabled();
  });
}
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
