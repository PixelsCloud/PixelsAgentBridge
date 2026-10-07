import { test, expect } from "@playwright/test";

const url = "http://127.0.0.1:1429/tests/history-settings-ui.html";
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
