import { test, expect } from '@playwright/test';

test('password refresh is single-flight, keeps a hidden password hidden, and copies the new credential',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/device-credentials.html?delay');
  await page.getByRole('button',{name:'Hide',exact:true}).click();
  const refresh=page.getByRole('button',{name:'Refresh password',exact:true});
  await refresh.click();
  await expect(refresh).toBeDisabled();
  await expect(page.getByRole('button',{name:'Copy Password',exact:true})).toBeDisabled();
  expect(await page.evaluate(()=>(window as any).fixture.calls)).toEqual(['rotate_device_password']);
  await page.evaluate(()=>(window as any).fixture.finish());
  await expect(refresh).toBeEnabled();
  await expect(page.getByRole('img',{name:'Password (Hide)',exact:true})).toBeVisible();
  await expect(page.locator('.home-password-value')).not.toContainText('NEWPASS8');
  await page.getByRole('button',{name:'Show',exact:true}).click();
  await expect(page.locator('.home-password-value strong')).toHaveText('NEWPASS8');
  await page.getByRole('button',{name:'Copy Password',exact:true}).click();
  expect(await page.evaluate(()=>(window as any).fixture.copied)).toBe('NEWPASS8');
  await page.evaluate(()=>(window as any).fixture.status({device_code:'123456789',device_id:'fixture',temporary_password:'OLDPASS8',password_version:1,executor_running:true,control_phase:'Authenticated'}));
  await expect(page.locator('.home-password-value strong')).toHaveText('NEWPASS8');
});

test('refresh failure preserves the password and supports retry',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/device-credentials.html?error');
  const refresh=page.getByRole('button',{name:'Refresh password',exact:true});
  await refresh.click();
  await expect(page.getByRole('alert')).toContainText('Could not confirm');
  await expect(page.locator('.home-password-value strong')).toHaveText('OLDPASS8');
  await expect(refresh).toBeEnabled();
  await page.evaluate(()=>(window as any).fixture.error=false);
  await refresh.click();
  await expect(page.locator('.home-password-value strong')).toHaveText('NEWPASS8');
  await expect(page.getByRole('alert')).toHaveCount(0);
});

test('offline service disables password refresh',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/device-credentials.html?offline');
  await expect(page.getByRole('button',{name:'Refresh password',exact:true})).toBeDisabled();
  expect(await page.evaluate(()=>(window as any).fixture.calls)).toEqual([]);
});

for (const [language,label] of [['zh-CN','刷新密码'],['zh-TW','重新產生密碼'],['en','Refresh password']]) {
  test(`password controls fit the home card in ${language}`,async({page})=>{
    await page.goto(`http://127.0.0.1:1429/tests/device-credentials.html?language=${language}`);
    await expect(page.getByRole('button',{name:label,exact:true})).toBeVisible();
    const geometry=await page.locator('.home-password-value').evaluate(element=>{
      const parent=element.getBoundingClientRect();
      const strong=element.querySelector('strong')!;
      return {clipped:strong.scrollWidth>strong.clientWidth, inside:[...element.querySelectorAll('button')].every(button=>button.getBoundingClientRect().right<=parent.right+1)};
    });
    expect(geometry).toEqual({clipped:false,inside:true});
    if(language==='zh-CN') await page.screenshot({path:'../../.build/password-refresh-preview.png'});
  });
}
