// Test Downloads -> Convert with the real components and staging hook, using IPC fixtures.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bundle = await build({ stdin: { resolveDir: root, loader: 'tsx', contents: `
import React,{useState} from 'react';import{createRoot}from'react-dom/client';
import{DownloadsView}from'./src/components/DownloadsView';
import{ConvertView}from'./src/components/ConvertView';
import{useConvert}from'./src/lib/useConvert';
const items=[['video','clip.mp4','finished'],['audio','song.flac','finished'],['image','photo.jpg','finished'],['document','notes.txt','finished'],['unfinished','pending.mp4','downloading']].map(([id,filename,state])=>({id,filename,state,directory:'C:/Downloads',url:'https://media.test/'+filename,downloadedBytes:100,totalBytes:100,connections:1,bytesPerSecond:0,etaSeconds:null,priority:0,error:null}));
window.calls=[];window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
window.calls.push({name,args});
if(name==='media_capabilities')return {formats:['mp4','png','jpg','mp3','flac'],ffprobe:true};
if(name==='list_remux_jobs')return [];
if(name==='choose_directory')return 'D:/Selected work';
if(name==='download_file_status')return {path:'C:/Downloads/'+items.find(item=>item.id===args.id).filename,status:'present',detail:null};
if(name==='download_thumbnail')return null;
if(name==='scan_media')return args.paths.map(path=>({path,extension:path.split('.').pop(),videoCodec:path.endsWith('.flac')?null:'h264',audioCodec:path.endsWith('.jpg')?null:'aac',width:160,height:120,durationSeconds:2,bytes:100,unprobed:false}));
if(name==='plan_conversion')return args.paths.map(path=>({path,error:null,plan:{streams:{video:'encode',audio:'absent'},description:'Ready for conversion',warnings:[],durationSeconds:2,outputExtension:args.settings.format}}));
if(name.startsWith('plugin:'))return null;
if(name==='remux_queue_state')return {paused:false,parallelism:2,queued:[]};
if(name==='list_watch_rules')return {rules:[],statuses:[]};
if(name==='conversion_thumbnail')return 'data:image/svg+xml;utf8,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="160" height="120"><rect width="160" height="120" fill="#456"/></svg>');
if(name==='conversion_waveform')return Array.from({length:args.buckets},(_, i)=>(i%7)/7);
if(['set_remux_paused','set_remux_parallelism','move_remux_job','save_watch_rules'].includes(name))return null;
throw Error('Unexpected command '+name);
}};
function Fixture(){const[section,setSection]=useState('Downloads');const[working,setWorking]=useState('C:/Downloads');const convert=useConvert(section==='Convert',working);const[composing,setComposing]=useState(false);
const actions={toggle:()=>{},pause:async()=>{},resume:async()=>{},remove:()=>{},reveal:()=>{},convert:items=>{setSection('Convert');void convert.addPaths(items.map(item=>item.path),true);}};
return <><nav aria-label="Test navigation"><button onClick={()=>setSection('Downloads')}>Downloads</button><button onClick={()=>setSection('Convert')}>Convert</button></nav>
<div hidden={section!=='Downloads'}><DownloadsView active={section==='Downloads'} items={items} videoJobs={[]} pendingRemoval={new Set()} status="live" error={null} onClearError={()=>{}} onAdd={async()=>true} actions={actions} destination="C:/Downloads" onWorkingDirectoryChange={setWorking} composing={composing} onComposingChange={setComposing}/></div>
<div hidden={section!=='Convert'}><ConvertView convert={convert}/></div></>;}
createRoot(document.getElementById('root')).render(<Fixture/>);
` }, bundle: true, write: false, format: 'iife', platform: 'browser', jsx: 'automatic' });
const assets = path.join(root, 'dist', 'assets');
const css = (await readdir(assets)).find(name => name.endsWith('.css'));
const server = createServer(async (request, response) => {
  if (request.url === '/fixture.js') { response.setHeader('Content-Type', 'text/javascript'); response.end(bundle.outputFiles[0].text); }
  else if (request.url.startsWith('/assets/')) { response.setHeader('Content-Type', request.url.endsWith('.css') ? 'text/css' : 'font/woff2'); response.end(await readFile(path.join(assets, path.basename(request.url)))); }
  else { response.setHeader('Content-Type', 'text/html'); response.end('<html><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/' + css + '"><div id="root"></div><script src="/fixture.js"></script></html>'); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ headless: true, channel: process.env.CHROME_CHANNEL || 'chrome' });
try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:' + server.address().port);
  const downloads = page.getByRole('navigation', { name: 'Test navigation' }).getByRole('button', { name: 'Downloads', exact: true });
  await page.getByRole('button', { name: 'Destination C:/Downloads. Click to change.', exact: true }).click();
  for (const id of ['document', 'unfinished']) {
    await page.locator('[data-row-id="' + id + '"] [data-download-name]').click({ button: 'right' });
    assert.equal(await page.getByRole('menuitem', { name: 'Convert…', exact: true }).count(), 0);
    await page.keyboard.press('Escape');
  }
  for (const [id, filename] of [['video', 'clip.mp4'], ['audio', 'song.flac'], ['image', 'photo.jpg'], ['video', 'clip.mp4']]) {
    await downloads.click();
    await page.locator('[data-row-id="' + id + '"] [data-download-name]').click({ button: 'right' });
    await page.getByRole('menuitem', { name: 'Convert…', exact: true }).click();
    const selected = page.locator('[data-conversion-selected="true"]');
    await selected.waitFor();
    await page.waitForFunction(filename => document.activeElement?.getAttribute('aria-label') === 'Conversion input ' + filename, filename);
    assert.equal(await selected.getAttribute('data-conversion-path'), 'C:/Downloads/' + filename);
    assert.equal(await page.locator('[data-conversion-source-path]').getAttribute('data-conversion-source-path'), 'C:/Downloads/' + filename);
    assert.equal(await page.locator('#section-files').count(), 0, 'download handoff shows the source header instead of file intake');
    assert.equal(await page.getByLabel('Destination folder', { exact: true }).getAttribute('placeholder'), 'D:/Selected work/Converted', 'conversion follows the folder selected in the Downloads footer');
  }
  assert.equal(await page.locator('[data-conversion-path]').count(), 3, 'existing inputs are selected without duplication');
  assert.equal(await page.evaluate(() => window.calls.some(call => call.name === 'start_conversions' || call.name === 'remux_download')), false, 'opening Convert never starts a job');
  await downloads.click();
  await page.getByTitle('Back to C:/Downloads', { exact: true }).click();
  await page.getByRole('navigation', { name: 'Test navigation' }).getByRole('button', { name: 'Convert', exact: true }).click();
  assert.equal(await page.getByLabel('Destination folder', { exact: true }).getAttribute('placeholder'), 'C:/Downloads/Converted');
  await downloads.click();
  for (const id of ['video', 'audio', 'document', 'unfinished']) await page.locator('[data-row-id="' + id + '"]').getByRole('checkbox').click();
  await page.locator('[data-row-id="video"] [data-download-name]').click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Convert… (2)', exact: true }).click();
  await page.locator('[data-conversion-selected="true"]').waitFor();
  const lastScan = await page.evaluate(() => window.calls.filter(call => call.name === 'scan_media').at(-1).args.paths);
  assert.deepEqual([...lastScan].sort(), ['C:/Downloads/clip.mp4', 'C:/Downloads/song.flac']);
  assert.deepEqual(errors, []);
  console.log('Passed: supported completed file menu, navigation, input focus, duplicate staging, bulk filtering, and no automatic conversion.');
} finally { await browser.close(); server.close(); }
