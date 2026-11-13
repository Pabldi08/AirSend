const assert = require('node:assert/strict');
const {chromium} = require('playwright');
const {mkdir, writeFile} = require('node:fs/promises');
const {installDesktopMock} = require('./desktop-mock.cjs');
const http = require('node:http');
const path = require('node:path');
const {readFile} = require('node:fs/promises');
const {tmpdir} = require('node:os');
let browser, server;
async function previewUrl() {
  if (process.env.AIRSEND_UI_URL) return process.env.AIRSEND_UI_URL;
  const dist = path.resolve(__dirname, '../dist');
  server = http.createServer(async (req, res) => {
    try {
      const pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
      const file = path.resolve(dist, '.' + (pathname === '/' ? '/index.html' : pathname));
      if (!file.startsWith(dist + path.sep)) { res.writeHead(403).end(); return; }
      const bytes = await readFile(file);
      const types = {'.html':'text/html', '.js':'text/javascript', '.css':'text/css'};
      res.writeHead(200, {'Content-Type':types[path.extname(file)] || 'application/octet-stream'}).end(bytes);
    } catch { res.writeHead(404).end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return `http://127.0.0.1:${server.address().port}`;
}
(async () => {
  const url=await previewUrl();
  const out=process.env.AIRSEND_SCREENSHOT_DIR || path.join(tmpdir(), 'airsend-ui-review');
  await mkdir(out,{recursive:true});
  browser=await chromium.launch({headless:true,executablePath:process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE || undefined});
  const errors=[];
  const context=await browser.newContext({viewport:{width:1120,height:760},colorScheme:'light'});
  await context.addInitScript(installDesktopMock);
  const page=await context.newPage();
  page.on('pageerror',e=>errors.push(e.message));
  await page.goto(url);
  await page.locator('.device').first().waitFor();
  await page.locator('.device').filter({hasText:'Office'}).getByRole('button',{name:'Connect',exact:true}).click();
  await page.locator('#play-stop').click();
  await page.waitForFunction(()=>document.querySelector('#player-status').textContent==='playing');
  await page.screenshot({path:`${out}/after-speakers-light.png`});
  assert.equal(await page.locator('#latency').getAttribute('step'),'50');
  assert.equal(await page.locator('#latency').getAttribute('min'),'100');
  const boundary=await page.evaluate(()=>({
    nav:document.querySelector('.navigation').getBoundingClientRect().toJSON(),
    content:document.querySelector('.view-container').getBoundingClientRect().toJSON(),
    player:document.querySelector('.playback-panel').getBoundingClientRect().toJSON(),
  }));
  const stateBefore=await page.evaluate(()=>window.__mockCalls.filter(x=>/^(stop_streaming|start_streaming|disconnect_device|save_)/.test(x.command)).length);
  for (const name of ['Audio','Diagnostics','Speakers']) {
    await page.getByRole('navigation').getByRole('button',{name,exact:true}).click();
    assert.equal(await page.locator('.view:visible').count(),1);
    assert.equal(await page.locator('.nav-item[aria-current="page"]').textContent(),name);
    assert.equal(await page.locator('#player-status').textContent(),'playing');
  }
  assert.equal(await page.evaluate(()=>window.__mockCalls.filter(x=>/^(stop_streaming|start_streaming|disconnect_device|save_)/.test(x.command)).length),stateBefore,'Navigation must not alter playback or preferences');
  await page.getByRole('navigation').getByRole('button',{name:'Audio',exact:true}).click();
  assert.equal(await page.locator('#audio-source').isDisabled(),true);
  assert.equal(await page.locator('#legacy-timing').isDisabled(),true);
  assert.equal(await page.locator('#experimental-local-buffer').isDisabled(),true);
  await page.screenshot({path:`${out}/after-audio-light.png`});
  await page.emulateMedia({colorScheme:'dark'});
  await page.screenshot({path:`${out}/after-audio-dark.png`});
  await page.setViewportSize({width:880,height:620});
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true,'Minimum desktop window must not overflow horizontally');
  assert.equal(await page.locator('#play-stop').isVisible(),true);
  await page.screenshot({path:`${out}/after-minimum.png`});
  await page.setViewportSize({width:1120,height:760});
  await page.locator('#lang-toggle').click();
  assert.equal(await page.getByRole('navigation').getByRole('button',{name:'Altavoces',exact:true}).count(),1);
  await page.getByRole('navigation').getByRole('button',{name:'Diagnóstico',exact:true}).click();
  assert.equal(await page.locator('#view-diagnostics').isVisible(),true);
  await page.getByRole('navigation').getByRole('button',{name:'Audio',exact:true}).click();
  assert.equal(await page.locator('#audio-source').isDisabled(),true);
  await page.locator('#play-stop').click();
  await page.waitForFunction(()=>!document.querySelector('#audio-source').disabled);
  await page.locator('#legacy-timing').check();
  await page.waitForFunction(()=>window.__mockCalls.some(x=>x.command==='save_audio_options'&&x.args.options.legacy_timing===true));
  await page.locator('#audio-source').selectOption('');
  await page.waitForFunction(()=>window.__mockCalls.some(x=>x.command==='save_audio_options'&&x.args.options.device_id===null));
  const idle=await context.newPage();
  idle.on('pageerror',e=>errors.push(e.message));
  await idle.goto(url);
  await idle.locator('.device').first().waitFor();
  assert.equal(await idle.locator('#player-empty').isVisible(),true);
  await idle.waitForFunction(()=>window.__mockCalls.some(x=>x.command==='set_tray_playback_state'&&!x.args.canStart&&!x.args.canStop));
  await idle.getByRole('navigation').getByRole('button',{name:'Audio',exact:true}).click();
  assert.equal(await idle.locator('#receiver-settings-empty').isVisible(),true);
  assert.equal(await idle.locator('#receiver-settings').isVisible(),false);
  await idle.getByRole('navigation').getByRole('button',{name:'Speakers',exact:true}).click();
  await idle.locator('.device').filter({hasText:'Office'}).getByRole('button',{name:'Connect',exact:true}).click();
  await idle.waitForFunction(()=>window.__mockCalls.filter(x=>x.command==='set_tray_playback_state').at(-1)?.args.canStart===true);
  // Tray actions reuse the selected receiver and ignore duplicate start/stop
  // events while an operation is pending, even with the window unfocused.
  await page.bringToFront();
  await idle.evaluate(()=>{
    window.__mockEmit('airplay://tray-playback','start');
    window.__mockEmit('airplay://tray-playback','start');
  });
  await idle.waitForFunction(()=>document.querySelector('#player-status').textContent==='playing');
  await idle.waitForFunction(()=>window.__mockCalls.filter(x=>x.command==='set_tray_playback_state').at(-1)?.args.canStop===true);
  assert.equal(await idle.evaluate(()=>window.__mockCalls.filter(x=>x.command==='start_streaming').length),1);
  await idle.evaluate(()=>window.__mockEmit('airplay://tray-playback','start'));
  assert.equal(await idle.evaluate(()=>window.__mockCalls.filter(x=>x.command==='start_streaming').length),1,'Starting from the tray must not restart active playback');
  await idle.evaluate(()=>{
    window.__mockEmit('airplay://tray-playback','stop');
    window.__mockEmit('airplay://tray-playback','stop');
  });
  await idle.waitForFunction(()=>window.__mockCalls.filter(x=>x.command==='set_tray_playback_state').at(-1)?.args.canStart===true);
  assert.equal(await idle.evaluate(()=>window.__mockCalls.filter(x=>x.command==='stop_streaming').length),1);
  assert.equal(await idle.evaluate(()=>window.__mockCalls.filter(x=>x.command==='disconnect_device').length),1);
  assert.equal(await idle.locator('#player-empty').isVisible(),false,'Stopping from the tray must retain the selected receiver');
  await idle.evaluate(()=>window.__mockEmit('airplay://tray-playback','start'));
  await idle.waitForFunction(()=>document.querySelector('#player-status').textContent==='playing');
  const trayStarts=await idle.evaluate(()=>window.__mockCalls.filter(x=>x.command==='start_streaming').map(x=>x.args));
  assert.equal(trayStarts.length,2);
  assert.deepEqual(trayStarts[1],trayStarts[0],'Quick start must reuse the receiver, volume and latency');
  await idle.setViewportSize({width:600,height:760});
  assert.equal(await idle.locator('#lang-toggle').isVisible(),true);
  assert.equal(await idle.locator('#check-updates').isVisible(),true);
  assert.deepEqual(errors,[]);
  await writeFile(`${out}/layout-bounds.json`,JSON.stringify(boundary,null,2));
  await browser.close();
  console.log('PASS: tray start/stop, duplicate actions, receiver/settings retention; navigation preserves streaming; source locks, stop/source change, Spanish labels, desktop sizing, and light/dark renders.');
})().catch(e=>{console.error(e);process.exitCode=1}).finally(async()=>{await browser?.close();await new Promise(resolve=>server ? server.close(resolve) : resolve());});
