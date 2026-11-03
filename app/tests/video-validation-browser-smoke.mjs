// Uses an existing playwright-core via PLAYWRIGHT_MODULE and the built app CSS.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.resolve(root, '../scratch/release2-browser');
await mkdir(output, { recursive: true });
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'fixture.tsx', loader: 'tsx', contents: `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { VideoView } from './src/components/VideoView.tsx';
import { useVideo } from './src/lib/useVideo.ts';
const formats = [
  {id:'unknown',container:'mp4',height:1080,hasAudio:null},
  {id:'inferred',container:'mp4',height:720,hasAudio:true,validation:{status:'inferred'}},
  {id:'verified',container:'mp4',height:480,hasAudio:true,validation:{status:'verified'}}
].map(f=>({...f,videoUrl:'https://cdn.test/'+f.id+'.mp4',requiresMerge:false}));
window.__TAURI_INTERNALS__={invoke:async(name)=>{
  if(name==='analyze_video')return {title:'Validation fixture',webpageUrl:'https://fixture.test/video',formats};
  if(name==='list_video_jobs')return [
    {id:'checked',title:'Checked clip',state:'finished',downloadedBytes:1000,path:'C:/Downloads/checked.mp4',validation:{status:'verified',hasAudio:true,samplesDecoded:true}},
    {id:'unchecked',title:'Unchecked clip',state:'finished',downloadedBytes:1000,path:'C:/Downloads/unchecked.mp4',validation:{status:'unknown'}},
    {id:'failed',title:'Failed clip',state:'failed',downloadedBytes:1000,path:'C:/Downloads/failed.mp4',error:'Invalid media retained for inspection'}
  ];
  throw new Error('Unexpected command: '+name);
}};
function Fixture() { return React.createElement(VideoView, {video:useVideo()}); }
createRoot(document.getElementById('root')).render(React.createElement(Fixture));
` }, bundle: true, write: false, format: 'iife', platform: 'browser', jsx: 'automatic' });
const assets = path.join(root, 'dist', 'assets');
const css = (await readdir(assets)).find((name) => name.endsWith('.css'));
const server = createServer(async (req, res) => {
  if (req.url === '/fixture.js') {
    res.setHeader('Content-Type', 'text/javascript'); res.end(bundle.outputFiles[0].text);
  } else if (req.url.startsWith('/assets/')) {
    res.setHeader('Content-Type', req.url.endsWith('.css') ? 'text/css' : 'font/woff2');
    try { res.end(await readFile(path.join(assets, path.basename(req.url)))); }
    catch { res.writeHead(404); res.end(); }
  } else {
    res.setHeader('Content-Type', 'text/html');
    res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/' + css + '"><div id="root"></div><script src="/fixture.js"></script></html>');
  }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 850 } });
  const errors = [];
  page.on('pageerror', (error) => { errors.push(error.message); console.error(error.message); });
  await page.goto('http://127.0.0.1:' + server.address().port);
  await page.locator('input').first().fill('https://fixture.test/video');
  await page.getByRole('button', { name: 'analyze', exact: true }).click();
  await page.getByText('audio unverified', { exact: true }).waitFor();
  await page.getByText('audio inferred', { exact: true }).waitFor();
  await page.getByRole('tab', { name: 'Finished 2', exact: true }).click();
  await page.getByText(/tracks and samples checked/).waitFor();
  await page.getByText(/verification unavailable/).waitFor();
  await page.getByRole('tab', { name: 'Failed 1', exact: true }).click();
  await page.getByText('Failed clip', {exact:true}).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Send Failed clip to Discord', exact: true }).count(), 0);
  assert.equal(await page.getByRole('button', { name: 'Show in folder', exact: true }).count(), 1);
  await page.getByRole('tab', { name: 'Finished 2', exact: true }).click();
  await page.screenshot({ path: path.join(output, 'video-validation.png'), fullPage: true });
  assert.deepEqual(errors, []);
  console.log('Video validation UI checks passed. Artifacts: ' + output);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
