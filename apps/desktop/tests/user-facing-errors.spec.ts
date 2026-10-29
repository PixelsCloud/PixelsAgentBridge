import { test, expect } from "@playwright/test";
import { errorMessages, userFacingError } from "../src/userFacingError";
import { messages } from "../src/i18n";

test("errors explain the cause in both supported languages without exposing backend details", () => {
  const cases = [
    ["remote desktop Busy: remote desktop Busy: this device already has a desktop viewer", "occupied"],
    ["android_system_action_unavailable", "androidUnsupported"],
    ["remote desktop Unsupported: Android remote desktop is not available on this device", "androidUnsupported"],
    ["Console session discovery unavailable", "console"],
    ["remote desktop MediaUnavailable: desktop presentation backlog expired", "media"],
    ["execution_context_expired", "userChanged"],
    ["remote desktop InvalidRequest: remote desktop is view only", "viewOnly"],
    ["No space left on device (os error 28)", "space"],
    ["permission denied: C:\\private\\account.toml", "permission"],
    ["unconfirmed: connection timed out", "unconfirmed"],
    ["TLS certificate unknown issuer", "certificate"],
    ["SHA256 hash mismatch", "integrity"],
    ["desktop_taken_over", "takenOver"],
  ] as const;
  for (const [raw, key] of cases) {
    expect(userFacingError(raw, "zh-CN")).toBe(errorMessages[key][0]);
    expect(userFacingError(raw, "en")).toBe(errorMessages[key][1]);
  }
  expect(userFacingError({ code: "permission_denied", message: "C:\\secret.txt" }, "en")).toBe(errorMessages.permission[1]);
  for (const value of [new Error("internal_foo /private/config token=secret"), {}, null, "<script>alert(1)</script>", "x".repeat(100_000)]) {
    expect(userFacingError(value, "zh-CN")).toBe(errorMessages.unknown[0]);
    expect(userFacingError(value, "en")).toBe(errorMessages.unknown[1]);
  }
});

test("known translations follow the current language and contextual errors keep safe prefixes", () => {
  const keys = (value: object, prefix = ""): string[] => Object.entries(value).flatMap(([key, entry]) =>
    entry && typeof entry === "object" ? keys(entry, `${prefix}${key}.`) : [`${prefix}${key}`]).sort();
  expect(keys(messages.en)).toEqual(keys(messages["zh-CN"]));
  expect(userFacingError(messages.en.accountUsernameInvalid, "zh-CN")).toBe(messages["zh-CN"].accountUsernameInvalid);
  expect(userFacingError(String(new Error(messages.en.accountUsernameInvalid)), "zh-CN")).toBe(messages["zh-CN"].accountUsernameInvalid);
  expect(userFacingError("checksum does not match", "zh-CN", "update")).toBe(errorMessages.updateVerification[0]);
  expect(userFacingError("Native action failed: open_macos_permission_settings", "en")).toBe(errorMessages.macosSettings[1]);
  expect(userFacingError(`${messages.en.accountRegisterFailed}: database_driver_error /secret/file`, "zh-CN"))
    .toBe(`${messages["zh-CN"].accountRegisterFailed}: ${errorMessages.unknown[0]}`);
  for (const [zh, en] of Object.values(errorMessages)) {
    expect(zh).toMatch(/[\u4e00-\u9fff]/);
    expect(en).not.toMatch(/[\u4e00-\u9fff]/);
    expect(userFacingError(zh, "en")).toBe(en);
    expect(userFacingError(en, "zh-CN")).toBe(zh);
  }
});

for (const language of ["zh-CN", "en"] as const) {
  test(`desktop Busy is readable and does not manufacture takeover permission (${language})`, async ({ page }) => {
    await page.goto(`http://127.0.0.1:1429/tests/remote-desktop.html?language=${language}`);
    await expect(page.getByRole("status")).toContainText("P2P");
    await page.evaluate(() => (window as any).fixture.emit({ phase: "failed", error: "remote desktop Busy: remote desktop Busy: this device already has a desktop viewer" }));
    await expect(page.locator("section").getByRole("alert")).toHaveText(errorMessages.occupied[language === "en" ? 1 : 0]);
    await expect(page.getByRole("button", { name: language === "en" ? "Take over" : "强制接管", exact: true })).toHaveCount(0);
    expect(await page.evaluate(() => (window as any).fixture.takeoverDialogs)).toBe(0);
    await page.screenshot({ path: test.info().outputPath("localized-busy.png") });
  });

  test(`occupied session keeps the takeover action after an error (${language})`, async ({ page }) => {
    await page.goto(`http://127.0.0.1:1429/tests/remote-desktop.html?language=${language}`);
    await expect(page.getByRole("status")).toContainText("P2P");
    await page.evaluate(() => (window as any).fixture.emit({ phase: "occupied", takeover_session: "verified-session", error: "The remote session changed or could not be released. Reconnect and confirm again." }));
    const button = page.getByRole("button", { name: language === "en" ? "Take over" : "强制接管", exact: true });
    await expect(button).toBeVisible();
    await expect(button).toBeEnabled();
    await expect.poll(() => page.evaluate(() => (window as any).fixture.takeoverDialogs)).toBe(1);
    await expect(page.locator("section")).toContainText(errorMessages.takeoverChanged[language === "en" ? 1 : 0]);
    await button.click();
    await expect.poll(() => page.evaluate(() => (window as any).fixture.takeoverDialogs)).toBe(2);
    await page.screenshot({ path: test.info().outputPath("localized-takeover.png") });
  });
}
