import { test, expect } from '@playwright/test';
test('adding a device requires account login without disabling receiving service',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?logged-out');
  await expect(page.getByText('Register or sign in before adding or controlling devices. Receiving devices do not need to sign in.')).toBeVisible();
  await page.getByPlaceholder('000 000 000').fill('123456789');
  await page.locator('#home-connect-password').fill('test-device-password');
  await expect(page.getByRole('button',{name:'Add device',exact:true})).toBeDisabled();
  await page.getByRole('button',{name:'Register / Sign in',exact:true}).click();
  expect(await page.evaluate(()=>(window as any).fixture.signedIn)).toBe(true);
  expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='operator_connect'))).toEqual([]);
});
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
test('revoked login reopens sign-in and keeps the device draft',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?expired');
  await page.getByPlaceholder('000 000 000').fill('123456789');
  await page.locator('#home-connect-password').fill('test-device-password');
  await page.getByRole('button',{name:'Add device',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.signedIn)).toBe(true);
  await expect(page.getByPlaceholder('000 000 000')).toHaveValue('123 456 789');
  await expect(page.locator('#home-connect-password')).toHaveValue('test-device-password');
});
test('GitHub browser wait can be cancelled without signing in',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?github');
  await page.getByRole('button',{name:'Continue with GitHub'}).click();
  await expect(page.getByText('Complete GitHub authorization in your browser')).toBeVisible();
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  await expect(page.getByRole('button',{name:'Cancel',exact:true})).not.toBeVisible();
  expect(await page.evaluate(()=>(window as any).fixture.signedIn)).toBe(false);
});
