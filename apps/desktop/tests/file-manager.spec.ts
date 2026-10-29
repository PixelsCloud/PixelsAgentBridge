import { expect, test } from "@playwright/test";
import { TransferRateTracker, remainingTime, unknownTransferRate, type RateSnapshot } from "../src/fileTransferRate";
import { directoryAffected, directoryChangeKey, type DirectoryTransfer } from "../src/fileDirectoryRefresh";
import { pathLabel, utf8Path } from "../src/fileManagerApi";

const rateSnapshot = (revision: number, confirmed: bigint, phase = "running", size = 10000n, scan_complete = true): RateSnapshot => ({ spec: { id: "rate-fixture" }, revision: String(revision), phase, confirmed: String(confirmed), size: String(size), scan_complete });

test("rate estimates preserve large counters and ignore an older paused snapshot", () => {
  const rates = new TransferRateTracker(), baseline = 9007199254740993n, size = baseline + 5000n;
  expect(rates.observe(rateSnapshot(1, baseline, "running", size), 0)).toEqual(unknownTransferRate);
  expect(rates.observe(rateSnapshot(2, baseline + 1000n, "running", size), 1000)).toEqual({ bytesPerSecond: 1000n, etaSeconds: 4n });
  expect(rates.observe(rateSnapshot(1, baseline, "paused", size), 1100)).toEqual({ bytesPerSecond: 1000n, etaSeconds: 4n });
});

test("rate segments exclude paused time and require new progress after a stalled acknowledgement", () => {
  const rates = new TransferRateTracker();
  rates.observe(rateSnapshot(1, 0n), 0);
  expect(rates.observe(rateSnapshot(2, 1000n), 1000).bytesPerSecond).toBe(1000n);
  expect(rates.observe(rateSnapshot(3, 1000n, "pause_requested"), 1500)).toEqual(unknownTransferRate);
  rates.observe(rateSnapshot(4, 1000n, "paused"), 2000);
  expect(rates.observe(rateSnapshot(5, 1000n), 9000)).toEqual(unknownTransferRate);
  expect(rates.observe(rateSnapshot(6, 2000n), 10000)).toEqual({ bytesPerSecond: 1000n, etaSeconds: 8n });
  expect(rates.fresh("rate-fixture", 13000)).toBe(false);
  expect(rates.observe(rateSnapshot(6, 2000n), 13001)).toEqual(unknownTransferRate);
  expect(rates.observe(rateSnapshot(7, 2100n), 14000)).toEqual(unknownTransferRate);
  expect(rates.observe(rateSnapshot(8, 2200n), 15000)).toEqual({ bytesPerSecond: 100n, etaSeconds: 78n });
});

test("the bounded rate window drops old bursts and resets on a counter restart or lost observation gap", () => {
  const rates = new TransferRateTracker();
  rates.observe(rateSnapshot(1, 0n, "running", 100000n), 0);
  rates.observe(rateSnapshot(2, 10000n, "running", 100000n), 1000);
  for (let second = 2; second <= 7; second++) rates.observe(rateSnapshot(second + 1, 9000n + BigInt(second) * 1000n, "running", 100000n), second * 1000);
  expect(rates.observe(rateSnapshot(9, 17000n, "running", 100000n), 8000).bytesPerSecond).toBe(1000n);
  expect(rates.observe(rateSnapshot(10, 100n), 9000)).toEqual(unknownTransferRate);
  expect(rates.observe(rateSnapshot(11, 200n), 10000).bytesPerSecond).toBe(100n);
  expect(rates.observe(rateSnapshot(12, 5000n), 20000)).toEqual(unknownTransferRate);
  rates.retain([]);
  expect(rates.fresh("rate-fixture", 20001)).toBe(false);
});

test("ETA stays unknown while scanning or awaiting publication and formats bounded readable durations", () => {
  const rates = new TransferRateTracker();
  rates.observe(rateSnapshot(1, 0n), 0);
  expect(rates.observe(rateSnapshot(2, 1000n, "running", 10000n, false), 1000)).toEqual({ bytesPerSecond: 1000n, etaSeconds: null });
  expect(rates.observe(rateSnapshot(3, 2000n, "running", 2000n), 2000).etaSeconds).toBeNull();
  expect(rates.observe(rateSnapshot(4, 2000n, "verifying", 2000n), 2500)).toEqual(unknownTransferRate);
  expect(remainingTime(null, true)).toBe("—"); expect(remainingTime(199n, true)).toBe("3m 19s");
  expect(remainingTime(3660n, false)).toBe("1小时1分钟"); expect(remainingTime(900000000000n, true)).toBe(">99d");
});
test.beforeEach(async ({ page }) => { await page.goto("http://127.0.0.1:1429/tests/file-manager.html"); await expect(page.getByRole("region", { name: "This computer", exact: true }).getByRole("status")).toContainText("100000 items"); });

test("directory invalidation compares native components including raw names and Windows wide paths", () => {
  const wide = (value: string) => ({ encoding: "windows_wide" as const, value: Buffer.from(value, "utf16le").toString("base64") });
  const raw = (values: number[]) => ({ encoding: "unix_bytes" as const, value: Buffer.from(values).toString("base64") });
  expect(directoryAffected(utf8Path("c:\\Users\\测试"), wide("C:\\USERS\\测试\\new"), "windows")).toBe(true);
  expect(directoryAffected(wide("C:\\Users\\测试\\new"), utf8Path("c:/Users/测试/"), "windows")).toBe(true);
  expect(directoryAffected(utf8Path("C:\\Fixtures2"), utf8Path("C:\\Fixtures"), "windows")).toBe(false);
  expect(directoryAffected(utf8Path("D:\\Fixtures"), utf8Path("C:\\Fixtures"), "windows")).toBe(false);
  expect(directoryAffected(utf8Path("\\\\host\\share2"), wide("\\\\host\\share"), "windows")).toBe(false);
  expect(directoryAffected(raw([47, 255]), raw([47, 255, 47, 97]), "posix")).toBe(true);
  expect(directoryAffected(raw([47, 255]), raw([47, 254]), "posix")).toBe(false);
  expect(directoryAffected(utf8Path("/中文"), raw([...Buffer.from("/中文/new")]), "posix")).toBe(true);
  expect(directoryAffected(utf8Path("/a"), utf8Path("/A"), "posix")).toBe(false);
  expect(directoryAffected(utf8Path("/a"), utf8Path("/ab"), "posix")).toBe(false);
  expect(directoryAffected(utf8Path("/"), utf8Path("/tmp"), "posix")).toBe(true);
  expect(directoryAffected(utf8Path("/a"), utf8Path("/a/../b"), "posix")).toBe(true);
});

test("directory invalidation keys ignore byte-only progress and isolate the receiving endpoint", () => {
  const batch: DirectoryTransfer = { spec: { id: "one", code: "428358431", direction: "upload", destination: utf8Path("C:\\Fixtures") }, completed: "0", phase: "running" };
  expect(directoryChangeKey([batch], "428358431", utf8Path("C:\\Fixtures"), "windows")).toBe("[]");
  const completed = { ...batch, completed: "9007199254740993" };
  const key = directoryChangeKey([completed], "428358431", utf8Path("C:\\Fixtures"), "windows");
  expect(key).toContain("9007199254740993");
  expect(directoryChangeKey([{ ...completed, phase: "publishing" }], "428358431", utf8Path("C:\\Fixtures"), "windows")).toBe(key);
  expect(directoryChangeKey([{ ...completed, phase: "completed" }], "428358431", utf8Path("C:\\Fixtures"), "windows")).not.toBe(key);
  expect(directoryChangeKey([completed], "local", utf8Path("C:\\Fixtures"), "windows")).toBe("[]");
  expect(directoryChangeKey([completed], "559038160", utf8Path("C:\\Fixtures"), "windows")).toBe("[]");
  expect(directoryChangeKey([completed], "428358431", utf8Path("D:\\Other"), "windows")).toBe("[]");
  expect(directoryChangeKey([{ ...completed, spec: { ...completed.spec, direction: "download" } }], "local", utf8Path("C:\\Fixtures"), "windows")).not.toBe("[]");
});

test("byte polling never rescans and completed updates wait for a dialog and coalesce", async ({ page }) => {
  await page.clock.install();
  const remote = page.getByRole("region", { name: "90", exact: true });
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await page.getByRole("region", { name: "This computer", exact: true }).getByRole("checkbox", { name: "Select file-000001.bin", exact: true }).check();
  await page.getByRole("button", { name: "Send (1)", exact: true }).click();
  const scans = () => page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.args.operation?.action === "open_directory").length);
  const before = await scans();
  for (const value of [600, 700, 800]) {
    await page.evaluate(value => { const batch = (window as any).fixture.batches[0]; batch.confirmed = String(value); batch.revision = String(value); }, value);
    await page.clock.runFor(1000);
  }
  expect(await scans()).toBe(before);
  await remote.getByRole("checkbox", { name: "Select file-000002.bin", exact: true }).check();
  await remote.getByTitle("Properties", { exact: true }).click();
  for (const count of [1, 2, 3]) {
    await page.evaluate(count => { const batch = (window as any).fixture.batches[0]; batch.completed = String(count); batch.total = "4"; batch.revision = String(1000 + count); }, count);
    await page.clock.runFor(1000);
  }
  await expect(page.locator(".fm-task")).toContainText("3 / 4");
  expect(await scans()).toBe(before);
  await page.getByRole("dialog", { name: "Properties", exact: true }).getByTitle("Cancel", { exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Properties", exact: true })).toHaveCount(0);
  await page.clock.runFor(1000);
  expect(await scans()).toBe(before + 1);
  await page.clock.runFor(3000);
  expect(await scans()).toBe(before + 1);
  await expect(remote.getByRole("checkbox", { name: "Select file-000002.bin", exact: true })).toBeChecked();
});

for (const direction of ["upload", "download"] as const) test(`a completed ${direction} refreshes only its destination and preserves selection through a slow scan`, async ({ page }) => {
  const local = page.getByRole("region", { name: "This computer", exact: true });
  const remote = page.getByRole("region", { name: "90", exact: true });
  const source = direction === "upload" ? local : remote, target = direction === "upload" ? remote : local;
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await source.getByRole("checkbox", { name: "Select file-000001.bin", exact: true }).check();
  await target.getByRole("checkbox", { name: "Select file-000002.bin", exact: true }).check();
  await target.getByRole("listbox").evaluate(element => { element.scrollTop = 38; element.dispatchEvent(new Event("scroll")); });
  await page.getByRole("button", { name: `${direction === "upload" ? "Send" : "Receive"} (1)`, exact: true }).click();
  await expect(page.locator(".fm-task")).toHaveCount(1);
  const before = await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.args.operation?.action === "open_directory").length);
  await page.evaluate(() => { const fixture = (window as any).fixture; fixture.holdNextScan = true; fixture.entryNameOverrides[3] = "newly-received.txt"; const batch = fixture.batches[0]; batch.phase = "completed"; batch.completed = "1"; batch.confirmed = batch.size; batch.revision = "2"; });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.held.length)).toBe(1);
  await expect(target.getByText("file-000002.bin", { exact: true })).toBeVisible();
  await expect(target.getByRole("checkbox", { name: "Select file-000002.bin", exact: true })).toBeChecked();
  await page.evaluate(() => (window as any).fixture.releaseScans());
  await expect(target.getByText("newly-received.txt", { exact: true })).toBeVisible();
  await expect(target.getByRole("checkbox", { name: "Select file-000002.bin", exact: true })).toBeChecked();
  await expect(source.getByText("newly-received.txt", { exact: true })).toHaveCount(0);
  expect(await target.getByRole("listbox").evaluate(element => element.scrollTop)).toBe(38);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.args.operation?.action === "open_directory").length)).toBe(before + 1);
});

for (const rejected of [false, true]) test(`account end clears private panes and dialogs and ignores late ${rejected ? "failure" : "success"} replies`, async ({ page }) => {
  await page.evaluate(rejected => {
    const fixture = (window as any).fixture;
    fixture.holdBatchReply = true; fixture.rejectHeldBatch = rejected;
    fixture.items.push({ spec: { id: "private-item", relative_name: "private-account.txt", source: { encoding: "utf8", value: "C:\\private\\private-account.txt" }, destination: { encoding: "utf8", value: "D:\\private\\private-account.txt" }, size: "12" }, state: "running", confirmed: "0", message: null, attempt: "old-account-attempt" });
  }, rejected);
  await page.getByRole("region", { name: "This computer", exact: true }).getByRole("checkbox", { name: "Select file-000001.bin", exact: true }).check();
  await page.getByRole("button", { name: "Send (1)", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.batchReplyWaiters.length)).toBe(1);
  await page.getByTitle("Transfer details", { exact: true }).click();
  await expect(page.getByRole("dialog").getByText("private-account.txt", { exact: true })).toBeVisible();
  await page.evaluate(() => { (window as any).fixture.holdBatchPoll = true; });
  await expect.poll(() => page.evaluate(() => (window as any).fixture.batchPollWaiters.length)).toBeGreaterThan(0);
  await page.evaluate(() => (window as any).fixture.emit("file-manager-account-ended", null));
  const ended = "Account changed. Reopen file transfer under the current account.";
  await expect(page.getByRole("alert")).toHaveText(ended);
  await expect(page.locator(".fm-panes")).toHaveCount(0);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByRole("tab")).toHaveCount(0);
  await expect(page.locator(".fm-task, .fm-submission")).toHaveCount(0);
  await expect(page.getByText("Native local fixture user", { exact: true })).toHaveCount(0);
  await page.evaluate(() => {
    const fixture = (window as any).fixture;
    fixture.batchReplyWaiters.splice(0).forEach((done: () => void) => done());
    fixture.batchPollWaiters.splice(0).forEach((done: () => void) => done());
    fixture.emit("file-manager-open-tab", { code: "559038160", name: "Private late tab", os_family: "windows" });
  });
  // A browser task barrier lets both old promise continuations and React commits finish.
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await expect(page.getByRole("alert")).toHaveText(ended);
  await expect(page.locator(".fm-task, .fm-submission, .fm-panes")).toHaveCount(0);
  await expect(page.getByRole("tab")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send", exact: true })).toHaveCount(0);
  // UI cleanup must not rewrite durable backend history or submit another transfer.
  expect(await page.evaluate(() => (window as any).fixture.batches.length)).toBe(1);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "file_manager_start_batch").length)).toBe(1);
});

test("visible transfer estimates clear on pause, acknowledgement stalls and a hung status request", async ({ page }) => {
  await page.clock.install();
  const local = page.getByRole("region", { name: "This computer", exact: true }), tasks = page.getByRole("region", { name: "Transfers", exact: true });
  await local.getByRole("checkbox", { name: "Select file-000001.bin", exact: true }).check();
  await page.getByRole("button", { name: "Send (1)", exact: true }).click();
  const speed = tasks.getByLabel("Transfer speed", { exact: true }), eta = tasks.getByLabel("Estimated remaining", { exact: true });
  await expect(speed).toHaveText("—");
  await page.evaluate(() => { const batch = (window as any).fixture.batches[0]; batch.confirmed = "750"; batch.revision = "2"; });
  await page.clock.runFor(1000);
  await expect(speed).toHaveText(/B\/s$/); await expect(eta).toHaveText(/~\d+s/);
  await tasks.getByTitle("Pause", { exact: true }).click();
  await expect(speed).toHaveText("—"); await expect(eta).toHaveText("—");
  await page.evaluate(() => (window as any).fixture.ack("pause")); await page.clock.runFor(750);
  await tasks.getByTitle("Resume", { exact: true }).click(); await page.clock.runFor(5000);
  await expect(speed).toHaveText("—"); await expect(eta).toHaveText("—");
  for (const confirmed of [1000, 1250]) {
    await page.evaluate(value => { const batch = (window as any).fixture.batches[0]; batch.size = "2000"; batch.confirmed = String(value); batch.revision = String(BigInt(batch.revision) + 1n); }, confirmed);
    await page.clock.runFor(750);
  }
  await expect(speed).toHaveText(/B\/s$/);
  await page.evaluate(() => { (window as any).fixture.holdBatchPoll = true; }); await page.clock.runFor(3500);
  await expect(speed).toHaveText("—"); await expect(eta).toHaveText("—");
  await page.evaluate(() => { const fixture = (window as any).fixture; fixture.holdBatchPoll = false; fixture.batchPollWaiters.splice(0).forEach((done: () => void) => done()); });
});
test("active transfer time uses durable snapshots and never advances from a UI timer", async ({ page }) => {
  await page.clock.install();
  const local=page.getByRole("region",{name:"This computer",exact:true}),tasks=page.getByRole("region",{name:"Transfers",exact:true});
  await local.getByRole("checkbox",{name:"Select file-000001.bin",exact:true}).check();
  await page.getByRole("button",{name:"Send (1)",exact:true}).click();
  const elapsed=tasks.getByLabel("Active transfer time",{exact:true});
  await expect(elapsed).toHaveText("Active —");
  await page.evaluate(()=>{const batch=(window as any).fixture.batches[0];batch.active_elapsed_ms="3660999";batch.revision="2";});
  await page.clock.runFor(1000);await expect(elapsed).toHaveText("Active 1h 1m");
  await tasks.getByTitle("Pause",{exact:true}).click();
  await page.evaluate(()=>(window as any).fixture.ack("pause"));
  await page.clock.runFor(120_000);await expect(elapsed).toHaveText("Active 1h 1m");
  await page.evaluate(()=>{const batch=(window as any).fixture.batches[0];batch.active_elapsed_ms="9007199254740993";batch.revision=String(BigInt(batch.revision)+1n);});
  await page.clock.runFor(1000);await expect(elapsed).toHaveText("Active >99d");
  await expect(elapsed).not.toContainText("ms");
});

test("dual panes bound their DOM, paginate indexed rows, and filter without rescanning", async ({ page }) => {
  const local = page.getByRole("region", { name: "This computer", exact: true });
  await expect(local.getByRole("option")).toHaveCount(32);
  await expect(local.getByText("8.00 PiB", { exact: true }).first()).toBeVisible();
  await local.getByTitle("Next page", { exact: true }).click();
  await expect(local.getByText("file-000256.bin", { exact: true })).toBeVisible();
  const scans = await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.args.operation?.action === "open_directory").length);
  await local.getByRole("textbox", { name: "This computer Search this folder" }).fill("file-099999");
  await expect(local.getByText("file-099999.bin", { exact: true })).toBeVisible();
  await expect(local.getByRole("status")).toContainText("1 items");
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.args.operation?.action === "open_directory").length)).toBe(scans);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy();
});
test("a cancelled directory result arriving late never replaces the active folder", async ({ page }) => {
  const local = page.getByRole("region", { name: "This computer", exact: true });
  await page.evaluate(() => { (window as any).fixture.holdNextScan = true; });
  await local.getByText("Folder", { exact: true }).dblclick();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.held.length)).toBe(1);
  await local.getByTitle("Back", { exact: true }).click();
  await expect(local.getByRole("status")).toContainText("100000 items");
  const held = await page.evaluate(() => (window as any).fixture.held[0].id);
  await page.evaluate(() => (window as any).fixture.releaseScans());
  await expect.poll(() => page.evaluate(id => (window as any).fixture.closed.includes(id), held)).toBeTruthy();
  await expect(local.locator(".fm-breadcrumbs")).toContainText("Fixtures");
  await expect(local.getByText("Folder", { exact: true })).toBeVisible();
});
test("pause and cancel stay requested until the receiver acknowledgement, and file profiles remain separate", async ({ page }) => {
  const local = page.getByRole("region", { name: "This computer", exact: true });
  await local.getByRole("checkbox", { name: "Select file-000001.bin", exact: true }).check();
  await page.getByRole("button", { name: "Send (1)", exact: true }).click();
  const tasks = page.getByRole("region", { name: "Transfers", exact: true });
  await tasks.getByTitle("Pause", { exact: true }).click();
  await expect(tasks).toContainText("Pausing…");
  await expect(tasks).not.toContainText("Paused");
  await page.evaluate(() => (window as any).fixture.ack("pause"));
  await expect(tasks).toContainText("Paused");
  await tasks.getByTitle("Resume", { exact: true }).click();
  await tasks.getByTitle("Cancel", { exact: true }).click();
  await expect(tasks).toContainText("Cancelling…");
  await page.evaluate(() => (window as any).fixture.ack("cancel"));
  await expect(tasks).toContainText("Cancelled");
  await page.locator(".fm-remote-tab:not([hidden]) .fm-relay-toggle input").evaluate((input: HTMLInputElement) => input.click());
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "file_manager_contexts" && call.args.forceRelay))).toBeTruthy();
  expect(await page.evaluate(() => (window as any).fixture.calls.some((call: any) => /disconnect|set_connection_mode/.test(call.command)))).toBeFalsy();
});

test("changing file profiles preserves the native user and never falls back to service when unavailable", async ({ page }) => {
  const remote = page.getByRole("region", { name: "90", exact: true });
  const account = page.locator(".fm-remote-tab:not([hidden])");
  const relay = page.locator(".fm-remote-tab:not([hidden]) .fm-relay-toggle input");
  const local = page.getByRole("region", { name: "This computer", exact: true });
  await expect(account).toHaveAttribute("data-execution-ref", "user-auto");
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await local.getByRole("checkbox", { name: "Select file-000001.bin", exact: true }).check();
  await expect(page.getByRole("button", { name: "Send (1)", exact: true })).toBeEnabled();
  await page.evaluate(() => { (window as any).fixture.calls = []; });
  await relay.evaluate((input: HTMLInputElement) => input.click());
  await expect(account).toHaveAttribute("data-execution-ref", "user-relay");
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await expect(page.getByRole("button", { name: "Send (1)", exact: true })).toBeEnabled();
  const calls = await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "file_manager_request" && call.args.endpoint === "428358431"));
  expect(calls.length).toBeGreaterThan(0);
  expect(calls.every((call: any) => call.args.execution.mode === "user")).toBeTruthy();
  await relay.evaluate((input: HTMLInputElement) => input.click());
  await expect(account).toHaveAttribute("data-execution-ref", "user-auto");
  await page.evaluate(() => { (window as any).fixture.hideRelayUser = true; (window as any).fixture.calls = []; });
  await relay.evaluate((input: HTMLInputElement) => input.click());
  await expect(account).toHaveAttribute("data-execution-ref", "__unavailable");
  await expect(page.getByRole("button", { name: "Send (1)", exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "file_manager_request" && call.args.endpoint === "428358431" && call.args.forceRelay && call.args.operation.action !== "close_directory"))).toBeFalsy();
  await relay.evaluate((input: HTMLInputElement) => input.click());
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await expect(page.getByRole("button", { name: "Send (1)", exact: true })).toBeEnabled();
});
test("a native user reconnect renews the reference and keeps the current directory without service fallback", async ({ page }) => {
  const remote = page.getByRole("region", { name: "90", exact: true });
  const account = page.locator(".fm-remote-tab:not([hidden])");
  await expect(account).toHaveAttribute("data-execution-ref", "user-auto");
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await remote.getByText("Folder", { exact: true }).dblclick();
  await expect(remote.getByRole("status")).toContainText("0 items");
  const before = await remote.locator('.fm-breadcrumbs').textContent();
  await page.evaluate(() => { const f = (window as any).fixture; f.nativeGeneration = 1; f.calls = []; });
  await remote.getByTitle("Refresh", { exact: true }).click();
  await expect(account).toHaveAttribute("data-execution-ref", "user-auto-1");
  await expect(remote.locator('.fm-error')).toHaveCount(0);
  await expect(remote.getByRole("status")).toContainText("0 items");
  await expect(remote.locator('.fm-breadcrumbs')).toHaveText(before || '');
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  expect(calls.some((call: any) => call.command === "file_manager_contexts")).toBeTruthy();
  expect(calls.some((call: any) => call.command === "file_manager_request" && call.args.endpoint === "428358431" && call.args.execution.context_ref === "user-auto-1" && call.args.operation.action === "open_directory")).toBeTruthy();
  expect(calls.some((call: any) => call.command === "file_manager_request" && call.args.endpoint === "428358431" && call.args.execution.mode === "service")).toBeFalsy();
  expect(calls.some((call: any) => call.command === "file_manager_start_batch")).toBeFalsy();
});

test("a changed native login becomes unavailable after reconnect and never reuses the account name", async ({ page }) => {
  const remote = page.getByRole("region", { name: "90", exact: true });
  const account = page.locator(".fm-remote-tab:not([hidden])");
  await expect(account).toHaveAttribute("data-execution-ref", "user-auto");
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await page.evaluate(() => { const f = (window as any).fixture; f.nativeGeneration = 1; f.nativeLogon = "different-login"; f.calls = []; });
  await remote.getByTitle("Refresh", { exact: true }).click();
  await expect(account).toHaveAttribute("data-execution-ref", "__unavailable");
  await expect(remote.getByTitle("New folder", { exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "file_manager_request" && call.args.endpoint === "428358431" && call.args.execution.mode === "service"))).toBeFalsy();
});

test("unchanged rejected references do not cause repeated discovery or directory refresh storms", async ({ page }) => {
  const remote = page.getByRole("region", { name: "90", exact: true });
  await expect(page.locator(".fm-remote-tab:not([hidden])")).toHaveAttribute("data-execution-ref", "user-auto");
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await page.evaluate(() => { const f = (window as any).fixture; f.rejectNativeReads = true; f.calls = []; });
  await remote.getByTitle("Refresh", { exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "file_manager_contexts").length)).toBe(1);
  await expect(remote.locator('.fm-error')).toContainText("The selected user session is no longer available.");
  await page.waitForTimeout(2500);
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  expect(calls.filter((call: any) => call.command === "file_manager_contexts")).toHaveLength(1);
  expect(calls.filter((call: any) => call.command === "file_manager_request" && call.args.endpoint === "428358431" && call.args.operation.action === "open_directory")).toHaveLength(1);
});

test("renewing a native context retains an unknown mutation and only inspects its original request", async ({ page }) => {
  const remote = page.getByRole("region", { name: "90", exact: true });
  const account = page.locator(".fm-remote-tab:not([hidden])");
  await expect(account).toHaveAttribute("data-execution-ref", "user-auto");
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await remote.getByRole("checkbox", { name: "Select file-000001.bin", exact: true }).check();
  await page.evaluate(() => { const f = (window as any).fixture; f.loseDeleteSuffix = "file-000001.bin"; f.calls = []; });
  await remote.getByTitle("Delete", { exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Delete", exact: true });
  await dialog.getByRole("button", { name: "Confirm", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Inspect original request", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Confirm", exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "file_manager_contexts"))).toBeFalsy();
  const original = await page.evaluate(() => (window as any).fixture.calls.find((call: any) => call.command === "file_manager_request" && call.args.operation.action === "delete").args.requestId);
  await dialog.getByRole("button", { name: "Cancel", exact: true }).last().click();
  await page.evaluate(() => { (window as any).fixture.nativeGeneration = 1; });
  await remote.getByTitle("Refresh", { exact: true }).click();
  await expect(account).toHaveAttribute("data-execution-ref", "user-auto-1");
  await expect(remote.locator('.fm-error')).toHaveCount(0);
  await expect(remote.getByRole("status")).toContainText("100000 items");
  await remote.getByTitle("Delete", { exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Confirm", exact: true })).toBeDisabled();
  await dialog.getByRole("button", { name: "Inspect original request", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).fixture.calls);
  expect(calls.filter((call: any) => call.command === "file_manager_request" && call.args.operation.action === "delete")).toHaveLength(1);
  expect(calls.some((call: any) => call.command === "file_manager_status" && call.args.requestId === original)).toBeTruthy();
});

test("transfer details resolve the original conflict once, retain the list during updates and restore focus", async ({ page }) => {
  const previous = page.getByRole("tab").first();
  await previous.focus();
  await page.evaluate(() => {
    const fixture = (window as any).fixture;
    fixture.batches.push({ spec: { id: "batch-conflict", code: "428358431", direction: "upload" }, phase: "waiting_conflict", intent: "run", revision: "1", total: "1", completed: "0", skipped: "0", failed: "0", cancelled: "0", size: "12", confirmed: "0", scan_complete: true, message: null });
    fixture.items.push({ spec: { id: "original-item", relative_name: "folder/中文.txt", source: { encoding: "utf8", value: "C:\\source\\中文.txt" }, destination: { encoding: "utf8", value: "D:\\destination\\中文.txt" }, size: "12" }, state: "waiting_conflict", confirmed: "0", message: "The destination already exists", attempt: "original-attempt" });
  });
  const dialog = page.getByRole("dialog", { name: "Transfer details", exact: true });
  await expect(dialog.getByText("folder/中文.txt", { exact: true })).toBeVisible();
  await expect(dialog.getByText("original-attempt", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("combobox", { name: "Item status" })).toHaveValue("waiting_conflict");
  await dialog.getByRole("button", { name: "Keep both", exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await expect(previous).toBeFocused();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "file_manager_resolve_conflict").map((call: any) => call.args))).toEqual([{ batch: "batch-conflict", item: "original-item", choice: "keep_both", applyRemaining: false }]);
  await expect(page.locator('.fm-task')).toContainText('Transferring');
  await page.getByTitle("Transfer details", { exact: true }).click();
  await expect(dialog.getByText("folder/中文.txt", { exact: true })).toBeVisible();
  await expect(dialog.locator(".fm-batch-item").getByText("Queued", { exact: true })).toBeVisible();
  await expect(dialog.getByText("D:\\destination\\中文 (1).txt",{exact:true})).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();

});

test("directories remain pending until their child barrier and attribute finalization finish", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).fixture;
    fixture.batches.push({ spec: { id: "directory-barrier", code: "428358431", direction: "upload" }, phase: "queued", intent: "run", revision: "1", total: "2", completed: "1", skipped: "0", failed: "0", cancelled: "0", size: "12", confirmed: "12", scan_complete: true, message: null });
    fixture.items.push({ spec: { id: "directory-item", kind: "directory", relative_name: "folder", source: { encoding: "utf8", value: "C:\\source\\folder" }, destination: { encoding: "utf8", value: "D:\\destination\\folder" }, size: "0" }, state: "waiting_children", confirmed: "0", message: null, attempt: "directory-created" });
  });
  await page.getByTitle("Transfer details", { exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Transfer details", exact: true });
  await expect(dialog.locator(".fm-batch-item").getByText("Waiting for child items", { exact: true })).toBeVisible();
  await expect(dialog.locator(".fm-batch-item").getByText("Completed", { exact: true })).not.toBeVisible();
  await page.evaluate(() => {
    const fixture = (window as any).fixture;
    fixture.items[0].state = "completed"; fixture.items[0].attempt = "directory-finalized";
    fixture.batches[0].phase = "completed"; fixture.batches[0].completed = "2"; fixture.batches[0].revision = "2";
  });
  await expect(dialog.locator(".fm-batch-item").getByText("Completed", { exact: true })).toBeVisible();
  await expect(dialog.getByText("directory-finalized", { exact: true })).toBeVisible();
});

test("completed file details preserve typed attribute warnings during polling", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).fixture;
    fixture.batches.push({ spec: { id: "attributes", code: "428358431", direction: "upload" }, phase: "completed", intent: "run", revision: "1", total: "1", completed: "1", skipped: "0", failed: "0", cancelled: "0", size: "12", confirmed: "12", scan_complete: true, message: null });
    fixture.items.push({ spec: { id: "attributes-item", relative_name: "file.txt", source: { encoding: "utf8", value: "C:\\source\\file.txt" }, destination: { encoding: "utf8", value: "D:\\destination\\file.txt" }, size: "12" }, state: "completed", confirmed: "12", message: null, attempt: "attributes-original", metadata_result: { requested: { modified_at_unix_ms: 1700000000123, unix_mode: 416 }, actual: { modified_at_unix_ms: 1700000000123, unix_mode: null }, warnings: [{ attribute: "unix_mode", code: "unsupported", message: "ordinary POSIX mode is unavailable" }] } });
  });
  await page.getByTitle("Transfer details", { exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Transfer details", exact: true });
  await expect(dialog.getByText("Completed with attribute warnings", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("note")).toContainText("Permissions");
  await expect(dialog.getByRole("note")).toContainText("file contents were saved");
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "file_manager_batch_items").length)).toBeGreaterThan(1);
  await expect(dialog.getByText("attributes-original", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("note")).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "file_manager_resolve_conflict"))).toBeFalsy();
});
test("task area resizes and collapses without remounting panes, and retries only the chosen batch", async ({ page }) => {
  await page.setViewportSize({ width: 960, height: 640 });
  const scans = await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.args.operation?.action === "open_directory").length);
  const divider = page.getByRole("separator", { name: "Resize transfers" });
  await divider.focus(); await page.keyboard.press("ArrowUp");
  await expect(divider).toHaveAttribute("aria-valuenow", "210");
  await page.getByTitle("Hide transfers", { exact: true }).click();
  await expect(divider).not.toBeVisible();
  await page.getByTitle("Show transfers", { exact: true }).click();
  await expect(divider).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.args.operation?.action === "open_directory").length)).toBe(scans);
  await page.evaluate(() => (window as any).fixture.batches.push({ spec: { id: "retry-one", code: "428358431", direction: "upload" }, phase: "partial_failed", intent: "run", revision: "1", total: "4", completed: "2", skipped: "1", failed: "1", cancelled: "0", size: "100", confirmed: "90", scan_complete: true, message: null, runner_active: false }));
  await page.getByTitle("Retry failed files", { exact: true }).click();
  await expect(page.getByRole("region", { name: "Transfers", exact: true })).toContainText("Transferring");
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "file_manager_retry_failed").map((call: any) => call.args))).toEqual([{ id: "retry-one" }]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBeTruthy();
});
test("native drops use an owned token and frozen destination; a lost reply cannot create another batch", async ({ page }) => {
  await expect(page.getByRole("region", { name: "This computer", exact: true })).toBeVisible();
  await page.evaluate(() => {
    const pane = document.querySelector(".fm-remote-panes")!;
    const transfer = new DataTransfer(); transfer.setData("text/uri-list", "file:///C:/forged-path");
    pane.dispatchEvent(new DragEvent("drop", { dataTransfer: transfer, bubbles: true }));
  });
  expect(await page.evaluate(() => (window as any).fixture.calls.some((call: any) => call.command === "file_manager_start_drop"))).toBeFalsy();
  await page.evaluate(() => {
    const fixture = (window as any).fixture, bounds = document.querySelector(".fm-remote-panes")!.getBoundingClientRect();
    fixture.nativeDrops.add("os-native-drop"); fixture.nativeDropLostReply = true;
    fixture.emit("file-manager-native-drop", { drop_id: "os-native-drop", count: 2, names: ["first.bin", "folder"], x: (bounds.left + 30) * devicePixelRatio, y: (bounds.top + 30) * devicePixelRatio });
  });
  const dialog = page.getByRole("dialog", { name: "Send dropped files", exact: true });
  await expect(dialog).toContainText("90");
  await page.evaluate(() => (window as any).fixture.emit("file-manager-open-tab", { code: "947158375", name: "XRServer", os_family: "windows" }));
  await dialog.getByRole("button", { name: "Send files", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("The operation result is not confirmed.");
  await dialog.getByRole("button", { name: "Send files", exact: true }).click();
  await expect(dialog).not.toBeVisible();
  const calls = await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "file_manager_start_drop").map((call: any) => call.args));
  expect(calls).toHaveLength(2); expect(calls[0]).toEqual(calls[1]);
  expect(calls[0].code).toBe("428358431"); expect(calls[0].dropId).toBe("os-native-drop"); expect(calls[0]).not.toHaveProperty("sources");
  expect(await page.evaluate(() => (window as any).fixture.batches.length)).toBe(1);
});
test("a lost delete reply blocks resubmission, observes the original and preserves remaining selections", async ({ page }) => {
  const local = page.getByRole("region", { name: "This computer", exact: true });
  for (const name of ["file-000001.bin", "file-000002.bin", "file-000003.bin"]) await local.getByRole("checkbox", { name: `Select ${name}`, exact: true }).check();
  await page.evaluate(() => { (window as any).fixture.loseDeleteSuffix = "file-000002.bin"; });
  await local.getByTitle("Delete", { exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Delete", exact: true });
  await expect(dialog.locator("li")).toHaveCount(3);
  await dialog.getByRole("button", { name: "Confirm", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Inspect original request", exact: true })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Confirm", exact: true })).toBeDisabled();
  await dialog.locator("footer").getByRole("button", { name: "Cancel", exact: true }).click();
  await local.getByTitle("Delete", { exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Confirm", exact: true })).toBeDisabled();
  await dialog.getByRole("button", { name: "Inspect original request", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "Confirm", exact: true })).toBeEnabled();
  await expect(local.getByRole("checkbox", { name: "Select file-000003.bin", exact: true })).toBeChecked();
  await dialog.getByRole("button", { name: "Confirm", exact: true }).click();
  await expect(dialog).not.toBeVisible();
  const deletes = await page.evaluate(() => (window as any).fixture.deletes);
  expect(deletes).toEqual(["C:\\Fixtures\\file-000001.bin", "C:\\Fixtures\\file-000002.bin", "C:\\Fixtures\\file-000003.bin"]);
});

test("the context menu supports keyboard navigation and restores the originating pane focus", async ({ page }) => {
  const local = page.getByRole("region", { name: "This computer", exact: true });
  await local.getByRole("checkbox", { name: "Select file-000001.bin", exact: true }).check();
  const list = local.getByRole("listbox"); await list.focus(); await page.keyboard.press("Shift+F10");
  const menu = page.getByRole("menu"); await expect(menu.getByRole("menuitem", { name: "Copy path", exact: true })).toBeFocused();
  await page.keyboard.press("ArrowDown"); await expect(menu.getByRole("menuitem", { name: "Show in file manager", exact: true })).toBeFocused();
  await page.keyboard.press("End"); await expect(menu.getByRole("menuitem", { name: "Properties", exact: true })).toBeFocused();
  await page.keyboard.press("Escape"); await expect(menu).not.toBeVisible(); await expect(list).toBeFocused();
  await page.keyboard.press("Shift+F10"); await page.keyboard.press("End"); await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog", { name: "Properties", exact: true })).toBeVisible();
});

test("unrepresentable native paths remain operational values and cannot be edited or copied as escaped labels", async ({ page }) => {
  const local = page.getByRole("region", { name: "This computer", exact: true });
  const native = { encoding: "windows_wide", value: btoa(String.fromCharCode(67, 0, 58, 0, 92, 0, 0, 216)) };
  await page.evaluate(value => { (window as any).fixture.nativeDirectory = value; }, native);
  await local.getByTitle("Refresh", { exact: true }).click();
  await local.getByText("Folder", { exact: true }).dblclick();
  await expect(local.getByTitle("Edit path", { exact: true })).toBeDisabled();
  await expect(local.locator('.fm-breadcrumbs')).toContainText('u{d800}');
  await expect(local.getByRole("status")).toContainText("100000 items");
  await local.getByTitle("Copy path", { exact: true }).click();
  await expect(local.getByRole("alert")).toContainText("cannot be represented as clipboard text");
  const paths = await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.args.operation?.action === "open_directory").map((call: any) => call.args.operation.path));
  expect(paths).toContainEqual(native); expect(paths.some((path: any) => path.encoding === "utf8" && path.value.includes("u{d800}"))).toBeFalsy();
  await local.getByTitle("Show in file manager", { exact: true }).click();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((call: any) => call.command === "file_manager_reveal_local").map((call: any) => call.args.path))).toEqual([native]);
});

test("a missing transfer submission reply retains its ID and frozen source, target, account and connection profile", async ({ page }) => {
  const local=page.getByRole("region", {name:"This computer",exact:true});
  await local.getByRole("checkbox", {name:"Select file-000001.bin",exact:true}).check();
  await page.evaluate(() => {const fixture=(window as any).fixture;fixture.batchLostReply=true;fixture.holdBatchObservation=true;});
  await page.getByRole("button", {name:"Send (1)",exact:true}).click();
  const retry=page.getByRole("button", {name:"Retry original submission",exact:true});await expect(retry).toBeVisible();
  await expect(page.getByRole("button", {name:"Send (1)",exact:true})).toBeDisabled();
  await page.getByRole("tab", {name:"Company 559038160"}).click();
  await page.locator(".fm-remote-tab:not([hidden]) .fm-relay-toggle input").evaluate((input: HTMLInputElement) => input.click());
  await local.getByRole("checkbox", {name:"Select file-000002.bin",exact:true}).check();
  await retry.click();await expect(retry).not.toBeVisible();
  const calls=await page.evaluate(() => (window as any).fixture.calls.filter((call:any) => call.command==="file_manager_start_batch").map((call:any) => call.args));
  expect(calls).toHaveLength(2);expect(calls[0]).toEqual(calls[1]);expect(calls[0].code).toBe("428358431");expect(calls[0].forceRelay).toBeFalsy();expect(calls[0].sources).toHaveLength(1);
  expect(await page.evaluate(() => (window as any).fixture.batches.length)).toBe(1);
});

test("a confirmed pre-admission rejection releases the submission gate",async ({page}) => {
  const local=page.getByRole("region", {name:"This computer",exact:true});await local.getByRole("checkbox", {name:"Select file-000001.bin",exact:true}).check();
  await page.evaluate(() => {(window as any).fixture.rejectNextBatch=true;});await page.getByRole("button", {name:"Send (1)",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("Permission was denied.");await expect(page.getByRole("button", {name:"Retry original submission",exact:true})).not.toBeVisible();
  await expect(page.getByRole("button", {name:"Send (1)",exact:true})).toBeEnabled();expect(await page.evaluate(() => (window as any).fixture.batches.length)).toBe(0);
});

test("cross-pane multi-selection drags copy through the same batch API in both directions and reject foreign drag payloads",async ({page}) => {
  const local=page.getByRole("region",{name:"This computer",exact:true}),remote=page.getByRole("region",{name:"90",exact:true});
  await local.getByRole("checkbox",{name:"Select file-000001.bin",exact:true}).check();await local.getByRole("checkbox",{name:"Select file-000002.bin",exact:true}).check();
  await local.getByRole("option").filter({hasText:"file-000001.bin"}).dragTo(remote.getByRole("listbox"));
  await expect.poll(() => page.evaluate(() => (window as any).fixture.batches.length)).toBe(1);
  await remote.getByRole("checkbox",{name:"Select file-000003.bin",exact:true}).check();
  await remote.getByRole("option").filter({hasText:"file-000003.bin"}).dragTo(local.getByRole("listbox"));
  await expect.poll(() => page.evaluate(() => (window as any).fixture.batches.length)).toBe(2);
  const calls=await page.evaluate(() => (window as any).fixture.calls.filter((call:any) => call.command==="file_manager_start_batch").map((call:any) => call.args));
  expect(calls.map((call:any) => call.direction)).toEqual(["upload","download"]);expect(calls[0].sources.map((path:any) => path.value)).toEqual(["C:\\Fixtures\\file-000001.bin","C:\\Fixtures\\file-000002.bin"]);expect(calls[1].sources.map((path:any) => path.value)).toEqual(["C:\\Fixtures\\file-000003.bin"]);
  expect(calls.every((call:any) => call.code==="428358431")).toBeTruthy();
  await page.evaluate(() => {const data=new DataTransfer();data.setData("application/x-pixels-file-selection","forged");document.querySelector(".fm-remote-panes")!.dispatchEvent(new DragEvent("drop",{dataTransfer:data,bubbles:true}));});
  expect(await page.evaluate(() => (window as any).fixture.batches.length)).toBe(2);expect(await page.evaluate(() => (window as any).fixture.deletes)).toEqual([]);
});

test("closing device tabs keeps accepted transfers running and an old close reply cannot remove a reopened generation",async ({page})=>{
  const local=page.getByRole("region",{name:"This computer",exact:true});await local.getByRole("checkbox",{name:"Select file-000001.bin",exact:true}).check();await page.getByRole("button",{name:"Send (1)",exact:true}).click();
  await page.getByRole("button",{name:"Close 90 tab",exact:true}).click();await expect(page.getByRole("tab",{name:"90 428358431",exact:true})).not.toBeVisible();await expect(page.getByRole("tab",{name:"Company 559038160",exact:true})).toHaveAttribute("aria-selected","true");
  await expect(page.getByRole("region",{name:"Transfers",exact:true})).toContainText("Transferring");expect(await page.evaluate(()=>(window as any).fixture.calls.some((call:any)=>call.command==="file_manager_batch_intent"))).toBeFalsy();
  await page.evaluate(()=>(window as any).fixture.emit("file-manager-open-tab",{code:"428358431",name:"90",os_family:"windows",generation:"20000000-0000-0000-0000-000000000090"}));
  await expect(page.getByRole("region",{name:"90",exact:true}).getByRole("status")).toContainText("100000 items");
  await page.evaluate(()=>{(window as any).fixture.holdTabClose=true;});await page.getByRole("button",{name:"Close 90 tab",exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.tabClosers.length)).toBe(1);
  await page.evaluate(()=>{const fixture=(window as any).fixture;fixture.emit("file-manager-open-tab",{code:"428358431",name:"Reopened 90",os_family:"windows",generation:"30000000-0000-0000-0000-000000000090"});fixture.tabClosers.splice(0).forEach((done:()=>void)=>done());});
  await expect(page.getByRole("tab",{name:"Reopened 90 428358431",exact:true})).toHaveAttribute("aria-selected","true");
  await expect(page.getByRole("region",{name:"Reopened 90",exact:true}).getByRole("status")).toContainText("100000 items");
  const closeCalls=await page.evaluate(()=>(window as any).fixture.calls.filter((call:any)=>call.command==="file_manager_close_tab").map((call:any)=>call.args));expect(closeCalls).toEqual([{code:"428358431",generation:"10000000-0000-0000-0000-000000000090"},{code:"428358431",generation:"20000000-0000-0000-0000-000000000090"}]);
});

test("control and bidirectional filename characters are visible labels and renaming retains the original native path",async ({page})=>{
  const local=page.getByRole("region",{name:"This computer",exact:true}),original="line\nbreak\u202e.txt",label="line\\u{a}break\\u{202e}.txt";
  await page.evaluate(name=>{(window as any).fixture.entryNameOverrides[1]=name;},original);await local.getByTitle("Refresh",{exact:true}).click();
  await expect(local.getByText(label,{exact:true})).toBeVisible();await local.getByRole("checkbox",{name:`Select ${label}`,exact:true}).check();await local.getByTitle("Rename",{exact:true}).click();
  const dialog=page.getByRole("dialog",{name:"Rename",exact:true});await expect(dialog.getByRole("textbox",{name:"Name",exact:true})).toHaveValue("");await expect(dialog).toContainText("Enter a new name");
  await dialog.getByRole("textbox",{name:"Name",exact:true}).fill("renamed.txt");await dialog.getByRole("button",{name:"Confirm",exact:true}).click();
  const operations=await page.evaluate(()=>(window as any).fixture.calls.filter((call:any)=>call.args.operation?.action==="rename").map((call:any)=>call.args.operation));
  expect(operations).toEqual([{action:"rename",path:{encoding:"utf8",value:`C:\\Fixtures\\${original}`},name:{encoding:"utf8",value:"renamed.txt"}}]);
  await page.evaluate(name=>{const fixture=(window as any).fixture;fixture.batches.push({spec:{id:"control-details",code:"428358431",direction:"upload"},phase:"failed",intent:"run",revision:"1",total:"1",completed:"0",skipped:"0",failed:"1",cancelled:"0",size:"12",confirmed:"0",scan_complete:true,message:null});fixture.items.push({spec:{id:"control-item",relative_name:name,source:{encoding:"utf8",value:`C:\\Fixtures\\${name}`},destination:{encoding:"utf8",value:"D:\\destination\\file"},size:"12"},state:"failed",confirmed:"0",message:`Cannot copy ${name}`,attempt:"control-attempt"});},original);
  await page.getByTitle("Transfer details",{exact:true}).click();const details=page.getByRole("dialog",{name:"Transfer details",exact:true});await expect(details.locator(".fm-batch-item strong")).toHaveText(label);await expect(details.getByText("Could not copy this file. Check access to the source file and destination folder.",{exact:true})).toBeVisible();await expect(details.getByText(`C:\\Fixtures\\${label}`,{exact:true})).toBeVisible();
});

test("compact navigation uses console identity, breadcrumbs, shortcuts and pane transfer actions", async ({ page }) => {
  await page.goto('http://127.0.0.1:1429/tests/file-manager.html');
  const local = page.locator('.fm-local-tab');
  const remote = page.locator('.fm-remote-tab:not([hidden])');
  await expect(local.getByRole('button', {name:'Fixtures', exact:true})).toBeVisible();
  await expect(remote.locator('.fm-pane-heading')).toContainText('90(P2P · 12 ms)');
  await expect(page.locator('.fm-execution')).toHaveCount(0);
  await expect(page.getByText('Native local fixture user', {exact:true})).toHaveCount(0);
  await expect(page.locator('.fm-roots')).toHaveCount(0);
  await expect(local.locator('.fm-pathbar').getByRole('button', {name:'Send',exact:true})).toBeVisible();
  await expect(remote.locator('.fm-pathbar').getByRole('button', {name:'Receive',exact:true})).toBeVisible();
  await local.getByLabel('Disks and folders').click();
  await expect(local.locator('.fm-location-menu').getByRole('button',{name:'Home',exact:true})).toBeVisible();
  await local.locator('.fm-location-menu').getByRole('button',{name:'D:\\',exact:true}).click();
  await expect(local.locator('.fm-breadcrumbs')).toContainText('D:');
  await local.getByTitle('Edit path',{exact:true}).click();
  await local.getByRole('textbox',{name:'This computer path'}).fill('\\\\?\\C:\\Users\\chess');
  await local.getByRole('textbox',{name:'This computer path'}).press('Enter');
  await expect(local.locator('.fm-breadcrumbs')).toContainText('chess');
  await expect(local.locator('.fm-breadcrumbs')).not.toContainText('\\\\?\\');
  await expect(local.locator('.fm-breadcrumbs button')).toHaveText(['C:\\', 'Users', 'chess']);
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='file_manager_request' && c.args.endpoint!=='local').length)).toBeGreaterThan(0);
  const reads = await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='file_manager_request' && c.args.endpoint!=='local'));
  expect(reads.every((c:any)=>c.args.execution.mode==='user')).toBeTruthy();
  await page.screenshot({path:'../../.build/file-manager/compact-navigation.png'});
});

test("long breadcrumbs collapse without scrollbars and compact tools retain their actions", async ({ page }) => {
  const local = page.locator('.fm-local-tab');
  await local.getByTitle('Edit path', {exact:true}).click();
  const address=local.getByRole('textbox',{name:'This computer path',exact:true});
  await address.fill('D:\\projects\\very-long-project-name\\content\\assets\\images\\current-folder');
  await address.press('Enter');
  for(const width of [840,1100,1600]) {
    await page.setViewportSize({width,height:850});
    await expect(local.getByRole('button',{name:'Intermediate folders',exact:true})).toBeVisible();
    await expect(local.locator('.fm-breadcrumbs [aria-current=location]')).toHaveText('current-folder');
    expect(await local.locator('.fm-breadcrumbs').evaluate(e=>[getComputedStyle(e).overflowX,getComputedStyle(e).overflowY])).toEqual(['hidden','hidden']);
    const pathBox=await local.locator('.fm-path-control').boundingBox();
    const current=await local.locator('.fm-breadcrumbs [aria-current=location]').boundingBox();
    expect(current!.width).toBeGreaterThan(0);expect(current!.x+current!.width).toBeLessThanOrEqual(pathBox!.x+pathBox!.width);
  }
  await local.getByRole('button',{name:'Intermediate folders',exact:true}).click();
  await local.locator('.fm-crumb-menu').getByRole('button',{name:'projects',exact:true}).click();
  await expect(local.locator('.fm-breadcrumbs [aria-current=location]')).toHaveText('projects');
  await expect(local.locator('.fm-pathbar').getByTitle('Refresh',{exact:true})).toHaveCount(0);
  await expect(local.locator('.fm-tools .fm-hidden + button')).toHaveAttribute('title','Refresh');
  await expect(local.getByTitle('New folder',{exact:true})).toHaveText('');
  await expect(local.locator('.fm-hidden')).toHaveCSS('font-size','14px');
  await expect(page.getByRole('button',{name:'Receive',exact:true})).toHaveClass('primary');
  await local.getByRole('checkbox',{name:'Select file-000001.bin',exact:true}).check();
  await page.getByRole('button',{name:'Send (1)',exact:true}).click();
  const row=page.locator('.fm-task').first();
  await expect(row.locator(':scope > .fm-task-speed')).toBeVisible();
  await expect(row.locator(':scope > .fm-task-elapsed')).toBeVisible();
  const speed=await row.locator('.fm-task-speed').boundingBox(), elapsed=await row.locator('.fm-task-elapsed').boundingBox();
  expect(Math.abs(speed!.y-elapsed!.y)).toBeLessThan(1);
  await page.screenshot({path:'../../.build/file-manager/toolbar-refinement.png'});
});

test("completed transfer keeps progress and every metric in a single row", async ({ page }) => {
  await page.setViewportSize({width:1600,height:850});
  await page.locator('.fm-local-tab').getByRole('checkbox',{name:'Select file-000001.bin',exact:true}).check();
  await page.getByRole('button',{name:'Send (1)',exact:true}).click();
  await page.evaluate(()=>{const batch=(window as any).fixture.batches[0];Object.assign(batch,{phase:'completed',confirmed:'118185820',size:'118185820',active_elapsed_ms:'21000',completed:'1',revision:'2'});});
  const row=page.locator('.fm-task').first();
  await expect(row.getByLabel('Transfer speed',{exact:true})).toContainText('Avg 5.37 MiB/s');
  await expect(row.getByLabel('Estimated remaining',{exact:true})).toBeEmpty();
  await expect(row.getByLabel('Active transfer time',{exact:true})).toHaveText('Active 21s');
  await expect(row.locator(':scope > .fm-task-eta')).toHaveCount(1);
  await expect(row.locator('.fm-task-progress .fm-task-eta')).toHaveCount(0);
  const centers=await row.evaluate(element=>[...element.children,...element.querySelector('.fm-task-progress')!.children].filter(e=>e.getBoundingClientRect().height>0).map(e=>{const r=e.getBoundingClientRect();return r.y+r.height/2;}));
  expect(Math.max(...centers)-Math.min(...centers)).toBeLessThan(2);
  await page.screenshot({path:'../../.build/file-manager/single-row-transfer.png'});
});


for (const [choice, label] of [["replace", "Replace"], ["skip", "Skip"], ["keep_both", "Keep both"]]) {
  test(`conflict dialog applies ${choice} to remaining conflicts when checked`, async ({ page }) => {
    await page.evaluate(() => {
      const fixture = (window as any).fixture;
      fixture.batches.push({spec:{id:"bulk-conflict",code:"428358431",direction:"upload"},phase:"waiting_conflict",intent:"run",revision:"1",total:"1",completed:"0",skipped:"0",failed:"0",cancelled:"0",size:"12",confirmed:"0",scan_complete:true,message:null});
      fixture.items.push({spec:{id:"bulk-item",relative_name:"file.txt",source:{encoding:"utf8",value:"C:/source/file.txt"},destination:{encoding:"utf8",value:"D:/destination/file.txt"},size:"12"},state:"waiting_conflict",confirmed:"0",message:"Destination already exists",attempt:null});
    });
    const dialog = page.getByRole("dialog", {name:"Transfer details",exact:true});
    const apply = dialog.getByRole("checkbox",{name:"Apply this choice to all remaining conflicts in this transfer",exact:true});
    await expect(apply).not.toBeChecked();
    await apply.check();
    await dialog.getByRole("button",{name:label,exact:true}).click();
    await expect.poll(()=>page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==="file_manager_resolve_conflict").map((c:any)=>c.args))).toEqual([{batch:"bulk-conflict",item:"bulk-item",choice,applyRemaining:true}]);
    await expect(dialog).not.toBeVisible();
    await expect(page.locator(".fm-task")).toContainText("Transferring");
    await expect(dialog).not.toBeVisible();
  });
}

test("device tabs follow the logo and relay stays hidden with conflicts defaulting to ask", async ({ page }) => {
  await expect(page.locator('.fm-titlebar').getByRole('tab')).toHaveCount(2);
  await expect(page.locator('.fm-titlebar img')).toHaveCount(1);
  await expect(page.locator('.fm-transfer-tools')).toHaveCount(0);
  const heading = page.locator('.fm-remote-panes .fm-pane-heading:visible');
  await expect(heading.locator('.fm-relay-toggle')).toBeHidden();
  expect(await heading.evaluate(e=>!!e.querySelector('.fm-link-status + .fm-relay-toggle'))).toBe(true);
  await page.getByRole('region',{name:'This computer',exact:true}).getByRole('checkbox',{name:'Select file-000001.bin',exact:true}).check();
  await page.getByRole('button',{name:'Send (1)',exact:true}).click();
  expect(await page.evaluate(()=>(window as any).fixture.calls.find((c:any)=>c.command==='file_manager_start_batch')?.args.conflict)).toBe('ask');
  await page.screenshot({path:'../../.build/file-manager/titlebar-relay.png'});
});

test("clear completed removes only successful records, retains failures on error and stays cleared after polling", async ({ page }) => {
  const clear = page.getByRole('button',{name:'Clear completed',exact:true});
  await expect(clear).toBeDisabled();
  await page.evaluate(() => {
    const fixture=(window as any).fixture;
    for (const [index,phase] of ['completed','completed_with_skips','failed','running','paused','cancelled'].entries()) {
      fixture.batches.push({spec:{id:`clear-${index}`,code:'428358431',direction:'upload'},phase,intent:'run',revision:'1',total:'1',completed:phase==='completed'?'1':'0',skipped:phase==='completed_with_skips'?'1':'0',failed:phase==='failed'?'1':'0',cancelled:phase==='cancelled'?'1':'0',size:'12',confirmed:'0',scan_complete:true,message:null});
    }
    fixture.failClearCompleted=true;
  });
  await expect(page.locator('.fm-task')).toHaveCount(6);
  const filter=page.getByRole('combobox',{name:'Transfer filter',exact:true});
  const buttonBox=await clear.boundingBox(), filterBox=await filter.boundingBox();
  expect(buttonBox!.x+buttonBox!.width).toBeLessThan(filterBox!.x);
  await clear.click();
  await expect(page.getByRole('alert')).toContainText('Could not clear completed transfers');
  await expect(page.locator('.fm-task')).toHaveCount(6);
  await page.evaluate(()=>(window as any).fixture.failClearCompleted=false);
  await clear.click();
  await expect(page.locator('.fm-task')).toHaveCount(4);
  await expect(clear).toBeDisabled();
  await expect(page.locator('.fm-task-list')).toContainText('Failed');
  const calls=await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='file_manager_batches').length);
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='file_manager_batches').length)).toBeGreaterThan(calls);
  await expect(page.locator('.fm-task')).toHaveCount(4);
});

test("conflict dialog stays open for remaining conflicts and failed choices", async ({ page }) => {
  await page.evaluate(()=>{
    const f=(window as any).fixture;
    f.batches.push({spec:{id:'two-conflicts',code:'428358431',direction:'upload'},phase:'waiting_conflict',intent:'run',revision:'1',total:'2',completed:'0',skipped:'0',failed:'0',cancelled:'0',size:'24',confirmed:'0',scan_complete:true,message:null});
    for (const id of ['first','second'])f.items.push({spec:{id,relative_name:id,source:{encoding:'utf8',value:'C:/'+id},destination:{encoding:'utf8',value:'D:/'+id},size:'12'},state:'waiting_conflict',confirmed:'0',message:null,attempt:null});
    f.failResolveConflict=true;
  });
  const dialog=page.getByRole('dialog',{name:'Transfer details',exact:true});
  await dialog.getByRole('button',{name:'Skip',exact:true}).first().click();
  await expect(dialog.getByRole('alert')).toContainText('Could not resolve the file conflict');
  await expect(dialog.locator('.fm-batch-item')).toHaveCount(2);
  await page.evaluate(()=>(window as any).fixture.failResolveConflict=false);
  await dialog.getByRole('button',{name:'Skip',exact:true}).first().click();
  await expect(dialog.locator('.fm-batch-item')).toHaveCount(1);
  await expect(dialog).toContainText('second');
  await dialog.getByRole('button',{name:'Skip',exact:true}).click();
  await expect(dialog).not.toBeVisible();
});


test("native path labels hide Windows extended prefixes without changing operation paths", async ({ page }) => {
  const raw = "\\\\?\\C:\\Fixtures\\file-000001.bin";
  expect(pathLabel(utf8Path(raw))).toBe("C:\\Fixtures\\file-000001.bin");
  const unc = "\\\\?\\UNC\\server\\share\\file.txt";
  expect(pathLabel(utf8Path(unc))).toBe("\\\\server\\share\\file.txt");
  const wide = {encoding:"windows_wide" as const,value:Buffer.from(unc,'utf16le').toString('base64')};
  expect(pathLabel(wide)).toBe("\\\\server\\share\\file.txt");
  expect(wide.value).toBe(Buffer.from(unc,'utf16le').toString('base64'));
  const local=page.locator('.fm-local-tab');
  await local.getByTitle('Edit path',{exact:true}).click();
  await local.getByRole('textbox',{name:'This computer path',exact:true}).fill("\\\\?\\C:\\Fixtures");
  await local.getByRole('textbox',{name:'This computer path',exact:true}).press('Enter');
  await local.getByRole('checkbox',{name:'Select file-000001.bin',exact:true}).check();
  await local.getByTitle('Delete',{exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Delete',exact:true});
  await expect(dialog.locator('li')).toHaveText('C:\\Fixtures\\file-000001.bin');
  const confirm=dialog.getByRole('button',{name:'Confirm',exact:true});
  await confirm.hover();
  await expect(confirm).toHaveCSS('background-color','rgb(167, 43, 43)');
  await expect(confirm).toHaveCSS('color','rgb(255, 255, 255)');
  await confirm.click();
  await expect(dialog).not.toBeVisible();
  expect(await page.evaluate(()=>(window as any).fixture.calls.find((c:any)=>c.args.operation?.action==='delete').args.operation.path)).toEqual(utf8Path(raw));
});

test("navigation and search controls share the same height beside the titlebar logo", async ({ page }) => {
  for (const pane of [page.locator('.fm-local-tab'),page.locator('.fm-remote-tab:not([hidden])')]) {
    const controls=pane.locator('.fm-path-control,.fm-location-picker summary,.fm-pathbar>.primary,.fm-search input');
    expect(await controls.evaluateAll(elements=>elements.map(e=>e.getBoundingClientRect().height))).toEqual([32,32,32,32]);
  }
  const logo=page.locator('.fm-titlebar-brand img');
  await expect(logo).toBeVisible();
  const box=await logo.boundingBox(), tab=await page.getByRole('tab').first().boundingBox();
  expect(box!.x+box!.width).toBeLessThan(tab!.x);
  await page.screenshot({path:'../../.build/file-manager/navigation-polish.png'});
});


test("Android file pane uses the advertised app UID without a console user", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/file-manager.html?android=1");
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c:any) => c.command === "file_manager_request" && c.args.endpoint !== "local" && c.args.operation.action === "locations").map((c:any) => c.args.execution.mode))).toContain("service");
  await expect(page.getByText("Console user is unavailable")).toHaveCount(0);
});

test("an unsupported Android file pane stops loading and displays its error", async ({ page }) => {
  await page.goto("http://127.0.0.1:1429/tests/file-manager.html?android=1&unsupported=1");
  await expect(page.locator(".fm-remote-tab:not([hidden])").getByText("This feature is not available on this Android device.", {exact:false})).toBeVisible();
  await expect(page.locator(".fm-remote-tab:not([hidden]) .fm-pane-status")).toContainText("Folder unavailable");
});
