// Optional real-browser check; reuse an installed playwright-core via PLAYWRIGHT_MODULE.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { mkdtemp, readFile, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = await mkdtemp(path.join(tmpdir(), 'zoltraak-images-ui-'));
const bundle = await build({ stdin: { resolveDir:root, sourcefile:'fixture.tsx', loader:'tsx', contents:`
import React from 'react';
import { createRoot } from 'react-dom/client';
import { ImagesView } from './src/components/ImagesView.tsx';
const mode = new URL(location.href).searchParams.get('mode');
const count = mode === 'complete' ? 26 : 5;
const items = Array.from({length:count},(_,i)=>({id:'facebook:'+i,kind:'image',url:'https://cdn.example/'+i+'.jpg',
  thumbnail:'data:image/svg+xml,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="200" height="150"><rect width="200" height="150" fill="#aaa"/><text x="80" y="80" font-size="30">'+(i+1)+'</text></svg>'),
  width:200,height:150,filesize:1000,extension:'jpg',verified:true}));
items[1] = {...items[1],kind:'video',extension:'mp4',formats:[{id:'hd',container:'mp4',videoUrl:'https://cdn.example/video.mp4',hasAudio:true}]};
const album = {title:'Facebook gallery',sourceUrl:'https://www.facebook.com/posts/12345',uploader:null,items,
  ...(mode === 'legacy' ? {} : {completeness:mode,expectedCount:26,warnings:mode === 'partial' ? ['Facebook rate-limited extraction. Wait a while, then extract again.'] : []})};
window.calls=[];
window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
  window.calls.push({name,args});
  if(name === 'analyze_images') return structuredClone(album);
  if(name === 'add_download' || name === 'start_video_download') return 'job';
  throw new Error('Unexpected command: '+name);
}};
createRoot(document.getElementById('root')).render(React.createElement(ImagesView));
` }, bundle:true, write:false, format:'iife', platform:'browser', jsx:'automatic' });
const assets = path.join(root, 'dist', 'assets');
const cssName = (await readdir(assets)).find(name=>name.endsWith('.css'));
const server = createServer(async(req,res)=>{
  if(req.url === '/fixture.js') { res.setHeader('Content-Type','text/javascript'); res.end(bundle.outputFiles[0].text); }
  else if(req.url.startsWith('/assets/')) {
    const name=path.basename(req.url); res.setHeader('Content-Type',name.endsWith('.css')?'text/css':'font/woff2');
    try { res.end(await readFile(path.join(assets,name))); } catch { res.writeHead(404); res.end(); }
  } else { res.setHeader('Content-Type','text/html'); res.end('<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+cssName+'"><div id="root"></div><script src="/fixture.js"></script>'); }
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true});
try {
  for(const mode of ['complete','partial','legacy']) {
    const page=await browser.newPage({viewport:{width:1200,height:850}});
    const errors=[]; page.on('pageerror',error=>errors.push(error.message));
    await page.goto('http://127.0.0.1:'+server.address().port+'/?mode='+mode);
    await page.getByLabel('Post or image URL').fill('https://www.facebook.com/posts/12345');
    await page.getByRole('button',{name:'extract',exact:true}).click();
    const count=mode === 'complete'?26:5;
    const title=mode === 'complete'?'Found 26 media':mode === 'partial'?'Found 5 of 26':'Found 5 · completeness unverified';
    await page.getByText(title,{exact:true}).waitFor();
    if(mode === 'partial') assert.ok(await page.getByRole('status').isVisible());
    await page.getByRole('button',{name:'download all ('+count+')',exact:true}).click();
    await page.getByText('Sent '+count+' files to the queue',{exact:true}).waitFor();
    const calls=await page.evaluate(()=>window.calls);
    assert.equal(calls.filter(c=>c.name === 'add_download').length,count-1);
    assert.equal(calls.filter(c=>c.name === 'start_video_download').length,1);
    for(const width of [1200,640]) {
      await page.setViewportSize({width,height:850});
      await page.screenshot({path:path.join(output,mode+'-'+width+'.png'),fullPage:true});
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth <= innerWidth),'no horizontal overflow');
    }
    assert.deepEqual(errors,[]);
    await page.close();
  }
  console.log('PASS: complete/partial/legacy results, mixed-media downloads, 1200px and 640px layouts');
  console.log('Screenshots: '+output);
} finally { await browser.close(); await new Promise(resolve=>server.close(resolve)); }
