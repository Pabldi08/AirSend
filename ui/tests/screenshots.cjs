// Render the real UI with test-only, anonymous receiver data. No hardware access.
const {chromium} = require('playwright');
const {installDesktopMock} = require('./desktop-mock.cjs');
const {readFile, mkdir} = require('node:fs/promises');
const http = require('node:http');
const path = require('node:path');
(async () => {
  const dist = path.resolve(__dirname, '../dist');
  const out = path.resolve(__dirname, '../../docs/screenshots');
  await mkdir(out, {recursive:true});
  const server = http.createServer(async (req, res) => {
    try {
      const pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
      const file = path.resolve(dist, '.' + (pathname === '/' ? '/index.html' : pathname));
      if (!file.startsWith(dist + path.sep)) { res.writeHead(403).end(); return; }
      const bytes = await readFile(file);
      res.writeHead(200, {'Content-Type': {'.html':'text/html','.js':'text/javascript','.css':'text/css'}[path.extname(file)] || 'application/octet-stream'}).end(bytes);
    } catch { res.writeHead(404).end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch({headless:true});
    const context = await browser.newContext({viewport:{width:1120,height:760}, colorScheme:'light'});
    await context.addInitScript(installDesktopMock);
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.locator('.device').first().waitFor();
    await page.locator('.device').filter({hasText:'Office'}).getByRole('button',{name:'Connect',exact:true}).click();
    await page.locator('#play-stop').click();
    await page.waitForFunction(()=>document.querySelector('#player-status').textContent==='playing');
    await page.screenshot({path:path.join(out,'speakers-light.png')});
    await page.getByRole('navigation').getByRole('button',{name:'Audio',exact:true}).click();
    await page.screenshot({path:path.join(out,'audio-light.png')});
    await page.emulateMedia({colorScheme:'dark'});
    await page.screenshot({path:path.join(out,'audio-dark.png')});
    if(errors.length) throw new Error(errors.join('\n'));
    console.log('Captured three UI previews using anonymous test data; no real devices or personal names accessed.');
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => {console.error(error);process.exitCode=1;});
