// Uses the same existing Playwright runtime as the other browser smoke checks.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.resolve(root, '../scratch/download-row');
await mkdir(output, { recursive: true });
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'fixture.tsx', loader: 'tsx', contents: `
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { DownloadRow } from './src/components/DownloadRow';
import { useAppearance } from './src/lib/useAppearance';
import { AppearanceView } from './src/components/AppearanceView';
import { videoJobToSnapshot, musicJobToSnapshot } from './src/lib/snapshots';
const base = {id:'video',title:'Coastal walk',webpageUrl:'https://media.test/clip',state:'paused',downloadedBytes:2400,totalBytes:5000,bytesPerSecond:0,etaSeconds:null,error:null,path:null,createdAt:'2026-10-01T00:00:00Z',thumbnail:location.origin+'/poster.png'};
const video=videoJobToSnapshot(base);
const music=musicJobToSnapshot({...base,id:'music',directory:'Downloads',format:'mp3',track:{title:'Night drive',artists:['Sample artist'],sourceUrl:'https://media.test/audio',coverUrl:location.origin+'/poster.png'}});
const picture={...video,id:'image',isVideo:false,filename:'photo.png',thumbnail:null,url:location.origin+'/poster.png'};
const plain={...video,id:'plain',isVideo:false,filename:'archive.zip',thumbnail:null};
const local={...video,id:'local',isVideo:false,state:'finished',filename:'ordinary.mp4',thumbnail:null,path:'C:/Downloads/ordinary.mp4'};
const legacy={...local,id:'legacy',isVideo:true,filename:'Old video job',path:'C:/Downloads/old.mkv'};
const missing={...local,id:'missing',filename:'lost.mp4',path:'C:/Downloads/lost.mp4'};
window.calls={};
window.feedbacks=[];
const nativeAnimate=Element.prototype.animate;
Element.prototype.animate=function(frames,options){if(options?.id==='file-open-feedback')window.feedbacks.push(options.duration);return nativeAnimate.call(this,frames,options);};
window.previewRequests=[];
window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
  if(name==='download_thumbnail') {window.previewRequests.push(args.id);return 'data:image/svg+xml;base64,'+btoa('<svg xmlns="http://www.w3.org/2000/svg" width="180" height="320"><rect width="180" height="320" fill="#688781"/></svg>');}
  if(name==='download_file_status') {
    if(args.id==='missing') return {path:'C:/Downloads/lost.mp4',status:'missing',detail:'The file was moved or deleted outside the app.'};
    return {path:'C:/Downloads/ordinary.mp4',status:'present',detail:null};
  }
  if(name==='download_file_statuses') {
    const map={};
    for(const id of args.ids) {
      if(id==='missing') map[id]={path:'C:/Downloads/lost.mp4',status:'missing',detail:'The file was moved or deleted outside the app.'};
      else map[id]={path:'C:/Downloads/ordinary.mp4',status:'present',detail:null};
    }
    return map;
  }
  throw new Error('Unexpected command: '+name);
}};
function Fixture() {
  const {update}=useAppearance();window.setAppearance=update;
  const [appearance,setAppearance]=useState(false);window.showAppearance=()=>setAppearance(true);
  const [expanded,setExpanded]=useState(null);
  const [selected,setSelected]=useState(new Set());
  const [items,setItems]=useState([video,music,picture,plain,local,legacy,missing]);
  window.changePoster=source=>setItems(prev=>prev.map(item=>item.id==='video'?{...item,thumbnail:source}:item));
  const record=name=>window.calls[name]=(window.calls[name]||0)+1;
  if(appearance)return <AppearanceView/>;
  return <div>{items.map(item=><DownloadRow key={item.id} item={item} expanded={expanded===item.id} selected={selected.has(item.id)} dimmed={false}
    onExpand={id=>setExpanded(prev=>prev===id?null:id)} onSelect={id=>{record('select');setSelected(prev=>{const next=new Set(prev);next.has(id)?next.delete(id):next.add(id);return next;});}}
    onToggle={()=>record('toggle')} onRemove={()=>record('remove')} onReveal={()=>record('reveal')} onOpen={()=>record('open')}/>)}</div>;
}
createRoot(document.getElementById('root')).render(<Fixture/>);
` }, bundle: true, write: false, format: 'iife', platform: 'browser', jsx: 'automatic', loader: { '.png': 'dataurl' } });
const assets = path.join(root, 'dist', 'assets');
const css = (await readdir(assets)).find(name => name.endsWith('.css'));
// A deterministic source poster keeps this fixture independent of external hosts.
const poster = '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360" viewBox="0 0 640 360"><rect width="640" height="360" fill="#bed5d8"/><circle cx="470" cy="83" r="32" fill="#f8e7ba"/><path d="M0 180L140 124L272 210L422 155L640 201V360H0Z" fill="#688781"/><path d="M0 234L143 198L250 235L350 218L460 242L640 211V360H0Z" fill="#334f46"/><path d="M640 217Q320 234 385 310L640 335Z" fill="#71a2ac"/><path d="M110 360Q168 274 270 259Q198 283 173 360" fill="#dfccac"/></svg>';
const server = createServer(async (req, res) => {
  if (req.url === '/fixture.js') { res.setHeader('Content-Type', 'text/javascript'); res.end(bundle.outputFiles[0].text); }
  else if (req.url === '/poster.png') { res.setHeader('Content-Type', 'image/svg+xml'); res.end(poster); }
  else if (req.url === '/portrait.png') { res.setHeader('Content-Type', 'image/svg+xml'); res.end(poster.replace('width="640" height="360" viewBox="0 0 640 360"','width="180" height="320" viewBox="0 0 180 320"')); }
  else if (req.url === '/broken.png') { res.writeHead(404); res.end(); }
  else if (req.url.startsWith('/assets/')) {
    try { res.setHeader('Content-Type', req.url.endsWith('.css') ? 'text/css' : 'font/woff2'); res.end(await readFile(path.join(assets, path.basename(req.url)))); }
    catch { res.writeHead(404); res.end(); }
  } else { res.setHeader('Content-Type', 'text/html'); res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>'); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ headless: true, ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}) });
try {
  const page = await browser.newPage({ viewport: { width: 1366, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://127.0.0.1:'+server.address().port);
  const row = page.locator('[data-row-id="video"]');
  const cell = row.locator('.download-row').getByText('PAUSED', {exact:true});
  const isOpen = async expected => assert.equal(await row.getAttribute('data-open'),String(expected));
  const finished = page.locator('[data-row-id="local"]');
  const finishedName = finished.locator('[data-download-name]');
  const finishedCell = finished.locator('.download-row').getByText('FINISHED',{exact:true});
  const missingRow = page.locator('[data-row-id="missing"]');
  assert.equal(await missingRow.locator('[data-missing-tag]').textContent(), 'missing', 'missing files render a missing tag');
  assert.equal(await missingRow.locator('.download-row').getByText('MISSING', { exact: true }).count(), 1, 'missing files render MISSING status');
  assert.equal(await finished.locator('[data-missing-tag]').count(), 0, 'present files do not render missing tag');
  await finishedName.click({button:'middle'});
  assert.deepEqual(await page.evaluate(()=>window.feedbacks),[200],'middle-click immediately acknowledges opening');
  await finished.locator('.download-row').evaluate(el=>{const a=el.getAnimations().find(a=>a.id==='file-open-feedback');if(a){a.pause();a.currentTime=40;}});
  await page.screenshot({path:path.join(output,'file-open-feedback.png')});
  assert.deepEqual(await page.evaluate(()=>window.calls),{open:1});
  assert.equal(await finished.getAttribute('data-open'),'false','middle-click does not expand the row');
  await finishedName.dblclick();
  assert.deepEqual(await page.evaluate(()=>window.calls),{open:1},'description double-click does not open files');
  assert.equal(await page.evaluate(()=>window.feedbacks.length),1,'description double-click does not flash');
  await finishedCell.dblclick();
  assert.deepEqual(await page.evaluate(()=>window.feedbacks),[200,200],'double-click replays the same feedback');
  await finished.locator('.download-row').evaluate(el=>el.getAnimations().forEach(a=>a.cancel()));
  await page.evaluate(()=>window.setAppearance({fileOpenFeedback:false}));
  await finishedName.click({button:'middle'});
  assert.equal(await page.evaluate(()=>window.feedbacks.length),2,'appearance toggle disables the flash');
  await page.evaluate(()=>window.setAppearance({fileOpenFeedback:true,fileOpenFeedbackDuration:400}));
  await finishedName.click({button:'middle'});
  assert.equal(await page.evaluate(()=>window.feedbacks.at(-1)),400,'appearance duration controls the flash');
  await page.emulateMedia({reducedMotion:'reduce'});
  await finishedName.click({button:'middle'});
  assert.equal(await page.evaluate(()=>window.feedbacks.at(-1)),100,'system reduced motion shortens color feedback');
  await page.emulateMedia({reducedMotion:'no-preference'});
  await page.evaluate(()=>{window.setAppearance({fileOpenFeedbackDuration:200});window.calls.open=2;});
  assert.deepEqual(await page.evaluate(()=>window.calls),{open:2},'double-click still opens finished files');
  await finishedName.click({button:'right'});
  await finished.getByRole('checkbox').click({button:'middle'});
  await finished.getByRole('button',{name:/^Delete /}).click({button:'middle'});
  assert.deepEqual(await page.evaluate(()=>window.calls),{open:2},'right-click and middle-click on controls do not open files');
  await page.evaluate(()=>window.setAppearance({downloadDoubleClickInterval:100}));
  await finishedCell.click();await page.waitForTimeout(180);await finishedCell.click();
  assert.equal(await page.evaluate(()=>window.calls.open),2,'clicks outside a shorter configured interval do not open');
  await page.evaluate(()=>window.setAppearance({downloadDoubleClickInterval:600}));
  await finishedCell.click();await page.waitForTimeout(450);await finishedCell.click();
  assert.equal(await page.evaluate(()=>window.calls.open),3,'custom timing accepts clicks inside the configured interval');
  await finishedCell.dblclick();
  assert.equal(await page.evaluate(()=>window.calls.open),4,'one double-click opens exactly once');
  await page.evaluate(()=>{window.setAppearance({downloadDoubleClickInterval:250});window.calls.open=2;});
  await row.locator('[data-download-name]').click({button:'middle'});
  await row.locator('.download-row').getByText('PAUSED',{exact:true}).dblclick();
  assert.deepEqual(await page.evaluate(()=>window.calls),{open:2,reveal:2},'unfinished entries retain reveal behavior');
  await page.evaluate(()=>window.calls={});
  if(await row.getAttribute('data-open')==='true')await row.getByRole('button',{name:'Close Coastal walk',exact:true}).click();
  await cell.click(); await isOpen(true);
  await row.locator('img').waitFor();
  await page.waitForFunction(()=>document.querySelector('[data-row-id="video"] img')?.naturalWidth > 0);
  await page.waitForTimeout(300);
  await cell.click(); await isOpen(false);
  await row.locator('[data-download-name]').click(); await isOpen(false);
  await row.getByRole('checkbox').click(); await isOpen(false);
  await row.getByRole('button', {name:'Resume Coastal walk',exact:true}).click(); await isOpen(false);
  await row.getByRole('button', {name:'Delete Coastal walk and local file',exact:true}).click(); await isOpen(false);
  assert.deepEqual(await page.evaluate(()=>window.calls),{select:1,toggle:1,remove:1});
  await row.getByRole('button', {name:'Open Coastal walk',exact:true}).focus();
  await page.keyboard.press('Enter'); await isOpen(true);
  await page.waitForTimeout(700); // Capture the settled accordion, not its opening frame.
  const layout = () => row.locator('[data-download-details]').evaluate(el => {
    const image=el.querySelector('figure'),fields=el.querySelector(':scope > div');
    const a=image?.getBoundingClientRect(),b=fields.getBoundingClientRect();
    return {display:getComputedStyle(el).display,columns:getComputedStyle(el).gridTemplateColumns,sideBySide:!!a&&a.right<=b.left&&Math.abs(a.top-b.top)<1,stacked:!!a&&a.bottom<=b.top};
  });
  assert.equal((await layout()).sideBySide,true,'desktop preview occupies the left column');
  await page.screenshot({path:path.join(output,'details-desktop.png')});
  await page.setViewportSize({width:768,height:900});
  assert.equal((await layout()).sideBySide,true,'intermediate layout retains two columns');
  await page.screenshot({path:path.join(output,'details-768.png')});
  await page.setViewportSize({width:390,height:844});
  assert.equal((await layout()).stacked,true,'narrow details stack the preview above fields');
  await page.screenshot({path:path.join(output,'details-390.png')});
  assert.equal(await row.locator('img').evaluate(img=>img.getBoundingClientRect().right<=innerWidth),true);
  await page.setViewportSize({width:1366,height:900});
  await page.evaluate(()=>window.changePoster(location.origin+'/portrait.png'));
  await page.waitForFunction(()=>document.querySelector('[data-row-id="video"] img')?.naturalHeight===320);
  const portrait = await row.locator('img').evaluate(img=>({width:img.getBoundingClientRect().width,height:img.getBoundingClientRect().height,naturalRatio:img.naturalWidth/img.naturalHeight,column:img.closest('figure').getBoundingClientRect().width}));
  assert.ok(Math.abs(portrait.width/portrait.height-portrait.naturalRatio)<0.001,'portrait keeps its natural ratio');
  assert.equal(portrait.width,portrait.column,'portrait fills the preview column');
  assert.ok(portrait.height>portrait.width);
  await page.evaluate(()=>document.documentElement.dataset.theme='dark');
  await page.screenshot({path:path.join(output,'portrait-desktop.png')});
  await page.setViewportSize({width:390,height:844});
  assert.equal((await layout()).stacked,true);
  await page.screenshot({path:path.join(output,'portrait-390.png')});
  await page.setViewportSize({width:1366,height:900});
  await page.evaluate(()=>window.changePoster(location.origin+'/broken.png'));
  await page.waitForFunction(()=>!document.querySelector('[data-row-id="video"] img'));
  assert.equal((await layout()).display,'block','failed preview restores the full-width details');
  await page.evaluate(()=>window.changePoster(location.origin+'/poster.png'));
  await page.waitForFunction(()=>document.querySelector('[data-row-id="video"] img')?.naturalWidth > 0);
  for (const id of ['music','image','plain']) {
    const target=page.locator('[data-row-id="'+id+'"]');
    await target.locator('.download-row').getByText('PAUSED',{exact:true}).click();
    assert.equal(await target.locator('img').count(),id==='plain'?0:1);
    if(id==='music')assert.match(await target.locator('img').getAttribute('alt'),/^Cover art/);
    if(id==='plain')assert.equal(await target.locator('[data-download-details]').evaluate(el=>getComputedStyle(el).display),'block');
  }
  await page.evaluate(()=>window.previewRequests=[]);
  for(const id of ['local','legacy']){
    const target=page.locator('[data-row-id="'+id+'"]');
    await target.getByRole('button',{name:/^Open /}).click();
    await page.waitForFunction(id=>document.querySelector('[data-row-id="'+id+'"] img')?.naturalHeight===320,id);
    assert.match(await target.locator('img').getAttribute('src'),/^data:image/);
  }
  assert.deepEqual(await page.evaluate(()=>window.previewRequests),['local','legacy'],'prefer source posters and only generate missing local previews');
  await page.evaluate(()=>window.showAppearance());
  const grain=page.getByRole('slider',{name:'Analogue Console Noise Grain size',exact:true});
  await page.evaluate(()=>{const overlay=document.createElement('div');overlay.id='atmosphere-noise-overlay';overlay.setAttribute('aria-hidden','true');document.body.append(overlay);});
  const noiseToggle=page.getByRole('switch',{name:'Analogue Console Noise',exact:true});
  assert.equal(await grain.isDisabled(),true,'grain size is disabled when noise is off');
  await noiseToggle.click();await grain.fill('2.5');
  assert.equal(await page.evaluate(()=>getComputedStyle(document.documentElement).getPropertyValue('--effect-noise-size').trim()),'320px','grain size scales the texture live');
  assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.appearance.v1')).effects.noiseGrainSize),2.5,'grain size persists');
  assert.equal(await page.locator('#atmosphere-noise-overlay').evaluate(el=>getComputedStyle(el).backgroundSize),'320px 320px','rendered overlay uses the grain scale');
  await page.getByRole('button',{name:/Underlay$/}).click();
  assert.equal(await page.locator('#atmosphere-noise-overlay').evaluate(el=>getComputedStyle(el).backgroundSize),'320px 320px','underlay uses the same grain scale');
  await grain.scrollIntoViewIfNeeded();await page.screenshot({path:path.join(output,'grain-size.png')});
  await noiseToggle.click();
  const feedbackToggle=page.getByRole('switch',{name:'File opening feedback',exact:true});
  const duration=page.getByRole('slider',{name:'File opening feedback duration',exact:true});
  const doubleClickInterval=page.getByRole('slider',{name:'Download double-click interval',exact:true});
  await doubleClickInterval.fill('150');
  assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.appearance.v1')).downloadDoubleClickInterval),150,'double-click interval saves from Appearance');
  await duration.fill('350');
  assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.appearance.v1')).fileOpenFeedbackDuration),350);
  await page.getByRole('button',{name:'Preview',exact:true}).click();
  assert.equal(await page.evaluate(()=>window.feedbacks.at(-1)),350,'appearance preview uses the configured duration');
  await feedbackToggle.click();assert.equal(await duration.isDisabled(),true);
  assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.appearance.v1')).fileOpenFeedback),false);
  await feedbackToggle.click();
  await duration.scrollIntoViewIfNeeded();await page.screenshot({path:path.join(output,'file-open-settings.png')});
  await page.setViewportSize({width:390,height:844});await duration.scrollIntoViewIfNeeded();
  assert.equal(await duration.evaluate(el=>el.getBoundingClientRect().right<=innerWidth),true,'feedback controls fit narrow screens');
  await page.screenshot({path:path.join(output,'file-open-settings-390.png')});
  assert.deepEqual(errors,[]);
  console.log('Passed: middle/double-click opening, unfinished entry reveal, isolated actions, row toggling, keyboard disclosure, previews, broken-image recovery, and narrow layouts.');
} finally { await browser.close(); await new Promise(resolve=>server.close(resolve)); }
