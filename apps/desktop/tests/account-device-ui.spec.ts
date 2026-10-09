import { test, expect } from '@playwright/test';
test('rename dialog enables save for its own changed draft and saves that name',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html');
  await page.getByText('Original',{exact:true}).first().click({button:'right'});
  await page.getByRole('menuitem',{name:/Rename/}).click();
  const dialog=page.getByRole('dialog');const save=dialog.getByRole('button',{name:/Save/});
  await expect(save).toBeDisabled();
  await dialog.locator('input').fill('Renamed device');await expect(save).toBeEnabled();
  await dialog.locator('input').fill('Original');await expect(save).toBeDisabled();
  await dialog.locator('input').fill('Renamed device');await save.click();
  await expect(dialog).not.toBeVisible();await expect(page.getByText('Renamed device',{exact:true}).first()).toBeVisible();
  const calls=await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='operator_rename_device'));
  expect(calls).toEqual([{command:'operator_rename_device',args:{code:'123456789',alias:'Renamed device'}}]);
});
test('GitHub browser wait can be cancelled without signing in',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?github');
  await page.getByRole('button',{name:'Continue with GitHub'}).click();
  await expect(page.getByText('Complete GitHub authorization in your browser')).toBeVisible();
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  await expect(page.getByRole('button',{name:'Cancel',exact:true})).not.toBeVisible();
  expect(await page.evaluate(()=>(window as any).fixture.signedIn)).toBe(false);
});
