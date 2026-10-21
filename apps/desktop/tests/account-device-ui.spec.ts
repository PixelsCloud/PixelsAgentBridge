import { test, expect } from '@playwright/test';

const connectionEntries = [
  { en:'Remote desktop', zh:'远程桌面', command:'desktop_viewer_window', args:{code:'123456789'} },
  { en:'Remote terminal', zh:'远程终端', command:'desktop_terminal_window', args:{code:'123456789',mode:'console'} },
  { en:'Remote terminal (System)', zh:'远程终端（System）', command:'desktop_terminal_window', args:{code:'123456789',mode:'system'} },
  { en:'Connect', zh:'连接', command:null, args:null },
];

for (const entry of connectionEntries) {
  for (const language of ['en','zh-CN']) test(`${entry.en} reports a wrong password and resumes the selected action after correction (${language})`,async({page})=>{
    const chinese=language==='zh-CN';
    await page.goto(`http://127.0.0.1:1429/tests/account-device-ui.html?language=${language}&connection-error=the%20device%20rejected%20authentication`);
    await page.getByText('Original',{exact:true}).first().click({button:'right'});
    await page.getByRole('menuitem',{name:chinese?entry.zh:entry.en,exact:true}).click();
    const dialog=page.getByRole('dialog');
    await expect(dialog.locator('.saved-connect-error')).toContainText(chinese?'设备验证失败，请检查密码':'Device authentication failed. Check the password');
    await expect(dialog.getByRole('button',{name:chinese?'重试':'Retry',exact:true})).toBeDisabled();
    expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='desktop_viewer_window'||c.command==='desktop_terminal_window'))).toEqual([]);
    await dialog.locator('#saved-connect-password').fill('correct-device-password');
    await dialog.locator('#saved-connect-password').press('Enter');
    if(entry.command){
      await expect(dialog).toBeHidden();
      expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='desktop_viewer_window'||c.command==='desktop_terminal_window'))).toEqual([{command:entry.command,args:entry.args}]);
    }else await expect(dialog).toContainText(chinese?'设备已连接':'Device connected');
    expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='operator_connect').map((c:any)=>c.args))).toEqual([{code:'123456789',password:'correct-device-password'}]);
  });

  test(`${entry.en} reports offline and retries with saved credentials`,async({page})=>{
    await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?connection-error=the%20device%20is%20offline');
    await page.getByText('Original',{exact:true}).first().click({button:'right'});
    await page.getByRole('menuitem',{name:entry.en,exact:true}).click();
    const dialog=page.getByRole('dialog');await expect(dialog.locator('.saved-connect-error')).toContainText('Device is offline');
    await expect(dialog.getByRole('button',{name:'Retry',exact:true})).toBeEnabled();
    await page.evaluate(()=>{(window as any).fixture.connectionError=null;});
    await dialog.getByRole('button',{name:'Retry',exact:true}).click();
    if(entry.command){
      await expect(dialog).toBeHidden();
      expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='desktop_viewer_window'||c.command==='desktop_terminal_window'))).toEqual([{command:entry.command,args:entry.args}]);
    }else await expect(dialog).toContainText('Device connected');
    expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='operator_connect_saved').length)).toBe(2);
    expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='operator_connect'))).toEqual([]);
  });

  test(`${entry.en} rechecks a stale connected card before opening`,async({page})=>{
    await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?presence=online&connected&connection-error=the%20device%20is%20offline');
    await expect(page.locator('.home-recent-connection')).toContainText('Connected');
    await page.getByText('Original',{exact:true}).first().click({button:'right'});
    await page.getByRole('menuitem',{name:entry.en,exact:true}).click();
    await expect(page.getByRole('dialog').locator('.saved-connect-error')).toContainText('Device is offline');
    expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='operator_connect_saved').length)).toBe(1);
    expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='desktop_viewer_window'||c.command==='desktop_terminal_window'))).toEqual([]);
    await expect(page.locator('.home-recent-connection')).not.toContainText('Connected');
  });
}

test('a window startup failure stays visible and retry preserves the terminal mode and authenticated state',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?presence=online');
  await page.evaluate(()=>{(window as any).fixture.launchError='Could not create the native window';});
  await page.getByText('Original',{exact:true}).first().click({button:'right'});
  await page.getByRole('menuitem',{name:'Remote terminal (System)',exact:true}).click();
  const dialog=page.getByRole('dialog');await expect(dialog.locator('.saved-connect-error')).toContainText('Could not create the native window');
  await expect(page.locator('.home-recent-connection')).toContainText('Connected');
  await page.evaluate(()=>{(window as any).fixture.launchError=null;});
  await dialog.getByRole('button',{name:'Retry',exact:true}).click();await expect(dialog).toBeHidden();
  expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='desktop_terminal_window').map((c:any)=>c.args.mode))).toEqual(['system','system']);
});

test('closing a pending request prevents its late result from opening a window or replacing a newer action',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?hold-connect');
  await page.getByText('Original',{exact:true}).first().click({button:'right'});
  await page.getByRole('menuitem',{name:'Remote desktop',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.pendingConnections.length)).toBe(1);
  await page.getByRole('dialog').locator('.ant-modal-footer').getByRole('button',{name:'Close',exact:true}).click();
  await page.getByText('Original',{exact:true}).first().click({button:'right'});
  await page.getByRole('menuitem',{name:'Remote terminal (System)',exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.pendingConnections.length)).toBe(2);
  await page.evaluate(()=>(window as any).fixture.pendingConnections[0]());
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.connected)).toBe(true);
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='desktop_viewer_window'||c.command==='desktop_terminal_window'))).toEqual([]);
  await page.evaluate(()=>(window as any).fixture.pendingConnections[1]());
  await expect(page.getByRole('dialog')).toBeHidden();
  expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='desktop_viewer_window'||c.command==='desktop_terminal_window'))).toEqual([{command:'desktop_terminal_window',args:{code:'123456789',mode:'system'}}]);
});

for(const [error,message,needsPassword] of [
  ['endpoint control was rejected with InvalidCredentials: invalid credential','Device authentication failed. Check the password',true],
  ['endpoint control was rejected with PermissionDenied: denied','Connection permission denied',false],
  ['endpoint control was rejected with NotFound: device not found','Device not found or connection details are no longer valid',false],
  ['PAB connection operation timed out','Connection timed out. Try again',false],
  ['the endpoint control supervisor is unavailable','The control service is unavailable',false],
] as const) test(`remote entry displays a readable connection error: ${error}`,async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?connection-error='+encodeURIComponent(error));
  await page.getByText('Original',{exact:true}).first().click({button:'right'});
  await page.getByRole('menuitem',{name:'Remote desktop',exact:true}).click();
  const dialog=page.getByRole('dialog');await expect(dialog.locator('.saved-connect-error')).toContainText(message);
  if(needsPassword)await expect(dialog.getByRole('button',{name:'Retry',exact:true})).toBeDisabled();
  else await expect(dialog.getByRole('button',{name:'Retry',exact:true})).toBeEnabled();
});

test('manual refresh reloads saved devices and presence without resetting a live connection', async ({page}) => {
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?presence=offline&connected');
  await expect(page.locator('.home-recent-connection')).toContainText('Connected');
  await page.evaluate(() => {
    const fixture = (window as any).fixture;
    fixture.presence = true;
    const original = (window as any).__TAURI_INTERNALS__.invoke;
    (window as any).__TAURI_INTERNALS__.invoke = async (command:string, args:any) => {
      if (command === 'operator_bootstrap') {
        await new Promise(resolve => { fixture.finishRefresh = resolve; });
        const saved = await original(command, args);
        return {...saved, devices: [...saved.devices, {...saved.devices[0], deviceId:'new-device', deviceCode:'987654321', alias:'New device', connected:false}]};
      }
      return original(command, args);
    };
  });
  const refresh = page.getByRole('button',{name:'Refresh device list',exact:true});
  await refresh.click();
  await expect(refresh).toBeDisabled();
  await page.evaluate(() => (window as any).fixture.finishRefresh());
  await expect(refresh).toBeEnabled();
  await expect(page.locator('.device-list .home-recent-device')).toHaveCount(2);
  await expect(page.locator('.count-badge')).toHaveText('2');
  await expect(page.locator('.home-recent-device').filter({hasText:'123 456 789'}).locator('.home-recent-connection')).toContainText('Connected');
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((call:any)=>call.command==='operator_saved_device_presence').length)).toBeGreaterThan(1);
  await expect(page.getByText('New device',{exact:true})).toBeVisible();
});

for (const view of ['home', 'remote']) {
  for (const [presence, label] of [['online', 'Online'], ['offline', 'Offline'], ['unknown', 'Unknown'], ['error', 'Unknown']]) {
    test(`${view} card shows ${label} without requiring an authenticated device connection (${presence})`, async ({page}) => {
      await page.goto(`http://127.0.0.1:1429/tests/account-device-ui.html?${view === 'home' ? 'home&' : ''}presence=${presence}`);
      const card = page.locator('.home-recent-device');
      await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label', label);
      await expect(card.locator('.home-recent-connection')).toHaveText(label);
      await expect(card).not.toContainText('Disconnected');
      expect(await page.evaluate(() => (window as any).fixture.calls.filter((call:any) => call.command === 'operator_connect_saved'))).toEqual([]);
    });
  }
}

test('a failed availability refresh recovers on the same open window', async ({page}) => {
  await page.clock.install();
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?home&presence=error');
  const card = page.locator('.home-recent-device');
  await expect(card.locator('.home-recent-connection')).toHaveText('Unknown');
  await page.evaluate(() => {
    (window as any).fixture.presenceError = false;
    (window as any).fixture.presence = true;
  });
  await page.clock.fastForward(15000);
  await expect(card.locator('.home-recent-connection')).toHaveText('Online');
  await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label', 'Online');
});

test('device search matches names, raw and formatted codes and supports clearing without connecting',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?many-devices');
  const search=page.getByRole('textbox',{name:'Search name or device code',exact:true});
  const cards=page.locator('.device-list .home-recent-device');
  await expect(cards).toHaveCount(33);
  await search.fill('DEVICE 33');
  await expect(cards).toHaveCount(1);
  await expect(cards.first()).toContainText('Device 33');
  await search.fill('123456791');
  await expect(cards).toHaveCount(1);
  await expect(cards.first()).toContainText('Device 3');
  await search.fill('123 456 791');
  await expect(cards).toHaveCount(1);
  await expect(cards.first()).toContainText('Device 3');
  await search.fill('missing-device');
  await expect(cards).toHaveCount(0);
  await expect(page.getByText('No matching devices',{exact:true})).toBeVisible();
  await page.locator('.device-search .ant-input-clear-icon').click();
  await expect(cards).toHaveCount(33);
  await page.locator('.remote-list').evaluate(element=>{element.style.width='290px';element.style.height='600px';});
  await page.locator('.remote-list').screenshot({path:'../../.build/device-search-preview.png'});
  expect(await page.evaluate(()=>(window as any).fixture.calls.filter((call:any)=>call.command==='operator_connect_saved'))).toEqual([]);
});

for (const presence of ['offline', 'unknown', 'error']) {
  test(`a confirmed connection shows online across home, list and details despite ${presence} directory data`,async({page})=>{
    await page.goto(`http://127.0.0.1:1429/tests/account-device-ui.html?home&presence=${presence}`);
    const card=page.locator('.home-recent-device');
    await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label',presence==='offline'?'Offline':'Unknown');
    await card.dblclick();
    await expect(page.getByRole('dialog')).toContainText('Device connected');
    await page.getByRole('dialog').locator('.ant-modal-footer').getByRole('button',{name:'Close',exact:true}).click();
    await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label','Online');
    await expect(card.locator('.home-recent-connection')).toContainText('Connected');
    await expect(card.locator('.home-recent-connection')).toContainText('P2P');
    await page.evaluate(()=>(window as any).fixture.setView('remote'));
    await expect(page.locator('.device-list .home-recent-presence')).toHaveAttribute('aria-label','Online');
    await expect(page.locator('.device-basic-grid > div').nth(1).locator('strong')).toHaveText('Online');
    await expect(page.locator('.device-basic-grid > div').nth(2).locator('strong')).toHaveText('Connected');
    await page.evaluate(()=>(window as any).fixture.setView('home'));
    await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label','Online');
  });
}

test('disconnect stops proving online, while directory online does not imply a client connection',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?home&presence=offline&connected');
  const card=page.locator('.home-recent-device');
  await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label','Online');
  await page.evaluate(()=>(window as any).fixture.emitConnection('disconnected'));
  await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label','Offline');
  await expect(card.locator('.home-recent-connection')).toHaveText('Offline');
  await page.evaluate(()=>{const f=(window as any).fixture;f.presence=true;f.setView('remote');});
  await expect(page.locator('.device-list .home-recent-presence')).toHaveAttribute('aria-label','Online');
  await expect(page.locator('.device-list .home-recent-connection')).toHaveText('Online');
  await expect(page.locator('.device-basic-grid > div').nth(1).locator('strong')).toHaveText('Online');
  await expect(page.locator('.device-basic-grid > div').nth(2).locator('strong')).toHaveText('Disconnected');
});

test('an in-flight disconnected path snapshot cannot overwrite a successful connection',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?home&presence=offline&hold-paths');
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.pendingPaths.length)).toBe(1);
  const card=page.locator('.home-recent-device');await card.dblclick();
  await expect(page.getByRole('dialog')).toContainText('Device connected');
  await page.getByRole('dialog').locator('.ant-modal-footer').getByRole('button',{name:'Close',exact:true}).click();
  await page.evaluate(()=>(window as any).fixture.pendingPaths[0].resolve([{deviceCode:'123456789',path:null}]));
  // The next refresh stays pending so it cannot conceal a stale-state regression.
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.pendingPaths.length)).toBeGreaterThan(1);
  await expect(card.locator('.home-recent-connection')).toHaveText('Connected');
  await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label','Online');
});

test('an in-flight connected path snapshot cannot resurrect a disconnected session',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?home&presence=unknown&connected&hold-paths');
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.pendingPaths.length)).toBe(1);
  await page.evaluate(()=>{const f=(window as any).fixture;f.emitConnection('disconnected');f.pendingPaths[0].resolve([{deviceCode:'123456789',path:'p2p'}]);});
  await expect.poll(()=>page.evaluate(()=>(window as any).fixture.pendingPaths.length)).toBeGreaterThan(1);
  await expect(page.locator('.home-recent-connection')).toHaveText('Unknown');
  await expect(page.locator('.home-recent-presence')).toHaveAttribute('aria-label','Unknown');
});

for (const error of ['device password has not been entered', 'the device rejected authentication']) {
  test(`online presence is independent of a failed device password: ${error}`, async ({page}) => {
    await page.goto(`http://127.0.0.1:1429/tests/account-device-ui.html?home&presence=online&connection-error=${encodeURIComponent(error)}`);
    const card = page.locator('.home-recent-device');
    await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label', 'Online');
    await expect(card.locator('.home-recent-connection')).toHaveText('Online');
    await card.dblclick();
    const dialog = page.getByRole('dialog');
    await expect(dialog.locator('#saved-connect-password')).toBeVisible();
    await expect(dialog.getByRole('button', {name:'Retry', exact:true})).toBeDisabled();
    await expect(card.locator('.home-recent-presence')).toHaveAttribute('aria-label', 'Online');
    await expect(card.locator('.home-recent-connection')).toHaveText('Online');
    await dialog.locator('.ant-modal-footer').getByRole('button', {name:'Close', exact:true}).click();
    await page.evaluate(() => (window as any).fixture.setView('remote'));
    await expect(page.locator('.device-list .home-recent-presence')).toHaveAttribute('aria-label', 'Online');
    await expect(page.locator('.device-basic-grid > div').nth(1).locator('strong')).toHaveText('Online');
    await expect(page.locator('.device-basic-grid > div').nth(2).locator('strong')).toHaveText('Disconnected');
    await page.getByText('Original', {exact:true}).first().click({button:'right'});
    await page.getByRole('menuitem', {name:'Connect', exact:true}).click();
    await dialog.locator('#saved-connect-password').fill('correct-device-password');
    await dialog.getByRole('button', {name:'Retry', exact:true}).click();
    await expect(dialog).toContainText('Device connected');
    await expect(page.locator('.device-list .home-recent-presence')).toHaveAttribute('aria-label', 'Online');
    await expect(page.locator('.device-basic-grid > div').nth(2).locator('strong')).toHaveText('Connected');
  });

  test(`saved connection allows password correction: ${error}`, async ({page}) => {
    await page.goto(`http://127.0.0.1:1429/tests/account-device-ui.html?connection-error=${encodeURIComponent(error)}`);
    await page.getByText('Original', {exact:true}).first().click({button:'right'});
    await page.getByRole('menuitem', {name:'Connect', exact:true}).click();
    const dialog = page.getByRole('dialog');
    const retry = dialog.getByRole('button', {name:'Retry', exact:true});
    await expect(retry).toBeDisabled();
    await expect(dialog).not.toContainText('device password has not been entered');
    await dialog.locator('#saved-connect-password').fill('wrong-password');
    await retry.click();
    await expect(dialog).toContainText('Device authentication failed. Check the password');
    await expect(dialog.locator('#saved-connect-password')).toHaveValue('');
    await expect(retry).toBeDisabled();
    await dialog.locator('#saved-connect-password').fill('correct-device-password');
    await dialog.locator('#saved-connect-password').press('Enter');
    await expect(dialog).toContainText('Device connected');
    const calls = await page.evaluate(() => (window as any).fixture.calls.filter((c:any) => c.command === 'operator_connect'));
    expect(calls.map((c:any) => c.args)).toEqual([
      {code:'123456789',password:'wrong-password'},
      {code:'123456789',password:'correct-device-password'},
    ]);
    await expect(dialog.locator('#saved-connect-password')).toHaveCount(0);
  });
}

test('network failure still allows retry without replacing the saved password', async ({page}) => {
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?connection-error=the%20device%20is%20offline');
  await page.getByText('Original', {exact:true}).first().click({button:'right'});
  await page.getByRole('menuitem', {name:'Connect', exact:true}).click();
  const retry = page.getByRole('dialog').getByRole('button', {name:'Retry', exact:true});
  await expect(retry).toBeEnabled();
  await retry.click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c:any) => c.command === 'operator_connect_saved').length)).toBe(2);
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c:any) => c.command === 'operator_connect'))).toEqual([]);
});

test('manual connection can correct the password inside the failure dialog', async ({page}) => {
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?manual');
  await page.getByPlaceholder('000 000 000').fill('123456789');
  await page.locator('#home-connect-password').fill('wrong-password');
  await page.getByRole('button', {name:'Add device',exact:true}).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toContainText('Device authentication failed. Check the password');
  await dialog.locator('#saved-connect-password').fill('correct-device-password');
  await dialog.getByRole('button', {name:'Retry',exact:true}).click();
  await expect(dialog.getByRole('button', {name:'Open device'})).toBeVisible();
  await expect(page.locator('#home-connect-password')).toHaveValue('');
});
test('adding a device requires account login without disabling receiving service',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html?logged-out');
  await expect(page.getByText('Sign in to control a remote computer.')).not.toBeVisible();
  await expect(page.getByRole('button',{name:'Register / Sign in',exact:true})).not.toBeVisible();
  await page.getByRole('button',{name:'Add device',exact:true}).click();
  await expect(page.getByRole('dialog')).toContainText('Sign in to control a remote computer.');
  await page.getByRole('button',{name:'Cancel',exact:true}).click();
  expect(await page.evaluate(()=>(window as any).fixture.signedIn)).toBe(false);
  await page.getByPlaceholder('000 000 000').fill('123456789');
  await page.locator('#home-connect-password').fill('test-device-password');
  await page.getByRole('button',{name:'Add device',exact:true}).click();
  await page.getByRole('button',{name:'Register / Sign in',exact:true}).click();
  expect(await page.evaluate(()=>(window as any).fixture.signedIn)).toBe(true);
  expect(await page.evaluate(()=>(window as any).fixture.calls.filter((c:any)=>c.command==='operator_connect'))).toEqual([]);
  await expect(page.getByPlaceholder('000 000 000')).toHaveValue('123 456 789');
  await expect(page.locator('#home-connect-password')).toHaveValue('test-device-password');
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

test('the two remote terminal menu entries request their own explicit launch mode',async({page})=>{
  await page.goto('http://127.0.0.1:1429/tests/account-device-ui.html');
  for (const mode of ['console', 'system']) {
    await page.getByText('Original',{exact:true}).first().click({button:'right'});
    await page.getByRole('menuitem',{name:mode === 'system' ? 'Remote terminal (System)' : 'Remote terminal',exact:true}).click();
    await expect.poll(()=>page.evaluate(()=>(window as any).fixture.calls.filter((call:any)=>call.command==='desktop_terminal_window').at(-1)?.args.mode)).toBe(mode);
  }
});
