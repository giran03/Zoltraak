// Render the shipped popup with deterministic browser/API fixtures.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.resolve(root, '../scratch/popup-two-column');
const manifest = JSON.parse(await readFile(path.join(root, 'manifest.json'), 'utf8'));
const { chromium, firefox } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const api = `
export const ensurePaired = async () => ({paired:true});
export const health = async () => window.fixtureUp !== false;
export const getToken = async () => window.fixtureToken || 'pairing-token-fixture';
export const setToken = async value => {window.fixtureToken=value;};
export const verify = async token => Boolean(token);
export const reconnect = async () => ({paired:true});
`;
const server = createServer(async (req, res) => {
  const name = new URL(req.url, 'http://localhost').pathname.slice(1) || 'popup.html';
  if (name === 'api.js') { res.setHeader('Content-Type', 'text/javascript'); res.end(api); return; }
  if (!/^(popup\.(html|css|js)|theme\.js|placement-controller\.js|fonts\/[\w-]+\.woff2)$/.test(name)) { res.writeHead(404); res.end(); return; }
  try {
    res.setHeader('Content-Type', name.endsWith('.html') ? 'text/html' : name.endsWith('.css') ? 'text/css' : name.endsWith('.js') ? 'text/javascript' : 'font/woff2');
    res.end(await readFile(path.join(root, name)));
  } catch { res.writeHead(404); res.end(); }
});
await mkdir(output, {recursive:true});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await (process.env.BROWSER_ENGINE === 'firefox' ? firefox : chromium).launch({headless:true, executablePath:process.env.BROWSER_ENGINE === 'firefox' ? process.env.FIREFOX_EXECUTABLE : undefined});
try {
  for (const colorScheme of ['dark', 'light']) {
    const context = await browser.newContext({viewport:{width:680,height:600},colorScheme});
    await context.addInitScript(({version}) => {
      const stored = {captureAll:true,enableSfx:true,showMediaButton:true,showSessionPill:false};
      const listeners = [];
      window.fixtureCalls = [];
      window.chrome = {
        storage:{local:{
          get:async keys => keys == null ? {...stored} : typeof keys === 'string' ? {[keys]:stored[keys]} : Array.isArray(keys) ? Object.fromEntries(keys.map(key=>[key,stored[key]])) : {...keys,...stored},
          set:async values => {const changes=Object.fromEntries(Object.entries(values).map(([key,newValue])=>[key,{oldValue:stored[key],newValue}]));Object.assign(stored,values);for(const listener of listeners)listener(changes,'local');},
        },onChanged:{addListener:fn=>listeners.push(fn)}},
        runtime:{getManifest:()=>({version}),getURL:name=>new URL(name,location.origin+'/').href,reload:()=>window.fixtureCalls.push({type:'reload'}),sendMessage:async message=>{window.fixtureCalls.push(message);return {ok:true,format:{hasAudio:true},count:2};}},
        tabs:{query:async()=>[{id:42,url:'https://fixture.example/watch'}],sendMessage:async()=>({candidates:[]}),create:async value=>window.fixtureCalls.push(value)},
      };
    }, {version:manifest.version});
    const page = await context.newPage();
    const errors=[]; page.on('pageerror', error=>errors.push(error.message));
    await page.goto(`http://127.0.0.1:${server.address().port}/popup.html`);
    await page.locator('#statusWord').filter({hasText:'line open'}).waitFor();
    await page.evaluate(()=>document.fonts.ready);
    const layout = await page.evaluate(()=>{
      const box = el => {const r=el.getBoundingClientRect();return {x:r.x,y:r.y,right:r.right,bottom:r.bottom,width:r.width,height:r.height};};
      return {body:box(document.body),left:box(document.querySelector('.actions-column')),right:box(document.querySelector('.settings-column')),footer:box(document.querySelector('footer')),overflow:document.documentElement.scrollWidth>680};
    });
    assert.equal(layout.overflow,false);
    assert.equal(layout.body.width,680);
    assert.ok(layout.body.height<=600);
    assert.ok(layout.right.x>=layout.left.right-1);
    assert.equal(layout.left.y,layout.right.y);
    const scrollArea = await page.locator('.popup-columns').boundingBox();
    assert.ok(layout.footer.y >= scrollArea.y + scrollArea.height - 1);
    await page.locator('#orderGuide').scrollIntoViewIfNeeded();
    const lastAction = await page.locator('#orderGuide').boundingBox();
    assert.ok(lastAction.y + lastAction.height <= layout.footer.y + 1);
    assert.equal(await page.locator('footer #extVersion').textContent(),`v${manifest.version}`);
    assert.equal(await page.locator('.actions-column .orders > button').count(),6);
    for(const [name,id] of [['Capture all browser downloads','captureAll'],['Sound notifications','enableSfx'],['Download button on videos','showMediaButton'],['Dispatch Board pill','showSessionPill']]) {
      const control=page.getByRole('switch',{name,exact:true});
      const before=await control.getAttribute('aria-checked'); await control.click();
      assert.notEqual(await control.getAttribute('aria-checked'),before);
      assert.equal(await control.getAttribute('id'),id);
      await control.click();
    }
    await page.screenshot({path:path.join(output,`popup-${colorScheme}.png`),clip:{x:0,y:0,width:680,height:Math.ceil(layout.body.height)}});
    await page.locator('#openPlacementModal').click();
    await page.locator('#placementModal:not([hidden])').waitFor();
    await page.getByRole('button',{name:'Close modal',exact:true}).click();
    await page.locator('#placementModal[hidden]').waitFor({state:'attached'});
    await page.getByRole('textbox',{name:'API token',exact:true}).fill('new-pairing-fixture');
    await page.getByRole('button',{name:'Save',exact:true}).click();
    assert.equal(await page.getByRole('textbox',{name:'API token',exact:true}).inputValue(),'new-pairing-fixture');
    await page.locator('#orderBest').click();
    await page.locator('#bestResult').filter({hasText:/./}).waitFor();
    await page.getByRole('button',{name:'Legal & Privacy',exact:true}).click();
    assert.ok(await page.evaluate(()=>window.fixtureCalls.some(call=>call.url?.endsWith('/legal'))));
    // Long diagnostics scroll inside the columns, keeping footer metadata visible.
    await page.evaluate(()=>{document.getElementById('interceptionResult').textContent='Download diagnostic '.repeat(100);});
    const footer=await page.locator('footer').boundingBox();
    assert.ok(footer.y+footer.height<=600);
    assert.deepEqual(errors,[]);
    await context.close();
  }
  console.log(`Popup layout and controls passed (${process.env.BROWSER_ENGINE || 'chromium'}). Artifacts: ${output}`);
} finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
