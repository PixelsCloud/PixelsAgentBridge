import { test, expect } from "@playwright/test";
import { fileURLToPath } from "node:url";

const url = "http://127.0.0.1:1429/tests/history-settings-ui.html";
test("completion arriving during a byte read drains the final stdout and stderr", async ({ page }) => {
  await page.goto(`${url}?language=en&streaming=true`);
  await expect.poll(() => page.evaluate(() => typeof (window as any).fixture.releaseInitial)).toBe("function");
  await page.evaluate(() => (window as any).fixture.finishTask());
  await expect(page.locator(".task-row em")).toHaveText("Succeeded");
  await page.evaluate(() => (window as any).fixture.releaseInitial());
  await expect(page.locator(".task-output pre").first()).toHaveText("first\nlast\n");
  await expect(page.locator(".task-output pre.stderr-output")).toHaveText("done\n");
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  expect(calls).toHaveLength(2);
  expect(calls[1].args.stdoutOffset).toBe(6);
  expect(calls[1].args.stderrOffset).toBe(0);
});
for (const [language, label] of [["en", "English"], ["zh-CN", "简体中文"], ["zh-TW", "繁體中文"]]) {
  test(`explicitly choosing the current ${language} default is remembered`, async ({ page }) => {
    await page.goto(`${url}?panel=settings&language=${language}`);
    expect(await page.evaluate(() => localStorage.getItem("pab.language"))).toBeNull();
    await page.locator(".settings-language").click();
    await page.locator(".ant-select-dropdown:visible").getByText(label, { exact: true }).click();
    await expect.poll(() => page.evaluate(() => localStorage.getItem("pab.language"))).toBe(language);
    await page.reload();
    expect(await page.evaluate(() => localStorage.getItem("pab.language"))).toBe(language);
  });
  for (const theme of ["light", "dark"]) {
    test(`long task titles retain the visible state in ${language}/${theme}`, async ({ page }) => {
      for (const embedded of [false, true]) {
        await page.goto(`${url}?language=${language}&theme=${theme}&embedded=${embedded}`);
        const row = page.locator(".task-row");
        await expect(row.locator("em")).toHaveText("Succeeded");
        const bounds = await row.evaluate(e => {
          const list = e.parentElement!;
          const state = e.querySelector("em")!.getBoundingClientRect();
          const title = e.querySelector("strong")!;
          return { overflow: list.scrollWidth - list.clientWidth,
            stateRight: state.right, listRight: list.getBoundingClientRect().right,
            titleRight: title.getBoundingClientRect().right, stateLeft: state.left,
            ellipsis: getComputedStyle(title).textOverflow };
        });
        expect(bounds.overflow).toBeLessThanOrEqual(1);
        expect(bounds.stateRight).toBeLessThanOrEqual(bounds.listRight);
        expect(bounds.titleRight).toBeLessThanOrEqual(bounds.stateLeft);
        expect(bounds.ellipsis).toBe("ellipsis");
      }
    });
  }
}

test("changing output encoding rereads the completed task and drops a delayed old reply", async ({ page }) => {
  await page.goto(`${url}?language=en&invalid=true`);
  await expect(page.getByText("Some bytes could not be decoded. Try another encoding.")).toBeVisible();
  await page.evaluate(() => { (window as any).fixture.delayGbk = true; });
  await page.getByLabel("Output encoding").click();
  await page.locator(".ant-select-dropdown:visible").getByText("GBK / CP936", { exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).fixture.releaseGbk)).toBe("function");
  await page.getByLabel("Output encoding").click();
  await page.locator(".ant-select-dropdown:visible").getByText("Big5 / CP950", { exact: true }).click();
  await expect(page.locator(".task-output pre")).toHaveText("繁體結果");
  await page.evaluate(() => (window as any).fixture.releaseGbk());
  await expect(page.locator(".task-output pre")).toHaveText("繁體結果");
  await expect(page.getByText("Some bytes could not be decoded. Try another encoding.")).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  expect(calls.every((call: any) => call.command === "operator_task")).toBeTruthy();
  expect(calls.filter((call: any) => ["gbk", "big5"].includes(call.args.encoding)).every((call: any) =>
    call.args.taskId === "long-title" && call.args.stdoutOffset === 0 && call.args.stderrOffset === 0)).toBeTruthy();
});

for (const language of ["zh-CN", "zh-TW", "en"]) {
  test(`server overrides stay empty by default and can be restored in ${language}`, async ({ page }) => {
    await page.goto(`${url}?panel=settings&section=server&language=${language}`);
    const fields = page.locator(".settings-fields input");
    await expect(fields.first()).toBeEnabled();
    await expect(fields.first()).toHaveValue("");
    await expect(fields.last()).toHaveValue("");
    await expect(page.locator("body")).not.toContainText("pab.rgaa.vip");
    const save = page.locator(".settings-actions button").first();
    await fields.first().fill("wss://custom.example/control");
    await save.click();
    await expect(page.getByRole("alert")).toBeVisible();
    expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "save_operator_server_settings").length)).toBe(0);
    await fields.last().fill("https://relay.example");
    await save.click();
    await expect(page.locator(".settings-actions button")).toHaveCount(2);
    await page.reload();
    await expect(fields.first()).toHaveValue("wss://custom.example/control");
    await expect(fields.last()).toHaveValue("https://relay.example");
    await fields.first().fill("");
    await fields.last().fill("");
    await save.click();
    await page.reload();
    await expect(fields.first()).toHaveValue("");
    await expect(fields.last()).toHaveValue("");
  });
}

test("server settings cannot overwrite an unreadable configuration", async ({ page }) => {
  await page.goto(`${url}?panel=settings&section=server&settingsError=true`);
  await expect(page.getByRole("alert")).toContainText("settings unavailable");
  await expect(page.locator(".settings-actions button").first()).toBeDisabled();
});

for (const language of ["zh-CN", "zh-TW", "en"]) {
  test(`server verification is independent and does not save in ${language}`, async ({ page }) => {
    await page.goto(`${url}?panel=settings&section=server&language=${language}`);
    const control = page.locator('[data-server-kind="control"]');
    const relay = page.locator('[data-server-kind="relay"]');
    await expect(control.getByRole("button")).toBeDisabled();
    await expect(relay.getByRole("button")).toBeDisabled();
    await control.locator("input").fill("wss://control.example/control");
    await control.getByRole("button").click();
    await expect(control.getByRole("status")).toContainText("42 ms");
    await expect(relay.locator(".settings-server-result")).toBeEmpty();
    await relay.locator("input").fill("https://fail.example");
    await relay.getByRole("button").click();
    await expect(relay.locator(".is-error")).toBeVisible();
    await expect(control.getByRole("status")).toContainText("42 ms");
    const calls = await page.evaluate(() => (window as any).fixture.calls);
    expect(calls.filter((c: any) => c.command === "validate_operator_server").map((c: any) => c.args.kind)).toEqual(["control", "relay"]);
    expect(calls.some((c: any) => ["save_operator_server_settings", "restart_desktop"].includes(c.command))).toBe(false);
    await relay.locator("input").fill("https://relay.example");
    await expect(relay.locator(".settings-server-result")).toBeEmpty();
    await relay.getByRole("button").click();
    await expect(relay.getByRole("status")).toContainText("42 ms");
    if (language === "zh-CN") await page.locator(".settings-server").screenshot({ path: fileURLToPath(new URL("../../../acceptance/server-probe-2026-10-15/settings-verified.png", import.meta.url)) });
  });
}

test("editing a server discards an older verification result", async ({ page }) => {
  await page.goto(`${url}?panel=settings&section=server&language=en&delayProbe=true`);
  const control = page.locator('[data-server-kind="control"]');
  await control.locator("input").fill("wss://old.example/control");
  await control.getByRole("button").click();
  await expect(control.getByRole("button")).toContainText("Verifying");
  await expect.poll(() => page.evaluate(() => typeof (window as any).fixture.releaseProbe)).toBe("function");
  await control.locator("input").fill("wss://fail.example/control");
  await control.getByRole("button").click();
  await expect(control.getByRole("status")).toContainText("Certificate validation failed");
  await page.evaluate(() => (window as any).fixture.releaseProbe());
  await expect(control.getByRole("status")).toContainText("Certificate validation failed");
});
