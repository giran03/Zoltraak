// Exercise the real App and hooks. Only the native engine boundary is mocked.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';
const {chromium}=await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const output=path.resolve(root,'../scratch/download-ux');
await mkdir(output,{recursive:true});
const bundle=await build({stdin:{resolveDir:root,sourcefile:'fixture.tsx',loader:'tsx',contents:`
import React from 'react';
import {createRoot} from 'react-dom/client';
import App from './src/App';
import {DEFAULT_ENGINE_SETTINGS} from './src/lib/engine';
const clone=value=>JSON.parse(JSON.stringify(value));
const row=(id,filename,state,error=null)=>({id,filename,state,error,url:'https://files.test/'+filename,directory:'C:/Fixture/Downloads',createdAt:'2026-09-30T00:00:00Z',downloadedBytes:state==='finished'?1200:0,totalBytes:1200,bytesPerSecond:0,connections:1,etaSeconds:null,priority:0});
window.fixtureRows=[row('old','clip2.zip','finished'),row('folder','locked.zip','failed','i/o error on C:/Fixture/Downloads: Access is denied (os error 5)'),row('signin','login.mp4','failed','server returned 403'),row('network','network.zip','failed','Connection timed out')];
const track={id:'track',title:'Music fixture',artists:['Fixture artist'],service:'soundcloud',sourceUrl:'https://music.test/song',duration:120,source:{kind:'direct',url:'https://music.test/song.mp3'}};
window.fixtureVideo=[{id:'video',title:'Video fixture',webpageUrl:'https://video.test/watch',state:'failed',downloadedBytes:0,totalBytes:1200,bytesPerSecond:0,etaSeconds:null,error:'No supported video formats',createdAt:'2026-09-30T00:00:00Z'}];
window.fixtureMusic=[{id:'music',track,format:'mp3',state:'failed',path:null,directory:'C:/Fixture/Downloads',downloadedBytes:0,totalBytes:1200,bytesPerSecond:0,etaSeconds:null,error:'i/o error: Permission denied (os error 13)',createdAt:'2026-09-30T00:00:00Z',position:1}];
let callbackId=0; const callbacks=new Map(); const events=new Map();
window.calls=[]; window.unexpected=[];
window.emitQueue=()=>events.get('zoltraak://queue')?.({event:'zoltraak://queue',id:1,payload:clone(window.fixtureRows)});
window.emitPrefill=payload=>events.get('zoltraak://download-prefill')?.({event:'zoltraak://download-prefill',id:1,payload});
window.__TAURI_INTERNALS__={transformCallback:callback=>{const id=++callbackId;callbacks.set(id,callback);return id;},unregisterCallback:id=>callbacks.delete(id),invoke:async(name,args={})=>{
  if(name==='plugin:event|listen'){events.set(args.event,callbacks.get(args.handler));return ++callbackId;}
  if(name==='plugin:event|unlisten')return;
  if(name==='get_settings')return {...DEFAULT_ENGINE_SETTINGS,desktopNotifications:false,playSoundOnComplete:false};
  if(name==='queue_snapshot')return clone(window.fixtureRows);
  if(name==='list_video_jobs')return clone(window.fixtureVideo);
  if(name==='list_music_jobs')return clone(window.fixtureMusic);
  if(name==='default_directory')return 'C:/Fixture/Downloads';
  if(name==='download_file_status')return {path:'C:/Fixture/Downloads/fixture.zip',status:'present',detail:null};
  if(name==='update_progress')return null;
  if(name==='add_download'){
    window.calls.push({name,...args});
    if(args.url.endsWith('/bad.zip'))throw {message:'Fixture server rejected this URL'};
    const id='added-'+window.calls.length;
    window.fixtureRows.push({...row(id,new URL(args.url).pathname.split('/').pop(),'waiting'),url:args.url,directory:args.directory||'C:/Fixture/Downloads'});
    return id;
  }
  if(['resume_download','resume_video_job','resume_music_job'].includes(name)){
    window.calls.push({name,...args});
    const rows=name==='resume_download'?window.fixtureRows:name==='resume_video_job'?window.fixtureVideo:window.fixtureMusic;
    const item=rows.find(item=>item.id===args.id);if(item){item.state='waiting';item.error=null;}
    return;
  }
  if(name==='choose_directory'){window.calls.push({name,...args});return 'C:/Fixture/Recovered';}
  if(name==='analyze_video'){
    window.calls.push({name,...args});
    return {title:'Recovered video',webpageUrl:args.url,formats:[{id:'direct',container:'mp4',height:720,hasAudio:true,videoUrl:'https://cdn.test/clip.mp4',requiresMerge:false,validation:{status:'verified'}}]};
  }
  if(name==='start_video_download'){window.calls.push({name,...args});return 'recovered-video';}
  if(name==='music_settings')return {maxConcurrent:2,output:{format:'mp3',bitrateKbps:192,folderMode:'none',filenameTemplate:'{artist} - {title}',numberTracks:false,embedMetadata:false,embedCover:false,saveCover:false,fetchArtwork:false,skipExisting:true}};
  if(name==='music_tools')return {ffmpeg:true,ffprobe:true};
  if(name==='music_directory')return 'C:/Fixture/Downloads/Music';
  if(name==='list_sessions'||name==='detect_browsers')return [];
  if(name==='api_status')return {listening:true,port:1450,token:'test-pairing-token',error:null};
  if(name==='analyze_music'){window.calls.push({name,...args});return {kind:'track',service:'soundcloud',title:track.title,sourceUrl:track.sourceUrl,tracks:[track],total:1};}
  if(name==='start_music_download'){window.calls.push({name,...args});return {jobIds:['recovered-music'],skipped:0,directory:args.options.directory||'C:/Fixture/Downloads/Music'};}
  if(name==='list_remux_jobs')return [];
  if(name==='reveal_download'||name==='get_app_version')return '0.1.8';
  window.unexpected.push(name);throw new Error('Unexpected native fixture command: '+name);
}};
window.Notification=class{static permission='denied';static requestPermission=async()=> 'denied';};
createRoot(document.getElementById('root')).render(<App/>);
`},bundle:true,write:false,format:'iife',platform:'browser',jsx:'automatic'});
const assets=path.join(root,'dist','assets');const css=(await readdir(assets)).find(name=>name.endsWith('.css'));
const server=createServer(async(req,res)=>{
  if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
  else if(req.url.startsWith('/assets/')){try{res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))));}catch{res.writeHead(404);res.end();}}
  else {res.setHeader('Content-Type','text/html');res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true});
try{
  const page=await browser.newPage({viewport:{width:1360,height:900},permissions:['clipboard-read','clipboard-write']});
  const errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.goto('http://127.0.0.1:'+server.address().port);
  await page.getByRole('textbox',{name:'Download URLs',exact:true}).waitFor();
  await page.locator('[data-row-id="music"]').waitFor();
  await page.clock.install();
  const advance=async(ms=600)=>{await page.clock.runFor(ms);await page.waitForTimeout(30);};
  const calls=()=>page.evaluate(()=>window.calls);

  // Batch review rejects duplicates before any engine call and retains failures.
  const input=page.getByRole('textbox',{name:'Download URLs',exact:true});
  await input.fill('https://files.test/good.zip\nhttps://files.test/bad.zip\nhttps://files.test/good.zip\nhttps://files.test/clip2.zip\nbroken-link');
  await page.getByRole('button',{name:'Review links',exact:true}).click();
  assert.equal((await calls()).filter(call=>call.name==='add_download').length,0);
  await page.getByText('2 ready · 2 duplicates skipped · 1 invalid',{exact:true}).waitFor();
  await page.screenshot({path:path.join(output,'batch-review.png'),fullPage:true});
  await page.getByRole('button',{name:'Add 2 downloads',exact:true}).click();
  await page.getByText('Added 1 download.',{exact:true}).waitFor();
  assert.equal(await input.inputValue(),'https://files.test/bad.zip\nbroken-link');
  assert.equal((await calls()).filter(call=>call.name==='add_download').length,2);
  await page.getByText('https://files.test/bad.zip: Fixture server rejected this URL',{exact:true}).waitFor();
  await input.fill('');

  // Extension credentials stay attached only to the originally captured URL.
  await page.evaluate(()=>window.emitPrefill({url:'https://secure.test/private.zip',headers:[['Cookie','session=fixture']]}));
  await page.waitForFunction(()=>document.querySelector('textarea[aria-label="Download URLs"]').value==='https://secure.test/private.zip');
  await page.getByRole('button',{name:'Add download',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(call=>call.name==='add_download'&&call.url==='https://secure.test/private.zip'));
  assert.deepEqual((await calls()).find(call=>call.name==='add_download'&&call.url==='https://secure.test/private.zip').headers,[['Cookie','session=fixture']]);
  await input.fill('https://other.test/public.zip');
  await page.getByRole('button',{name:'Add download',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(call=>call.name==='add_download'&&call.url==='https://other.test/public.zip'));
  assert.equal((await calls()).find(call=>call.name==='add_download'&&call.url==='https://other.test/public.zip').headers,undefined);

  // Actual App recovery actions preserve pipeline routing and chosen destinations.
  await page.locator('[aria-label="Recovery for locked.zip"]').getByRole('button',{name:'Choose another folder',exact:true}).click();
  assert.equal(await input.inputValue(),'https://files.test/locked.zip');
  await page.getByRole('button',{name:'Add download',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(call=>call.name==='add_download'&&call.url.endsWith('/locked.zip')&&call.directory==='C:/Fixture/Recovered'));
  await page.locator('[aria-label="Recovery for login.mp4"]').getByRole('button',{name:'Share sign-in',exact:true}).click();
  await page.getByRole('dialog',{name:'Browser Extension Setup Guide',exact:true}).waitFor();
  await page.getByRole('button',{name:'Close extension setup',exact:true}).click();
  await page.locator('[aria-label="Recovery for Video fixture"]').getByRole('button',{name:'Try another extraction method',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(call=>call.name==='analyze_video'&&call.useFallback===true));
  await page.getByRole('button',{name:'Download preset',exact:true}).waitFor();
  await page.getByRole('tab',{name:'Failed 1',exact:true}).click();
  // A file-system error now offers a destination change in the dedicated view.
  await page.evaluate(()=>{window.fixtureVideo[0].error='i/o error: Permission denied (os error 13)';});
  await advance(2100);
  await page.locator('[aria-label="Recovery for Video fixture"]').getByRole('button',{name:'Choose another folder',exact:true}).click();
  await page.getByText('This download will use: C:/Fixture/Recovered',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Download preset',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(call=>call.name==='start_video_download'&&call.directory==='C:/Fixture/Recovered'));
  await page.getByRole('button',{name:'Downloads',exact:true}).click();
  await page.locator('[aria-label="Recovery for Fixture artist - Music fixture"]').getByRole('button',{name:'Choose another folder',exact:true}).click();
  await page.getByRole('button',{name:'download all (1)',exact:true}).waitFor();
  await page.getByRole('button',{name:'download all (1)',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(call=>call.name==='start_music_download'&&call.options.directory==='C:/Fixture/Recovered'));
  await page.getByRole('button',{name:'Downloads',exact:true}).click();

  // Persist sorting, density, filter, search and column order across a real reload.
  await page.getByRole('combobox',{name:'Sort downloads',exact:true}).selectOption('name');
  await page.getByRole('button',{name:'Reverse sort direction',exact:true}).click();
  await page.getByRole('combobox',{name:'Download row density',exact:true}).selectOption('comfortable');
  await page.getByText('Arrange columns',{exact:true}).click();
  await page.getByRole('combobox',{name:'Column to move',exact:true}).selectOption('rate');
  await page.getByRole('button',{name:'Move left',exact:true}).click();
  await page.getByRole('textbox',{name:'Search downloads',exact:true}).fill('clip2');
  await page.getByRole('group',{name:'Filter by state',exact:true}).getByRole('button',{name:'Finished',exact:true}).click();
  const stored=await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.download-view.v1')));
  await page.reload();
  await page.getByRole('textbox',{name:'Search downloads',exact:true}).waitFor();
  assert.equal(await page.getByRole('textbox',{name:'Search downloads',exact:true}).inputValue(),'clip2');
  assert.equal(await page.getByRole('combobox',{name:'Sort downloads',exact:true}).inputValue(),'name');
  assert.equal(await page.getByRole('combobox',{name:'Download row density',exact:true}).inputValue(),'comfortable');
  assert.equal(await page.getByRole('button',{name:'Finished',exact:true}).getAttribute('aria-pressed'),'true');
  assert.deepEqual(await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.download-view.v1')).columnOrder),stored.columnOrder);

  // Grouped completion notifications across pipelines reopen entries hidden by a saved view.
  await page.getByRole('textbox',{name:'Search downloads',exact:true}).fill('does-not-match');
  await page.evaluate(()=>{
    window.fixtureRows.push({...window.fixtureRows[0],id:'done-a',filename:'first.zip',state:'waiting',url:'https://files.test/first.zip'});
    window.fixtureRows.push({...window.fixtureRows[0],id:'done-b',filename:'second.zip',state:'waiting',url:'https://files.test/second.zip'});
    window.emitQueue();
  });
  await advance(50);
  await page.evaluate(()=>{for(const item of window.fixtureRows)if(item.id.startsWith('done-'))item.state='finished';window.emitQueue();});
  await advance(600);
  await page.locator('.t-toast').getByText('2 downloads completed',{exact:true}).waitFor();
  await page.getByRole('button',{name:/^Notifications:/}).click();
  const history=page.getByRole('region',{name:'Notification history',exact:true});
  await page.waitForFunction(()=>!document.querySelector('.t-toast'));
  assert.equal(await page.locator('.t-toast').count(),0,'toast popups must not obscure notification history');
  await history.getByRole('button',{name:'Open download notification: 2 downloads completed',exact:true}).click();
  await page.locator('[data-row-id="done-a"][data-open="true"]').waitFor();
  assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.download-view.v1')).query),'');
  assert.equal(await page.getByRole('checkbox',{name:'Select first.zip',exact:true}).getAttribute('aria-checked'),'true');

  // Notify a new failure and retry only IDs that are still failed.
  await page.evaluate(()=>{const item=window.fixtureRows.find(item=>item.id==='network');item.state='waiting';window.emitQueue();});
  await advance(50);
  await page.evaluate(()=>{const item=window.fixtureRows.find(item=>item.id==='network');item.state='failed';item.error='Connection timed out';window.emitQueue();});
  await advance(600);
  await page.getByRole('button',{name:/^Notifications:/}).click();
  await history.getByRole('button',{name:'Errors only',exact:true}).click();
  assert.equal(await history.getByRole('button',{name:'Open download notification: 2 downloads completed',exact:true}).count(),0);
  await history.getByRole('button',{name:'Retry failed',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(call=>call.name==='resume_download'&&call.id==='network'));
  await advance(100);
  assert.equal(await history.getByRole('button',{name:'Retry failed',exact:true}).isDisabled(),true);
  await page.getByRole('button',{name:/^Notifications:/}).click();
  await page.waitForTimeout(300);
  await page.screenshot({path:path.join(output,'downloads-desktop.png'),fullPage:true});
  await page.setViewportSize({width:900,height:700});
  await page.screenshot({path:path.join(output,'downloads-narrow.png'),fullPage:true});
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth),false);
  await page.setViewportSize({width:1360,height:900});
  await page.emulateMedia({colorScheme:'dark'});
  await page.waitForTimeout(300);
  await page.screenshot({path:path.join(output,'downloads-dark.png'),fullPage:true});
  // A historical notification never recreates a download that was removed.
  await page.evaluate(()=>{window.fixtureRows=window.fixtureRows.filter(item=>!item.id.startsWith('done-'));window.emitQueue();});
  await advance(100);
  await page.getByRole('button',{name:/^Notifications:/}).click();
  await history.getByRole('button',{name:'All',exact:true}).click();
  const addedBefore=(await calls()).filter(call=>call.name==='add_download').length;
  await history.getByRole('button',{name:'Open download notification: 2 downloads completed',exact:true}).click();
  await page.locator('.t-toast').getByText('Entry unavailable',{exact:true}).waitFor();
  assert.equal((await calls()).filter(call=>call.name==='add_download').length,addedBefore);
  assert.deepEqual(await page.evaluate(()=>window.unexpected),[]);
  assert.deepEqual(errors,[]);
  console.log('Download UX App checks passed: review/partial failures, all recovery routes, saved views, grouped notification navigation, errors filter and targeted retries.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
