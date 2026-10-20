import { expect, test } from "@playwright/test";

test("a hidden native window keeps delivery pending and acknowledges after restoration", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/desktop-assistance.html");
  await page.evaluate(() => {
    const fixture = (window as any).fixture;
    fixture.nativeVisible = false;
    fixture.requests = [{ ...fixture.requests[0], assistance_id: "hidden" }];
  });
  const acknowledgments = () => page.evaluate(() => (window as any).fixture.calls.filter(
    (c: any) => c.command === "desktop_assistance_displayed" && c.args.id === "hidden",
  ).length);
  await expect.poll(acknowledgments).toBeGreaterThanOrEqual(2);
  await expect(page.getByRole("alert")).toHaveCount(0);
  const before = await acknowledgments();
  await page.evaluate(() => { (window as any).fixture.nativeVisible = true; });
  await expect.poll(acknowledgments).toBeGreaterThan(before);
  const confirmed = await acknowledgments();
  // Observe two retry intervals: delivery should stop probing after acceptance.
  await page.waitForTimeout(2200);
  expect(await acknowledgments()).toBe(confirmed);
  expect(await page.evaluate(() => (window as any).fixture.calls.some(
    (c: any) => /open|complete/.test(c.command),
  ))).toBe(false);
});

test("a delayed delivery acknowledgment is not duplicated and a failure can recover", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/desktop-assistance.html");
  await page.evaluate(() => {
    const fixture = (window as any).fixture;
    fixture.hold = ["desktop_assistance_displayed:delayed"];
    fixture.requests = [{ ...fixture.requests[0], assistance_id: "delayed" }];
  });
  await expect.poll(() => page.evaluate(() => Boolean((window as any).fixture.deferred["desktop_assistance_displayed:delayed"]))).toBe(true);
  await page.waitForTimeout(2200);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter(
    (c: any) => c.command === "desktop_assistance_displayed" && c.args.id === "delayed",
  ).length)).toBe(1);
  await page.evaluate(() => (window as any).fixture.deferred["desktop_assistance_displayed:delayed"].reject("Transient IPC failure"));
  await expect(page.getByRole("alert")).toHaveText("Could not update the assistance request.");
  await page.evaluate(() => { (window as any).fixture.hold = []; });
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByRole("dialog")).toBeVisible();
});

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
