import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';
const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'icon-fixture.tsx', loader: 'tsx', contents: `
import React from 'react'; import {createRoot} from 'react-dom/client';
import {AppIconSettings} from './src/components/AppIconSettings';
import {AppIconSync} from './src/lib/AppIconSync';
import {useAppearance} from './src/lib/useAppearance';
window.calls=[];window.warning=null;window.__TAURI_INTERNALS__={invoke:async(cmd,args)=>{window.calls.push({cmd,...args});return {shortcutsUpdated:1,warning:window.warning};}};
function Fixture(){const appearance=useAppearance();window.appearance=appearance;return <main className="min-h-dvh bg-panel p-6"><div className="mx-auto max-w-3xl"><AppIconSync/><AppIconSettings/></div></main>;}createRoot(document.getElementById('root')).render(<Fixture/>);
` }, bundle: true, write: false, format: 'iife', platform: 'browser', jsx: 'automatic', loader: { '.png': 'dataurl' } });
const assets = path.join(root, 'dist', 'assets'), css = (await readdir(assets)).find(n => n.endsWith('.css'));
const server = createServer(async (req, res) => {
  if (req.url === '/fixture.js') { res.setHeader('Content-Type', 'text/javascript'); res.end(bundle.outputFiles[0].text); }
  else if (req.url.startsWith('/assets/')) { res.setHeader('Content-Type', req.url.endsWith('.css') ? 'text/css' : 'font/woff2'); res.end(await readFile(path.join(assets, path.basename(req.url)))); }
  else { res.setHeader('Content-Type', 'text/html'); res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><link rel="stylesheet" href="/assets/' + css + '"><div id="root"></div><script src="/fixture.js"></script></html>'); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
const output = path.resolve(root, '../scratch/app-icon'); await mkdir(output, { recursive: true });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } }), errors = [];
  page.on('pageerror', e => errors.push(e.message)); await page.emulateMedia({ colorScheme: 'dark' });
  await page.goto('http://127.0.0.1:' + server.address().port);
  const synced = foreground => page.waitForFunction(foreground => window.calls.at(-1)?.foreground === foreground && !('background' in window.calls.at(-1)), foreground);
  await synced('#56a2f0');
  const fg = page.getByLabel('Icon foreground', { exact: true });
  assert.equal(await page.getByLabel('Icon background', { exact: true }).count(), 0);
  assert.equal(await fg.isDisabled(), true);
  const lettering = () => page.getByRole('switch', { name: 'Use Route Highlight color for icon' });
  await lettering().click();
  // A burst of picker events must paint locally without publishing global
  // appearance settings or starting native icon generation for each color.
  const before = await page.evaluate(() => ({ calls: window.calls.length, saved: window.appearance.settings.iconPalettes.dark?.foreground }));
  await fg.evaluate(input => {
    for (const color of ['#112233', '#334455', '#667788', '#ffeedd']) {
      input.value = color;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }
  });
  await page.getByText('#FFEEDD', { exact: true }).waitFor();
  assert.deepEqual(await page.evaluate(() => ({ calls: window.calls.length, saved: window.appearance.settings.iconPalettes.dark?.foreground })), before, 'scrubbing does not publish global settings');
  await synced('#ffeedd');
  assert.equal(await page.evaluate(() => window.calls.length), before.calls + 1, 'one native update after the picker settles');
  const preview = page.getByRole('img', { name: 'Icon preview 96 pixels' });
  assert.deepEqual(await preview.evaluate(e => { const s = getComputedStyle(e); return { background: s.backgroundColor, padding: s.padding }; }), { background: 'rgba(0, 0, 0, 0)', padding: '0px' });
  await page.screenshot({ path: path.join(output, 'desktop-dark.png') });
  await page.evaluate(() => window.appearance.setThemeMode('light')); await synced('#1560c4');
  assert.equal(await fg.isDisabled(), true);
  await page.evaluate(() => window.appearance.setThemeMode('dark')); await synced('#ffeedd');
  assert.equal(await fg.inputValue(), '#ffeedd');
  await lettering().click(); await synced('#56a2f0');
  await lettering().click(); await synced('#ffeedd');
  await page.evaluate(() => window.appearance.setThemeMode('system')); await page.emulateMedia({ colorScheme: 'light' }); await synced('#1560c4');
  await page.emulateMedia({ colorScheme: 'dark' }); await synced('#ffeedd');
  await page.evaluate(() => window.appearance.selectPreset('amber-crt')); await synced('#f59e0b');
  await lettering().click(); await fg.fill('#ffddaa'); await synced('#ffddaa');
  await page.evaluate(() => window.appearance.selectPreset('emerald-radar'));
  await page.waitForFunction(() => window.calls.at(-1)?.foreground !== '#ffddaa');
  await page.evaluate(() => window.appearance.selectPreset('amber-crt')); await synced('#ffddaa');
  await page.evaluate(() => { window.presetId = window.appearance.saveUserPreset('Icon fixture'); });
  assert.equal(await fg.inputValue(), '#ffddaa');
  const exported = await page.evaluate(() => window.appearance.exportThemeJson());
  assert.deepEqual(JSON.parse(exported).iconPalette, { useAppColors: false, foreground: '#ffddaa' });
  await page.getByRole('button', { name: 'Reset icon colors' }).click();
  assert.equal(await lettering().getAttribute('aria-checked'), 'true');
  assert.equal(await page.evaluate(json => window.appearance.importThemeJson(json).ok, exported), true); await synced('#ffddaa');
  assert.equal(await page.evaluate(() => window.appearance.importThemeJson(JSON.stringify({ colors: {}, iconPalette: { background: 'bad' } })).ok), false);
  await page.reload(); await synced('#ffddaa');
  await page.getByRole('switch', { name: 'Sync app icon with theme' }).click(); await synced('#56a2f0');
  await page.getByText('Sync is off.', { exact: false }).waitFor();
  await page.getByRole('switch', { name: 'Sync app icon with theme' }).click(); await synced('#ffddaa');
  await page.waitForFunction(() => document.querySelector('button') && ![...document.querySelectorAll('button')].find(e => e.textContent.includes('Refresh desktop icon'))?.disabled);
  await page.getByRole('button', { name: 'Refresh desktop icon' }).click(); await page.waitForFunction(() => window.calls.at(-1)?.force === true);
  await page.evaluate(() => { window.warning = 'App icons updated; one shortcut could not be written.'; });
  await page.getByRole('button', { name: 'Refresh desktop icon' }).click(); await page.getByText('App icons updated; one shortcut could not be written.').waitFor();
  await fg.fill('#0c0b08'); await page.getByText('This color has low contrast', { exact: false }).waitFor();
  await fg.fill('#ffddaa'); await synced('#ffddaa');
  await page.setViewportSize({ width: 390, height: 844 }); await page.screenshot({ path: path.join(output, 'mobile.png') });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await page.evaluate(() => window.appearance.setThemeMode('light')); await synced('#1560c4');
  assert.equal(await page.evaluate(() => JSON.parse(window.appearance.exportThemeJson()).colors.panel), '#d7dcd2', 'export uses the active light palette');
  await page.setViewportSize({ width: 1280, height: 800 }); await page.screenshot({ path: path.join(output, 'desktop-light.png') });
  await page.evaluate(() => window.appearance.update(prev => ({ ...prev, effects: { ...prev.effects, glass: true, glassThemeOverride: true } })));
  await synced('#0ea5e9');
  await lettering().click();
  await fg.fill('#c0ffee'); await synced('#c0ffee');
  const glassExport = await page.evaluate(() => window.appearance.exportThemeJson());
  await page.getByRole('button', { name: 'Reset icon colors' }).click(); await synced('#0ea5e9');
  await page.evaluate(json => window.appearance.importThemeJson(json), glassExport); await synced('#c0ffee');
  assert.deepEqual(errors, []);
  console.log('App icon browser checks passed: per-theme/preset colors, system switching, saved presets, export/import, persistence, default restoration, manual refresh, warnings, and responsive previews.');
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
