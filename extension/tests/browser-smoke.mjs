// Optional real-browser smoke: uses existing Playwright/web-ext installations.
// Set PLAYWRIGHT_MODULE to playwright-core/index.mjs and WEB_EXT_REMOTE_MODULE
// to web-ext/lib/firefox/remote.js. No production engine or user profile is used.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, cp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const { chromium, firefox } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const extension = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const scratch = await mkdtemp(path.join(tmpdir(), 'zoltraak-browser-smoke-'));
console.log(`Artifacts: ${scratch}`);
const receivedUrls = [];
const server = createServer((req, res) => {
  receivedUrls.push(req.url);
  if (req.url.startsWith('/stream.m3u8')) {
    res.writeHead(200, { 'Content-Type': 'application/vnd.apple.mpegurl' });
    res.end('#EXTM3U\n#EXTINF:10,\nsegment.ts\n#EXT-X-ENDLIST\n'); return;
  }
  if (req.url === '/media' || req.url === '/iframe') {
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(req.url === '/media'
      ? '<!doctype html><title>Media fixture</title><video style="width:640px;height:360px"></video><iframe src="/iframe"></iframe><script>fetch("/stream.m3u8?sig=a%2Fb+z&x=1").then(() => window.done=true)</script>'
      : '<!doctype html><script>fetch("/stream.m3u8?other=iframe").then(() => window.done=true)</script>'); return;
  }
  if (req.url === '/redirect') { res.writeHead(302, { Location: '/attachment' }); res.end(); return; }
  if (req.url === '/attachment' || req.url === '/small.bin') {
    res.writeHead(200, { 'Content-Type': 'text/plain', 'Content-Disposition': 'attachment; filename="fixture.txt"' });
    res.end('small local file'); return;
  }
  res.writeHead(200, { 'Content-Type': 'text/html' });
  res.end('<!doctype html><title>Download fixture</title><a id="explicit" href="/small.bin" download="fixture.bin">Download</a><a id="attachment" href="/attachment">Attachment</a><a id="redirect" href="/redirect">Redirect</a>');
});
await new Promise((resolve) => server.listen(0, '0.0.0.0', resolve));
const port = server.address().port;

async function prepare(kind) {
  const dest = path.join(scratch, kind);
  await cp(extension, dest, { recursive: true, filter: (source) => !source.startsWith(path.join(extension, 'tests')) });
  const manifest = JSON.parse(await readFile(path.join(dest, 'manifest.json'), 'utf8'));
  if (kind === 'chromium') {
    delete manifest.browser_specific_settings;
    manifest.permissions = manifest.permissions.filter((permission) => permission !== 'webRequestBlocking');
    manifest.background = { service_worker: 'background.js', type: 'module' };
  }
  manifest.content_scripts.push({ matches: ['http://*/*'], js: ['fixture-probe.js'], run_at: 'document_idle' });
  await writeFile(path.join(dest, 'manifest.json'), JSON.stringify(manifest));
  const bg = await readFile(path.join(dest, 'background.js'), 'utf8');
  const names = bg.match(/import\s*\{([\s\S]+?)\}\s*from "\.\/api\.js"/)[1].split(',').map((name) => name.trim()).filter(Boolean);
  await writeFile(path.join(dest, 'api.js'), names.map((name) => {
    if (name === 'ENGINE_ORIGIN') return 'export const ENGINE_ORIGIN = "http://127.0.0.1:1450";';
    const body = ({
      health: 'true', ensurePaired: 'true', verify: 'true', canUseBrowserCookies: 'false',
      getToken: '"fixture"', cookieHeader: 'null', youtubeSessionCookie: 'null',
      getSnapshot: '({ok:true,items:[],videoJobs:[]})',
      extensionStatusCursor: '({reloadVersion:0,sourceRevision:0})',
      apiFetch: 'args[0].startsWith("/extension/status") ? new Promise(() => {}) : new Response(JSON.stringify(args[0] === "/settings" ? {defaultDirectory:"C:/Downloads"} : []))',
    })[name] || '({ok:true})';
    return `export const ${name} = async (...args) => ${body};`;
  }).join('\n'));
  await writeFile(path.join(dest, 'background.js'), bg + `\nconst fixtureEvents = [];
  chrome.downloads.onCreated.addListener(item => { fixtureEvents.push({created:item}); chrome.storage.local.set({fixtureEvents}); });
  chrome.downloads.onChanged.addListener(item => { fixtureEvents.push({changed:item}); chrome.storage.local.set({fixtureEvents}); });
  chrome.runtime.onMessage.addListener((message, sender, reply) => {
    if(message.type !== 'fixture-state') return;
    Promise.all([chrome.storage.local.get(null), chrome.downloads.search({})]).then(([state, downloads]) => reply({...state, downloads})); return true;
  });\n`);
  await writeFile(path.join(dest, 'fixture-probe.js'), `document.addEventListener('fixture-state', () => {
    chrome.runtime.sendMessage({type:'fixture-state'}).then(state => document.documentElement.setAttribute('data-fixture-state', JSON.stringify(state)));
  });
  document.addEventListener('fixture-media', () => {
    chrome.runtime.sendMessage({type:'zoltraak-tab-media',documentStartedAt:performance.timeOrigin}).then(state => document.documentElement.setAttribute('data-fixture-media',JSON.stringify(state)));
  }); document.documentElement.setAttribute('data-fixture-ready','true');`);
  return dest;
}

async function state(page) {
  await page.evaluate(() => { document.documentElement.removeAttribute('data-fixture-state'); document.dispatchEvent(new Event('fixture-state')); });
  await page.waitForFunction(() => document.documentElement.hasAttribute('data-fixture-state'));
  return JSON.parse(await page.locator('html').getAttribute('data-fixture-state'));
}

try {
  for (const kind of ['chromium', 'firefox']) {
    const dest = await prepare(kind);
    let remote;
    let context;
    let nativeProcess;
    let connectedBrowser;
    try {
      if (kind === 'chromium') {
        await mkdir(path.join(scratch, 'chrome-profile', 'Default'), { recursive: true });
        await writeFile(path.join(scratch, 'chrome-profile', 'Default', 'Preferences'), JSON.stringify({
          download: { default_directory: path.join(scratch, 'native-downloads'), prompt_for_download: false },
        }));
        const profile = path.join(scratch, 'chrome-profile');
        nativeProcess = spawn(chromium.executablePath(), ['--headless=new', '--remote-debugging-port=0',
          `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check',
          '--host-resolver-rules=MAP media.test 127.0.0.1', '--no-proxy-server',
          `--disable-extensions-except=${dest}`, `--load-extension=${dest}`, 'about:blank'], { windowsHide: true, stdio: 'ignore' });
        let debugPort;
        for (let attempt = 0; attempt < 100; attempt++) {
          try { debugPort = (await readFile(path.join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; break; }
          catch { await new Promise((resolve) => setTimeout(resolve, 100)); }
        }
        assert.ok(debugPort, 'Chromium debugging port did not start');
        // Playwright's default download handling bypasses onDeterminingFilename.
        connectedBrowser = await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`, { noDefaults: true });
        context = connectedBrowser.contexts()[0];
      } else {
        const debugServer = createServer();
        await new Promise((resolve) => debugServer.listen(0, '127.0.0.1', resolve));
        const debugPort = debugServer.address().port;
        await new Promise((resolve) => debugServer.close(resolve));
        context = await firefox.launchPersistentContext(path.join(scratch, 'firefox-profile'), {
          executablePath: process.env.FIREFOX_EXECUTABLE,
          headless: true, args: ['--start-debugger-server', String(debugPort)],
          firefoxUserPrefs: { 'devtools.debugger.remote-enabled': true, 'devtools.debugger.prompt-connection': false,
            'devtools.chrome.enabled': true, 'xpinstall.signatures.required': false,
            'network.dns.localDomains': 'media.test', 'network.proxy.type': 0 },
        });
        const { connect } = await import(pathToFileURL(process.env.WEB_EXT_REMOTE_MODULE).href);
        remote = await connect(debugPort);
        await remote.installTemporaryAddon(dest, false);
      }
      const errors = [];
      let expected = 0;
      for (const host of ['127.0.0.1', 'localhost', 'media.test']) {
        for (const link of ['explicit', 'attachment', 'redirect']) {
          const page = await context.newPage();
          page.setDefaultTimeout(12000);
          page.on('pageerror', (error) => errors.push(error.message));
          await page.goto(`http://${host}:${port}/`);
          await page.waitForFunction(() => document.documentElement.hasAttribute('data-fixture-ready'));
          await page.locator(`#${link}`).click({ noWaitAfter: true });
          expected++;
          let captured;
          for (let attempt = 0; attempt < 30; attempt++) {
            captured = await state(page);
            if (Object.keys(captured).filter((key) => key.startsWith('intercept:')).length === expected && captured.lastInterception?.stage === 'captured') break;
            await new Promise((resolve) => setTimeout(resolve, 100));
          }
          assert.equal(Object.keys(captured).filter((key) => key.startsWith('intercept:')).length, expected, `${kind} ${host} ${link}: ${JSON.stringify({ diagnostic: captured.lastInterception, events: captured.fixtureEvents, intercept:captured.fixtureIntercept, downloads:captured.downloads, receivedUrls })}`);
          assert.equal(captured.lastInterception.stage, 'captured');
          console.log(`PASS ${kind}: ${host} ${link}`);
          if (link === 'redirect') await page.screenshot({ path: path.join(scratch, `${kind}-${host}-interception.png`) });
          await page.close();
        }
      }
      assert.deepEqual(errors, []);
      const mediaPage = await context.newPage();
      await mediaPage.goto(`http://127.0.0.1:${port}/media`);
      await mediaPage.waitForFunction(() => window.done && document.querySelector('iframe').contentWindow.done && document.documentElement.hasAttribute('data-fixture-ready'));
      await mediaPage.evaluate(() => document.dispatchEvent(new Event('fixture-media')));
      await mediaPage.waitForFunction(() => document.documentElement.hasAttribute('data-fixture-media'));
      const media = JSON.parse(await mediaPage.locator('html').getAttribute('data-fixture-media'));
      assert.deepEqual(media.media.map((entry) => entry.url), [`http://127.0.0.1:${port}/stream.m3u8?sig=a%2Fb+z&x=1`]);
      console.log(`PASS ${kind}: signed HLS capture excludes another iframe's stream`);
      await mediaPage.close();
    } finally {
      remote?.disconnect();
      await context?.close();
      await connectedBrowser?.close();
      nativeProcess?.kill();
    }
  }
  console.log(`Browser smoke passed. Artifacts: ${scratch}`);
} finally { await new Promise((resolve) => server.close(resolve)); }
