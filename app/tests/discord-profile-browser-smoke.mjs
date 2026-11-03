// Run after npm run build. PLAYWRIGHT_MODULE points to an installed playwright-core/index.mjs.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { mkdtemp, readFile, readdir, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = process.env.PROFILE_SCREENSHOT_DIR || await mkdtemp(path.join(tmpdir(), 'zoltraak-bot-profile-'));
await mkdir(output, { recursive: true });
const fixture = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { DiscordSettings } from './src/components/DiscordSettings.tsx';
const mode = new URL(location.href).searchParams.get('mode') || 'bot';
const settings = {mode,enabled:false,autoSend:false,guildId:'7',channelId:'8',guildName:'Download server',channelName:'downloads',
  botUsername:'Archive bot',uploadLimitBytes:10000000,botUploadLimitBytes:20000000,compressToFit:false,compressMaxHeight:720,
  messageTemplate:'{filename} · {size}',includeSourceUrl:false,notifyOnDelivery:false,pingSender:false,pingMessageTemplate:'{discord user id}'};
const profiles = {
  global:{id:'42',username:'Archive bot',avatar:null,banner:null},
  application:{id:'42',name:'Archive application',description:'Save your downloads to Discord.',icon:null,tags:['download'],bot_public:false},
  server:{nick:'Server bot',bio:'Server-only profile',avatar:null,banner:null}
};
window.calls=[]; window.failProfile=false; window.profiles=profiles;
window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
  window.calls.push({name,args});
  if(name==='discord_status')return {...settings,hasCredential:true,credentialHint:'1234',connected:true,uploadLimitLabel:'10 MiB',lastError:null};
  if(name==='discord_settings')return structuredClone(settings);
  if(name==='discord_deliveries')return [];
  if(name==='discord_guilds')return [{id:'7',name:'Download server'}];
  if(name==='discord_channels')return [{id:'8',name:'downloads',kind:0,position:0}];
  if(name==='update_discord_settings'){Object.assign(settings,args.settings);return structuredClone(settings);}
  if(name==='discord_bot_profile'){
    const {scope,fields}=args.request;
    if(fields){
      if(window.failProfile)throw {message:'Rate limited for 3600s. Retry later.'};
      Object.assign(profiles[scope],fields);
      if(scope==='global')settings.botUsername=profiles.global.username;
    }
    return structuredClone(profiles[scope]);
  }
  if(name==='discord_bot_diagnostics')return {credentialValid:true,destinationSelected:true,checks:[
    {label:'View channel',allowed:true},{label:'Send messages',allowed:true},{label:'Attach files',allowed:false},{label:'Embed links',allowed:true}]};
  if(name==='discord_reconnect')return {botUsername:settings.botUsername};
  throw new Error('Unexpected command: '+name);
}};
createRoot(document.getElementById('root')).render(<DiscordSettings />);
`;
const bundle = await build({ stdin: { contents: fixture, resolveDir: root, sourcefile: 'fixture.tsx', loader: 'tsx' },
  bundle: true, write: false, format: 'iife', platform: 'browser', jsx: 'automatic' });
const assets = path.join(root, 'dist', 'assets');
const cssName = (await readdir(assets)).find(name => name.endsWith('.css'));
const css = await readFile(path.join(assets, cssName));
const server = createServer(async (req, res) => {
  if (req.url.startsWith('/assets/')) {
    const name = path.basename(req.url.split('?')[0]);
    try { res.setHeader('Content-Type', name.endsWith('.woff2') ? 'font/woff2' : 'text/css'); res.end(await readFile(path.join(assets, name))); }
    catch { res.writeHead(404); res.end(); }
  } else if (req.url === '/fixture.js') { res.setHeader('Content-Type', 'text/javascript'); res.end(bundle.outputFiles[0].text); }
  else if (req.url === '/fixture.css') { res.setHeader('Content-Type', 'text/css'); res.end(css); }
  else {
    res.setHeader('Content-Type', 'text/html');
    // Match the desktop app's image policy: crops must work without blob: permission.
    res.setHeader('Content-Security-Policy', "img-src 'self' data: https:");
    res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="/fixture.css"><body><div id="root"></div><script src="/fixture.js"></script></body></html>');
  }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ headless: true });
const url = `http://127.0.0.1:${server.address().port}`;
const errors = [];
try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url);
  const name = page.getByRole('textbox', { name: 'Bot name', exact: true });
  await name.waitFor();
  const global = page.locator('section').filter({ has: name });
  const application = page.locator('section').filter({ has: page.getByRole('textbox', { name: 'Application description', exact: true }) });
  const member = page.locator('section').filter({ has: page.getByRole('textbox', { name: 'Server nickname', exact: true }) });
  assert.equal(await name.inputValue(), 'Archive bot');
  await name.fill('Renamed bot');
  await page.evaluate(() => { window.failProfile = true; });
  await global.getByRole('button', { name: 'Save changes', exact: true }).click();
  await global.getByText(/Your draft is preserved/).waitFor();
  assert.equal(await name.inputValue(), 'Renamed bot');
  await page.evaluate(() => { window.failProfile = false; });
  await global.getByRole('button', { name: 'Save changes', exact: true }).click();
  await global.getByText('Bot profile saved to Discord.').waitFor();
  assert.equal(await page.evaluate(() => window.profiles.global.username), 'Renamed bot');
  await name.fill('Discard me');
  assert.equal(await global.getByRole('button', { name: 'Refresh from Discord' }).isDisabled(), true);
  await global.getByRole('button', { name: 'Discard', exact: true }).click();
  assert.equal(await name.inputValue(), 'Renamed bot');

  const image = Buffer.from(await page.evaluate(() => {
    const canvas = document.createElement('canvas'); canvas.width = 1000; canvas.height = 800;
    const context = canvas.getContext('2d'); context.fillStyle = '#376681'; context.fillRect(0, 0, 500, 800);
    context.fillStyle = '#829984'; context.fillRect(500, 0, 500, 800); return canvas.toDataURL().split(',')[1];
  }), 'base64');
  await global.getByLabel('Upload bot avatar', { exact: true }).setInputFiles({ name: 'avatar.png', mimeType: 'image/png', buffer: image });
  await global.getByRole('button', { name: 'Use crop', exact: true }).waitFor();
  assert.equal(await global.getByRole('button', { name: 'Save changes', exact: true }).isDisabled(), true);
  await global.getByLabel('Horizontal', { exact: true }).fill('80');
  await global.getByRole('button', { name: 'Use crop', exact: true }).click();
  await global.getByLabel('Upload bot banner', { exact: true }).setInputFiles({ name: 'banner.png', mimeType: 'image/png', buffer: image });
  await global.getByRole('button', { name: 'Use crop', exact: true }).click();
  await global.getByRole('button', { name: 'Save changes', exact: true }).click();
  await global.getByText('Bot profile saved to Discord.').waitFor();
  const pixels = await page.evaluate(async () => {
    const decode = async source => {const image = new Image(); image.src=source;await image.decode();return [image.width,image.height];};
    return {avatar:await decode(window.profiles.global.avatar),banner:await decode(window.profiles.global.banner)};
  });
  assert.deepEqual(pixels, { avatar: [512, 512], banner: [1200, 480] });
  await global.screenshot({ path: path.join(output, 'bot-profile-desktop.png') });
  await global.getByRole('button', { name: 'Remove bot avatar', exact: true }).click();
  await global.getByRole('button', { name: 'Save changes', exact: true }).click();
  await global.getByText('Bot profile saved to Discord.').waitFor();
  assert.equal(await page.evaluate(() => window.profiles.global.avatar), null);
  await global.getByLabel('Upload bot avatar', { exact: true }).setInputFiles({ name: 'bad.png', mimeType: 'image/png', buffer: Buffer.from('not an image') });
  await global.getByRole('alert').waitFor();
  await global.getByRole('button', { name: 'Dismiss', exact: true }).click();

  await application.getByRole('textbox', { name: 'Application description', exact: true }).fill('Updated application description.');
  await application.getByRole('textbox', { name: 'Application tags', exact: true }).fill('downloads, media');
  await application.getByRole('button', { name: 'Save changes', exact: true }).click();
  await application.getByText('Application details saved to Discord.').waitFor();
  assert.deepEqual(await page.evaluate(() => window.profiles.application.tags), ['downloads', 'media']);
  await application.getByRole('textbox', { name: 'Application tags', exact: true }).fill('one,two,three,four,five,six');
  await application.getByRole('button', { name: 'Save changes', exact: true }).click();
  await application.getByText('Use at most five tags, each up to 20 characters.').waitFor();
  await application.getByRole('button', { name: 'Discard', exact: true }).click();

  await member.getByRole('button', { name: 'Reset server overrides', exact: true }).click();
  assert.equal(await page.evaluate(() => window.profiles.server.nick), 'Server bot', 'reset is staged');
  await member.getByRole('button', { name: 'Save changes', exact: true }).click();
  await member.getByText('Server profile saved to Discord.').waitFor();
  assert.equal(await page.evaluate(() => window.profiles.server.nick), null);
  await page.getByRole('button', { name: 'Check connection and permissions' }).click();
  await page.getByText('Unavailable', { exact: true }).waitFor();
  await page.getByRole('switch', { name: 'Include source link', exact: true }).click();
  assert.match(await page.getByLabel('Discord message preview').innerText(), /https:\/\/example.com/);
  assert.deepEqual(errors, []);

  // Every PATCH is explicitly initiated by a profile save, never by choosing an image or changing a field.
  const patches = await page.evaluate(() => window.calls.filter(c => c.name === 'discord_bot_profile' && c.args.request.fields).map(c => c.args.request));
  assert.equal(patches.filter(p => p.scope === 'global').length, 4);
  assert.deepEqual(patches.find(p => p.scope === 'server').fields, { nick: null, bio: null, avatar: null, banner: null });
  for (const width of [560, 390]) {
    await page.setViewportSize({ width, height: 900 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `no overflow at ${width}`);
    await global.screenshot({ path: path.join(output, `bot-profile-${width}.png`) });
  }
  await page.close();
  const webhook = await browser.newPage();
  await webhook.goto(`${url}?mode=webhook`);
  await webhook.getByRole('button', { name: 'Reconnect', exact: true }).waitFor();
  assert.equal(await webhook.getByRole('textbox', { name: 'Bot name', exact: true }).count(), 0);
  assert.equal(await webhook.evaluate(() => window.calls.filter(c => c.name === 'discord_bot_profile').length), 0);
  console.log(`Discord profile smoke passed. Screenshots: ${output}`);
} finally {
  await browser.close(); await new Promise(resolve => server.close(resolve));
}
