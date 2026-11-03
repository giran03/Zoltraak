// Run after npm run build; PLAYWRIGHT_MODULE points to playwright/index.mjs.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const fixture = `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { PanelHeader } from './src/components/PanelHeader.tsx';
window.calls = [];
if (!location.search.includes('browser')) {
  let maximized = false, nextId = 0;
  const callbacks = new Map(), listeners = new Map();
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_, id) => listeners.delete(id) };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' } },
    transformCallback: cb => { callbacks.set(++nextId, cb); return nextId; },
    invoke: async (command, args) => {
      window.calls.push(command);
      if (command === 'plugin:event|listen') { const id = ++nextId; listeners.set(id, args); return id; }
      if (command === 'plugin:event|unlisten') return;
      if (command === 'plugin:window|is_maximized') return maximized;
      if (command === 'plugin:window|toggle_maximize') {
        maximized = !maximized;
        for (const [id, entry] of listeners) if (entry.event === 'tauri://resize')
          callbacks.get(entry.handler)?.({ event: entry.event, id, payload: {width:1450,height:840} });
        return;
      }
      if (command === 'plugin:window|minimize' || command === 'plugin:window|close') return;
      throw new Error('Unexpected command: ' + command);
    }
  };
}
function Fixture() {
  const [active, setActive] = useState('Downloads');
  return <PanelHeader active={active} onSelect={setActive} status="live" throughput={0}
    onOpenExtension={() => {}} onOpenShortcuts={() => {}} />;
}
createRoot(document.getElementById('root')).render(<Fixture />);
`;
const bundle = await build({ stdin: {contents:fixture,resolveDir:root,sourcefile:'fixture.tsx',loader:'tsx'},
  bundle:true,write:false,format:'iife',platform:'browser',jsx:'automatic' });
const assets = path.join(root, 'dist/assets');
const css = await readFile(path.join(assets, (await readdir(assets)).find(name => name.endsWith('.css'))));
const server = createServer((req,res) => {
  if (req.url === '/fixture.js') { res.setHeader('Content-Type','text/javascript'); res.end(bundle.outputFiles[0].text); }
  else if (req.url === '/fixture.css') { res.setHeader('Content-Type','text/css'); res.end(css); }
  else { res.setHeader('Content-Type','text/html'); res.end('<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="/fixture.css"><div id="root"></div><script src="/fixture.js"></script>'); }
});
await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
const browser = await chromium.launch({headless:true});
try {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const url = 'http://127.0.0.1:' + server.address().port;
  await page.goto(url);
  await page.getByRole('button',{name:'Maximize window',exact:true}).waitFor();
  for (const width of [640,1024,1360,1450,1600]) {
    await page.setViewportSize({width,height:840});
    for (const glass of [false,true]) {
      await page.evaluate(glass => {
        document.body.dataset.effectGlass = String(glass);
        document.body.dataset.glassNavbar = String(glass);
      },glass);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),true,'Header must fit at '+width);
      const bounds = await page.locator('.window-controls').boundingBox();
      assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width,'Controls must remain inside the window');
      const tools = await page.locator('.header-tools').boundingBox();
      assert.ok(tools.x + tools.width <= bounds.x + 1,'Tools must not overlap window controls');
      assert.ok(bounds.height >= 32,'Window controls need usable hit targets');
    }
  }
  await page.getByRole('button',{name:'Minimize window',exact:true}).click();
  await page.getByRole('button',{name:'Maximize window',exact:true}).click();
  await page.getByRole('button',{name:'Restore window',exact:true}).click();
  await page.getByRole('button',{name:'Maximize window',exact:true}).waitFor();
  await page.getByRole('button',{name:'Close window',exact:true}).click();
  const calls = await page.evaluate(() => window.calls);
  assert.equal(calls.filter(call => call === 'plugin:window|toggle_maximize').length,2);
  assert.ok(calls.includes('plugin:window|minimize'));
  assert.ok(calls.includes('plugin:window|close'));
  await page.getByRole('button',{name:'Video',exact:true}).click();
  assert.equal(await page.getByRole('button',{name:'Video',exact:true}).getAttribute('aria-current'),'page');
  assert.equal(await page.getByRole('button',{name:'Video',exact:true}).getAttribute('data-tauri-drag-region'),null);
  assert.equal(await page.locator('header > div').first().getAttribute('data-tauri-drag-region'),'true');
  await page.goto(url+'?browser');
  await page.locator('header').waitFor();
  assert.equal(await page.getByRole('group',{name:'Window controls'}).count(),0);
  assert.deepEqual(errors,[]);
  console.log('Titlebar controls, maximize state, responsive layout, navigation, and browser fallback passed.');
} finally {
  await browser.close();
  await new Promise(resolve => server.close(resolve));
}
