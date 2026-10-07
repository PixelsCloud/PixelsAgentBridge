import { test, expect } from "@playwright/test";
import { mergeTransferUpdate } from "../src/transferUpdates";

test("late transfer replies preserve terminal results, device and cancellation", () => {
  const current = { id: "first", deviceCode: "123456789", state: "completed" as const, offset: 20, size: 20, message: null };
  expect(mergeTransferUpdate(current, { ...current, state: "running", offset: 0 })).toEqual(current);
  expect(mergeTransferUpdate({ ...current, state: "cancel_requested" }, { ...current, state: "running", offset: 10 }).state).toBe("cancel_requested");
  expect(mergeTransferUpdate({ ...current, state: "unconfirmed" }, current).state).toBe("completed");
  expect(mergeTransferUpdate(current, { ...current, deviceCode: "987654321" })).toEqual(current);
});

test("transfer passes the selected user and observes uncertainty without resubmission", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/execution-ui.html");
  await page.getByRole("menuitem", { name: "File transfer", exact: true }).click();
  await page.getByRole("combobox", { name: "Execute as" }).click();
  await page.locator(".ant-select-dropdown:visible").getByText("Alice · Session 2", { exact: true }).click();
  const start = page.getByRole("button", { name: "Start transfer", exact: true });
  await start.click();
  await expect(start).toBeDisabled();
  await expect(page.getByText("Unconfirmed", { exact: true })).toBeVisible();
  await expect(page.locator(".transfer-status").getByText(/Alice/)).toBeVisible();
  await page.getByRole("button", { name: "Inspect original operation" }).click();
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  expect(calls.filter((c: any) => c.command === "ui_start_transfer")).toHaveLength(1);
  expect(calls.find((c: any) => c.command === "ui_start_transfer").args.execution.mode).toBe("user");
  expect(calls.find((c: any) => c.command === "ui_inspect_transfer").args.id).toBe("original-transfer");
  await page.evaluate(() => (window as any).fixture.transferState("cancel_requested"));
  await expect(start).toBeDisabled();
  await page.evaluate(() => (window as any).fixture.transferState("completed"));
  await expect(start).toBeEnabled();
});

const url = "http://127.0.0.1:1429/tests/execution-ui.html";
test("explicit user survives refresh as expired instead of reverting to service", async ({ page }) => {
  await page.goto(url);
  const run = page.getByRole("button", { name: "Run command", exact: true });
  await run.click();
  await page.getByRole("combobox", { name: "Execute as" }).click();
  await page.locator(".ant-select-dropdown:visible").getByText("Alice · Session 2", { exact: true }).click();
  await run.click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "ui_run_command").map((c: any) => c.args.execution.mode))).toEqual(["service", "user"]);
  await page.getByRole("button", { name: "Refresh available users" }).click();
  await expect(run).toBeDisabled();
  await expect(page.getByText("Selection expired. Choose again.").first()).toBeVisible();
  await page.getByRole("combobox", { name: "Execute as" }).click();
  await page.locator(".ant-select-dropdown:visible").getByText("Alice · Session 2", { exact: true }).click();
  await expect(run).toBeEnabled();
});

test("application mutations inspect the same ID without replay", async ({ page }) => {
  await page.goto(url);
  await expect(page.getByRole("menuitem", { name: "Windows", exact: true })).toBeVisible();
  await page.getByRole("menuitem", { name: "Applications", exact: true }).click();
  await page.getByRole("combobox", { name: "Desktop session" }).click();
  await page.locator(".ant-select-dropdown:visible").getByText("Alice · Session 2", { exact: true }).click();
  await page.getByRole("button", { name: "Find applications" }).click();
  await page.getByRole("table").getByRole("radio").check();
  await page.evaluate(() => { (window as any).fixture.rejectNext = true; });
  await page.getByRole("button", { name: "Launch application" }).click();
  await expect(page.getByText("fixture request rejected before submission")).toBeVisible();
  await expect(page.getByRole("button", { name: "Launch application" })).toBeEnabled();
  await page.getByRole("button", { name: "Launch application" }).click();
  await expect(page.getByRole("button", { name: "Launch application" })).toBeDisabled();
  await page.getByRole("button", { name: "Inspect original operation" }).click();
  await expect(page.getByText("Request accepted by the system")).toBeVisible();
  await page.getByRole("checkbox", { name: "Open a new app instance" }).check();
  await page.getByRole("button", { name: "Launch application" }).click();
  await page.getByRole("button", { name: "Inspect original operation" }).click();
  await expect(page.getByRole("button", { name: "Launch application" })).toBeEnabled();
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  const launches = calls.filter((c: any) => c.command === "operator_execution_query" && c.args.query.query?.action === "execute");
  expect(launches).toHaveLength(3); // pre-dispatch rejection, default launch, explicit new instance
  expect(launches[1].args.query.query.request.new_instance).toBeUndefined();
  expect(launches[2].args.query.query.request.new_instance).toBe(true);
  expect(calls.find((c: any) => c.command === "operator_execution_query_result").args.requestId).toBe(launches[1].args.requestId);
  await page.getByPlaceholder("Full path to the remote file").fill("/Users/alice/中文 文件.txt");
  await page.getByRole("checkbox", { name: "Open with the selected application" }).check();
  await page.getByRole("button", { name: "Open file", exact: true }).click();
  const opened = await page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.args?.query?.query?.request?.operation === "open_file").at(-1));
  expect(opened.args.query.query.request).toEqual({ operation: "open_file", path: "/Users/alice/中文 文件.txt", application: { kind: "id", id: "fixture.editor" } });
});

test("a delayed terminal open is closed when switching devices", async ({ page }) => {
  await page.goto(url);
  await page.getByRole("menuitem", { name: "Terminal", exact: true }).click();
  await page.evaluate(() => { (window as any).fixture.delayTerminal = true; });
  await page.getByRole("button", { name: "Open terminal" }).click();
  await expect.poll(() => page.evaluate(() => !!(window as any).fixture.releaseTerminal)).toBe(true);
  const id = await page.evaluate(() => { const f = (window as any).fixture; f.switchDevice(); return f.terminalReply.sessionId; });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.revision)).toBe(2);
  await page.evaluate(() => (window as any).fixture.releaseTerminal());
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: any) => c.command === "operator_terminal_close").map((c: any) => c.args.id))).toEqual([id]);
});

for (const language of ["en", "zh-CN", "zh-TW"]) for (const theme of ["light", "dark"]) {
  test(`application layout ${language} ${theme}`, async ({ page }, info) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(`${url}?language=${language}&theme=${theme}`);
    const applications = page.getByRole("menuitem", { name: language === "en" ? "Applications" : language === "zh-TW" ? "應用管理" : "应用管理" });
    await applications.click();
    await expect(applications).toHaveClass(/ant-menu-item-selected/);
    await expect(page.locator(".application-browser")).toBeVisible();
    await page.screenshot({ path: info.outputPath(`${language}-${theme}.png`), fullPage: true, animations: "disabled" });
    expect(errors).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}
