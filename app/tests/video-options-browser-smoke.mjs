// Uses an existing playwright-core via PLAYWRIGHT_MODULE and the built app CSS.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.resolve(root, '../scratch/release4-browser');
await mkdir(output, { recursive: true });
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'fixture.tsx', loader: 'tsx', contents: `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { VideoView } from './src/components/VideoView.tsx';
import { useVideo } from './src/lib/useVideo.ts';
const formats = [
 {id:'hls',container:'mp4',height:1080,hasAudio:true,videoCodec:'avc1.64001f',audioCodec:'mp4a.40.2',videoUrl:'https://cdn.test/live.m3u8',requiresMerge:false},
 {id:'progressive',container:'mp4',height:720,hasAudio:true,videoCodec:'avc1.64001f',audioCodec:'mp4a.40.2',videoUrl:'https://cdn.test/v.mp4',requiresMerge:false},
 {id:'vp9',container:'webm',height:1440,hasAudio:null,videoCodec:'vp9',audioCodec:'opus',videoUrl:'https://cdn.test/v.webm',requiresMerge:false}
];
window.calls=[];
window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
 if(name==='analyze_video') {
  window.calls.push({name,args});
  if(args.url.endsWith('/fail') && !args.useFallback)throw new Error('Built-in extraction fixture failed');
  if(args.url.endsWith('/slow'))await new Promise(resolve=>setTimeout(resolve,250));
  return {title:args.url.endsWith('/slow')?'Stale item':'Release 4 fixture',webpageUrl:args.url,formats,subtitles:[{language:'en',label:'English',extension:'vtt',url:'https://cdn.test/en.vtt',automatic:false}]};
 }
 if(name==='list_video_jobs')return [];
 if(name==='start_video_download'){window.calls.push({name,args});return 'job';}
 throw new Error('Unexpected command: '+name);
}};
function Fixture(){const video=useVideo();const [prefill,setPrefill]=React.useState(null);window.fixtureVideo=video;window.prefill=(url)=>setPrefill({url,headers:[],nonce:performance.now()});return React.createElement(VideoView,{video,prefill});}
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
  await page.getByRole('textbox',{name:'Video page or media URL'}).fill('https://fixture.test/fail');
  await page.getByRole('button',{name:'analyze',exact:true}).click();
  await page.getByText('Built-in extraction fixture failed',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Try local yt-dlp',exact:true}).click();
  await page.getByText('Release 4 fixture',{exact:true}).waitFor();
  assert.equal(await page.evaluate(()=>window.calls.find(c=>c.name==='analyze_video'&&c.args.useFallback)?.args.useFallback),true);
  await page.getByRole('radio',{name:'Up to 720p',exact:true}).check();
  await page.getByText('Selected: 720p · mp4',{exact:true}).waitFor();
  await page.getByRole('combobox',{name:'Subtitles',exact:true}).selectOption('0');
  await page.getByRole('button',{name:'Download preset',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(c=>c.name==='start_video_download'));
  let calls=await page.evaluate(()=>window.calls.filter(c=>c.name==='start_video_download'));
  assert.equal(calls[0].args.format.id,'progressive');
  assert.equal(calls[0].args.format.downloadOptions.subtitle.language,'en');
  assert.equal(calls[0].args.format.downloadOptions.liveSeconds,undefined);
  await page.getByRole('radio',{name:'H.264 / AAC',exact:true}).check();
  await page.getByRole('switch',{name:'Record live HLS',exact:true}).click();
  await page.getByRole('spinbutton',{name:'Recording seconds',exact:true}).fill('2');
  assert.equal(await page.getByRole('button',{name:'Record preset',exact:true}).isDisabled(),true);
  await page.getByRole('spinbutton',{name:'Recording seconds',exact:true}).fill('300');
  await page.getByRole('button',{name:'Record preset',exact:true}).click();
  await page.waitForFunction(()=>window.calls.filter(c=>c.name==='start_video_download').length===2);
  calls=await page.evaluate(()=>window.calls.filter(c=>c.name==='start_video_download'));
  assert.equal(calls[1].args.format.id,'hls');
  assert.equal(calls[1].args.format.downloadOptions.liveSeconds,300);
  for(const width of [1200,900]) {
    await page.setViewportSize({width,height:850});
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth),true);
    await page.screenshot({path:path.join(output,'video-options-'+width+'.png'),fullPage:true});
  }
  // A late result for another item cannot replace the latest analysis.
  await page.evaluate(async()=>{
    const old=window.fixtureVideo.analyze('https://fixture.test/slow');
    await window.fixtureVideo.analyze('https://fixture.test/current');
    await old;
  });
  assert.equal(await page.evaluate(()=>window.fixtureVideo.metadata.webpageUrl),'https://fixture.test/current');
  await page.evaluate(async()=>{
    window.prefill('https://x.com/user/status/123/slow');
    await new Promise(resolve=>setTimeout(resolve,30));
    window.prefill('https://fixture.test/latest-prefill');
    await new Promise(resolve=>setTimeout(resolve,450));
  });
  assert.equal(await page.evaluate(()=>window.fixtureVideo.metadata.webpageUrl),'https://fixture.test/latest-prefill');
  assert.deepEqual(errors, []);
  console.log('Release 4 UI checks passed. Artifacts: ' + output);
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
