import { expect,test,type Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { resolve } from 'node:path';

async function fixture(page:Page,language='en',theme='light') {
  const device={endpoint:'a'.repeat(64),kind:'device',device_id:'device-fixture',name:'Android phone',code:'123456789',username:'Pixels',system:'android',os_name:'Android',os_version:'15',architecture:'aarch64',agent_version:'1.3.19'};
  const user={endpoint:'b'.repeat(64),kind:'user',username:'Pixels'};
  const state={fresh:true,supported:true,fail:false,requests:[] as URL[],duration:'65000',closed:false};
  await page.addInitScript(({language,theme})=>{localStorage.setItem('pab-web-language',language);localStorage.setItem('pab-web-theme',theme);},{language,theme});
  await page.routeWebSocket('**/api/web/events',()=>{});
  await page.route('**/api/web/**',async route=>{
    const url=new URL(route.request().url());const path=url.pathname.replace('/api/web','');let data:unknown;
    if(path==='/config')data={registration_enabled:true,version:'fixture'};
    else if(path==='/session')data={id:'diagnostics-admin',username:'Pixels',server_admin:true};
    else if(path==='/relays')data={mode:'cluster',items:[{id:'relay-a',url:'https://relay.example/',admin_state:'active',health:'healthy',online:true,configured:true,telemetry_current:true,telemetry_sampled_at:1792656000000,capacity_mode:'host',agent_version:'1.3.19'}],page:1,page_size:20,total:1};
    else if(path==='/relays/relay-a/runtime'){
      state.requests.push(url);if(state.fail){await route.fulfill({status:500,json:{code:'server_error'}});return;}
      const view=url.searchParams.get('view');const all=view==='links'?[{id:'link-1',a:device,b:user,status:state.fresh?(state.closed?'closed':'active'):'unknown',duration_ms:state.duration,started_at:1792655935000,last_forwarded_at:1792656000000,a_to_b_bytes:'9007199254740993',b_to_a_bytes:'2048',a_to_b_packets:'123',b_to_a_packets:'456',a_to_b_rate:'1024',b_to_a_rate:'2048'}]:[
        {id:'1',identity:device,status:state.fresh?(state.closed?'closed':'connected'):'unknown',connected_at:1792655935000,duration_ms:state.duration,closed_at:state.closed?1792656000000:null},
        {id:'2',identity:user,status:state.fresh?'connected':'unknown',connected_at:1792655935000,duration_ms:state.duration},
      ];
      const status=url.searchParams.get('status');const search=url.searchParams.get('q')?.toLowerCase()??'';const items=all.filter(row=>(!status||status==='all'||row.status===status)&&(!search||JSON.stringify(row).toLowerCase().includes(search)));
      data={supported:state.supported,fresh:state.fresh,sampled_at:1792656000000,host:{hostname:'relay-host',os:'linux',os_version:'Debian 12',architecture:'x86_64',cpu_model:'Test CPU',logical_cpus:4,cpu_usage_basis_points:1250,memory_total:'8589934592',memory_used:'1073741824',host_uptime_ms:'86400000',process_uptime_ms:'3600000',process_memory:'33554432',containerized:true},traffic:{received_bytes:'9007199254740993',sent_bytes:'2048',received_bytes_per_second:'1024',sent_bytes_per_second:'2048'},summary:{connections:state.fresh?'2':null,connected_endpoints:state.fresh?2:null,active_links:state.fresh?1:null,idle_links:state.fresh?0:null,connections_truncated:false,links_truncated:false},items,total:items.length,page:1,page_size:20};
    }else throw new Error(`Unexpected diagnostics fixture route: ${path}`);
    await route.fulfill({json:data});
  });return state;
}
async function open(page:Page,language='en') {await page.goto('/relay');await page.getByRole('button',{name:language==='en'?'Connections and links':'连接与链路',exact:true}).click();await expect(page.getByTestId('relay-runtime').getByText('relay-host',{exact:true})).toBeVisible();}

test('both detail entry points use a centered modal, and closing stops runtime polling',async({page})=>{
  await page.clock.install();const state=await fixture(page);await page.goto('/relay');
  await page.getByRole('button',{name:'Details',exact:true}).click();const dialog=page.getByRole('dialog');
  await expect(dialog).toHaveClass(/ant-modal/);await expect(dialog.getByRole('tab',{name:'Node information',exact:true})).toHaveAttribute('aria-selected','true');
  await dialog.getByRole('button',{name:'Close',exact:true}).click();await expect(dialog).toHaveCount(0);
  await page.getByRole('button',{name:'Connections and links',exact:true}).click();await expect(page.getByTestId('relay-runtime')).toBeVisible();
  await page.clock.fastForward(1000);
  await expect.poll(()=>dialog.evaluate(el=>{const r=el.getBoundingClientRect();return Math.abs((r.left+r.right)/2-innerWidth/2);})).toBeLessThan(2);
  await expect.poll(()=>dialog.evaluate(el=>{const r=el.getBoundingClientRect();return Math.abs((r.top+r.bottom)/2-innerHeight/2);})).toBeLessThan(2);
  await expect(page.locator('.ant-drawer-open')).toHaveCount(0);
  await page.keyboard.press('Escape');await page.clock.fastForward(1000);await expect(dialog).toHaveCount(0);await expect(page.getByTestId('relay-runtime')).toHaveCount(0);
  const count=state.requests.length;await page.clock.fastForward(15000);expect(state.requests).toHaveLength(count);
});

test('device inventory, exact traffic, filters and timestamps are usable',async({page})=>{
  const state=await fixture(page);await open(page);const panel=page.getByTestId('relay-runtime');
  await expect(panel).toContainText('Android phone');await expect(panel).toContainText('android · aarch64 · 1.3.19');await expect(panel).toContainText('1 min 5 sec');
  await panel.getByRole('searchbox',{name:'Device, account or endpoint ID'}).fill('Android');await panel.getByRole('searchbox',{name:'Device, account or endpoint ID'}).press('Enter');
  await expect.poll(()=>state.requests.at(-1)?.searchParams.get('q')).toBe('Android');await expect(panel.locator('tbody .ant-table-row')).toHaveCount(1);
  await panel.getByRole('tab',{name:'Relay links',exact:true}).click();
  await expect(panel).toContainText('8 PiB');await expect(panel).toContainText('1 KiB/s');
  await panel.getByRole('combobox',{name:'Status',exact:true}).selectOption('closed');await expect.poll(()=>state.requests.at(-1)?.searchParams.get('status')).toBe('closed');await expect(panel.locator('tbody .ant-table-row')).toHaveCount(0);
  await panel.getByRole('combobox',{name:'Status',exact:true}).selectOption('all');await expect(panel.locator('tbody .ant-table-row')).toHaveCount(1);
  await panel.getByRole('button',{name:'Expand row'}).click();await expect(panel).toContainText('First forwarded');await expect(panel).toContainText('123');
});

test('polling retains rendered cards and rows, stale reads stop claiming live counts, and hidden tabs stop polling',async({page})=>{
  await page.clock.install();const state=await fixture(page);await open(page);const panel=page.getByTestId('relay-runtime');const node=await panel.locator('.relay-runtime-stats').elementHandle();
  state.duration='70000';await page.clock.fastForward(5000);await expect(panel).toContainText('1 min 10 sec');expect(await node!.evaluate(el=>el.isConnected)).toBe(true);await expect(panel.locator('.ant-spin-spinning')).toHaveCount(0);
  state.fresh=false;await panel.getByRole('button',{name:'Refresh',exact:true}).click();await expect(panel).toContainText('Historical sample');await expect(panel.locator('.relay-runtime-stats strong')).toHaveText(['—','—','—','—']);await expect(panel.locator('.ant-table-tbody')).toContainText('Unknown');
  state.fail=true;await panel.getByRole('button',{name:'Refresh',exact:true}).click();await expect(panel.getByRole('alert')).toBeVisible();
  await page.getByRole('tab',{name:'Node information',exact:true}).click();await expect(panel).toHaveCount(0);const count=state.requests.length;await page.clock.fastForward(15000);expect(state.requests).toHaveLength(count);
});

test('older Relay versions show unavailable detail rather than fabricated zero counts',async({page})=>{
  const state=await fixture(page);state.supported=false;await page.goto('/relay');await page.getByRole('button',{name:'Connections and links',exact:true}).click();const panel=page.getByTestId('relay-runtime');await expect(panel).toContainText('has not reported connection details');await expect(panel.locator('.relay-runtime-stats')).toHaveCount(0);
});

for(const language of ['en','zh-CN'])for(const theme of ['light','dark'])for(const width of [390,1440]){
  test(`runtime layout ${language} ${theme} ${width}`,async({page})=>{
    await page.setViewportSize({width,height:1000});await fixture(page,language,theme);await open(page,language);const panel=page.getByTestId('relay-runtime');
    const dialog=page.getByRole('dialog');await expect(dialog).toHaveClass(/ant-modal/);
    await expect.poll(()=>dialog.evaluate(el=>{let opacity=1;for(let node:Element|null=el;node;node=node.parentElement)opacity*=Number(getComputedStyle(node).opacity);return opacity;})).toBeGreaterThanOrEqual(.99);
    await expect.poll(()=>dialog.evaluate(el=>{const r=el.getBoundingClientRect();return Math.abs((r.left+r.right)/2-innerWidth/2);})).toBeLessThan(2);
    await expect.poll(()=>dialog.evaluate(el=>{const r=el.getBoundingClientRect();return Math.abs((r.top+r.bottom)/2-innerHeight/2);})).toBeLessThan(2);
    await expect.poll(()=>panel.evaluate(el=>el.getBoundingClientRect().right)).toBeLessThanOrEqual(width+1);
    expect(await page.evaluate(()=>document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    await panel.getByRole('tab',{name:language==='en'?'Relay links':'中继链路',exact:true}).click();await expect(panel).toContainText('8 PiB');
    const directory=process.env.PAB_WEB_ARTIFACT_DIR;if(directory){mkdirSync(directory,{recursive:true});await page.screenshot({path:resolve(directory,`runtime-${language}-${theme}-${width}.png`),fullPage:true});}
  });
}
