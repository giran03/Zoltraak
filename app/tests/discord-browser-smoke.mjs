// Optional browser check; PLAYWRIGHT_MODULE points at an existing playwright-core/index.mjs.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { mkdtemp, readFile, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = await mkdtemp(path.join(tmpdir(), 'zoltraak-discord-ui-'));
const fixture = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { DiscordSettings } from './src/components/DiscordSettings.tsx';

const mode = new URL(location.href).searchParams.get('mode') || 'webhook';
const settings = { mode, enabled:false, autoSend:false, guildId:'g', channelId:'c', botUsername:'Fixture bot',
  guildName:'Fixture server', channelName:'downloads', uploadLimitBytes:10485760, compressToFit:false,
  messageTemplate:'', deleteAfterSend:false, groupFiles:false };
Object.assign(settings,JSON.parse(localStorage.getItem('discord-fixture-settings-'+mode)||'{}'));
const status = { ...settings, hasCredential:true, credentialHint:'1234', connected:true,
  uploadLimitDetected:mode === 'bot', uploadLimitLabel:'10 MiB', lastError:null };
window.calls = [];
window.__TAURI_INTERNALS__ = { invoke: async (name,args) => {
  window.calls.push(name);
  if(name === 'discord_status') return structuredClone(status);
  if(name === 'discord_settings') return structuredClone(settings);
  if(name === 'discord_deliveries') return [];
  if(name === 'discord_guilds') return [{id:'g',name:'Fixture server'}];
  if(name === 'discord_channels') return [{id:'c',name:'downloads',kind:0,position:0}];
  if(name === 'discord_bot_profile') return args.request.scope === 'global' ? {id:'42',username:'Fixture bot',avatar:null,banner:null}
    : args.request.scope === 'application' ? {id:'42',name:'Fixture application',description:'',icon:null,tags:[]} : {nick:null,bio:null,avatar:null,banner:null};
  if(name === 'update_discord_settings') {Object.assign(settings,args.settings);localStorage.setItem('discord-fixture-settings-'+mode,JSON.stringify(settings));return structuredClone(settings);}
  if(name === 'discord_reconnect') return new Promise((resolve,reject) => { window.finishReconnect = resolve; window.failReconnect = () => reject({message:'Saved credential rejected. Replace it and retry.'}); });
  throw new Error('Unexpected command: ' + name);
}};
createRoot(document.getElementById('root')).render(React.createElement(DiscordSettings));
`;
const bundle = await build({ stdin: { contents:fixture, resolveDir:root, sourcefile:'fixture.tsx', loader:'tsx' },
  bundle:true, write:false, format:'iife', platform:'browser', jsx:'automatic' });
const assets = path.join(root, 'dist', 'assets');
const cssName = (await readdir(assets)).find((name) => name.endsWith('.css'));
const css = await readFile(path.join(assets, cssName));
const server = createServer(async (req,res) => {
  if(req.url.startsWith('/assets/')) {
    const name = path.basename(req.url.split('?')[0]);
    try { res.setHeader('Content-Type', name.endsWith('.woff2') ? 'font/woff2' : 'text/css'); res.end(await readFile(path.join(assets,name))); }
    catch { res.writeHead(404); res.end(); }
  } else if(req.url === '/fixture.js') { res.setHeader('Content-Type','text/javascript'); res.end(bundle.outputFiles[0].text); }
  else if(req.url === '/fixture.css') { res.setHeader('Content-Type','text/css'); res.end(css); }
  else { res.setHeader('Content-Type','text/html'); res.end('<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="/fixture.css"><div id="root"></div><script src="/fixture.js"></script>'); }
});
await new Promise((resolve) => server.listen(0,'127.0.0.1',resolve));
const browser = await chromium.launch({ headless:true });
try {
  for(const mode of ['webhook','bot']) {
    const page = await browser.newPage({ viewport:{width:1200,height:900} });
    page.setDefaultTimeout(15000);
    page.on('pageerror', (error) => console.error(error.message));
    console.log(`Opening ${mode}`);
    await page.goto(`http://127.0.0.1:${server.address().port}/discord-fixture?mode=${mode}`, { waitUntil:'domcontentloaded', timeout:15000 });
    const reconnect = page.getByRole('button',{name:'Reconnect',exact:true});
    await reconnect.waitFor();
    await reconnect.click();
    assert.equal(await page.getByRole('button',{name:'Reconnecting…',exact:true}).isDisabled(),true);
    assert.equal(await page.getByLabel('Discord delivery method').isDisabled(),true);
    await page.evaluate(() => window.finishReconnect({}));
    await page.getByText('Connection verified with Discord. Delivery settings are unchanged.').waitFor();
    assert.equal(await page.getByRole('switch',{name:'Discord delivery enabled'}).getAttribute('aria-checked'),'false');
    await reconnect.click();
    await page.evaluate(() => window.failReconnect());
    await page.getByText('Saved credential rejected. Replace it and retry.').waitFor();
    const polls = await page.evaluate(() => window.calls.filter((name) => name === 'discord_status').length);
    await page.waitForFunction((before) => window.calls.filter((name) => name === 'discord_status').length > before, polls);
    assert.equal(await page.getByText('Saved credential rejected. Replace it and retry.').isVisible(),true);
    assert.equal(await page.evaluate(() => window.calls.filter((name) => name === 'discord_reconnect').length),2);
    assert.equal(await page.evaluate(() => window.calls.some((name) => name === 'discord_test' || name === 'update_discord_settings')),false);
    if(mode==='bot'){
      const limit=page.getByRole('spinbutton',{name:'Upload limit (megabytes)'});
      const save=page.getByRole('button',{name:'Save limit',exact:true});
      assert.equal(await limit.inputValue(),'20');
      await limit.fill('0');assert.equal(await save.isDisabled(),true);
      await limit.fill('32.5');await save.click();
      await page.getByText('Upload limit saved: 32.5 MB.',{exact:true}).waitFor();
      assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('discord-fixture-settings-bot')).botUploadLimitBytes),32500000);
      await page.reload();
      await limit.waitFor();assert.equal(await limit.inputValue(),'32.5');
      await limit.fill('40');
      await page.waitForTimeout(1200);
      assert.equal(await limit.inputValue(),'40','polling must preserve the unsaved draft');
    }
    await page.screenshot({path:path.join(output,`${mode}-desktop.png`),fullPage:true});
    await page.setViewportSize({width:390,height:844});
    await page.screenshot({path:path.join(output,`${mode}-mobile.png`),fullPage:true});
    const bounds = await reconnect.boundingBox();
    assert.ok(bounds && bounds.x >= 0 && bounds.x + bounds.width <= 390, 'Reconnect stays within the narrow viewport');
    console.log(`PASS ${mode}: loading, preserved settings, polling-resistant errors, responsive reconnect control`);
    await page.close();
  }
  console.log(`Discord UI artifacts: ${output}`);
} finally { await browser.close(); await new Promise((resolve) => server.close(resolve)); }
