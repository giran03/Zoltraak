// Exercise the real App and hooks. Only the native engine boundary is mocked.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';
const {chromium}=await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const output=path.resolve(root,'../scratch/ux-release2');
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
  if(name==='get_license_status')return {activated:true,deviceId:'fixture',entitlements:[],isOffline:false};
  if(name==='get_settings')return {...DEFAULT_ENGINE_SETTINGS,desktopNotifications:false,playSoundOnComplete:false};
  if(name==='queue_snapshot')return clone(window.fixtureRows);
  if(name==='list_video_jobs')return clone(window.fixtureVideo);
  if(name==='list_music_jobs')return clone(window.fixtureMusic);
  if(name==='default_directory')return 'C:/Fixture/Downloads';
  if(name==='update_progress')return null;
  if(name==='engine_ready') {if(window.healthFailure)throw {message:'Engine fixture disconnected'};return true;}
  if(name==='destination_health')return {path:'C:/Fixture/Downloads',writable:!window.folderBlocked,detail:window.folderBlocked?'Access denied':null};
  if(name==='extension_source_state')return {revision:1,clients:[],preferredClientId:null,activeClientId:null};
  if(name==='get_extension_version'||name==='get_firefox_extension_version')return '0.1.8';
  if(name==='download_file_status'){window.calls.push({name,...args});return {path:window.locatedPath||'C:/Fixture/Downloads/clip2.zip',status:window.fileState||'missing',detail:window.fileState==='inaccessible'?'Drive unavailable':'The file was moved or deleted outside the app.'};}
  if(name==='locate_download_file'){window.calls.push({name,...args});if(window.locateError)throw {message:'Choose a regular file'};if(window.cancelLocate)return null;window.locatedPath='C:/Fixture/Moved/clip2.zip';window.fileState='present';window.fixtureRows.find(row=>row.id===args.id).directory='C:/Fixture/Moved';window.emitQueue();return window.locatedPath;}
  if(name==='redownload_missing'){window.calls.push({name,...args});window.fixtureRows.find(row=>row.id===args.id).state='waiting';window.emitQueue();return;}
  if(name==='remove_download'){window.calls.push({name,...args});window.fixtureRows=window.fixtureRows.filter(row=>row.id!==args.id);window.emitQueue();return;}
  if(name==='get_extension_directory')return 'C:/Fixture/Extension';

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
  if(name==='music_tools')return {ffmpeg:'Fixture FFmpeg',ffprobe:true};
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


  const old = page.locator('[data-row-id="old"]');
  await old.getByRole('button',{name:'Open clip2.zip',exact:true}).click();
  await old.getByText('The file was moved or deleted outside the app.',{exact:true}).waitFor();
  assert.equal(await old.getByRole('button',{name:'Download again',exact:true}).count(),1);
  await page.screenshot({path:path.join(output,'missing-file-details.png'),fullPage:false});
  // Picker cancellation preserves the missing record; failures remain visible.
  await page.evaluate(()=>window.cancelLocate=true);
  await old.getByRole('button',{name:'Locate file',exact:true}).click();
  await advance(100);
  assert.equal(await old.getByRole('button',{name:'Download again',exact:true}).count(),1);
  await page.evaluate(()=>{window.cancelLocate=false;window.locateError=true;});
  await old.getByRole('button',{name:'Locate file',exact:true}).click();
  await old.getByText('Could not check or update file: Error: Choose a regular file',{exact:true}).waitFor();
  await page.evaluate(()=>window.locateError=false);
  await old.getByRole('button',{name:'Locate file',exact:true}).click();
  await old.getByText('File available on disk',{exact:true}).waitFor();
  await old.getByText('C:/Fixture/Moved/clip2.zip',{exact:true}).first().waitFor();
  assert.equal(await old.getByRole('button',{name:'Download again',exact:true}).count(),0);
  // An unavailable destination cannot offer a restart; focus rechecks external changes.
  await page.evaluate(()=>{window.fileState='inaccessible';window.dispatchEvent(new Event('focus'));});
  await old.getByText('Drive unavailable',{exact:true}).waitFor();
  assert.equal(await old.getByRole('button',{name:'Download again',exact:true}).count(),0);
  await page.evaluate(()=>{window.fileState='missing';window.dispatchEvent(new Event('focus'));});
  await old.getByRole('button',{name:'Download again',exact:true}).click();
  await page.waitForFunction(()=>window.calls.some(call=>call.name==='redownload_missing'&&call.id==='old'));
  await page.evaluate(()=>{window.fixtureRows.find(row=>row.id==='old').state='finished';window.emitQueue();});
  await old.getByRole('button',{name:'Remove entry only',exact:true}).click();
  await page.locator('.t-toast').getByRole('button',{name:'Undo',exact:true}).click();
  assert.equal((await calls()).filter(call=>call.name==='remove_download').length,0);
  await old.getByRole('button',{name:'Open clip2.zip',exact:true}).click();
  await old.getByRole('button',{name:'Remove entry only',exact:true}).click();
  await advance(6500);
  const removed=(await calls()).find(call=>call.name==='remove_download');
  assert.equal(removed.deleteFile,false,'entry-only action must ignore default file deletion setting');

  // One details component serves dedicated video and music jobs with real metadata.
  await page.evaluate(()=>{window.fixtureVideo[0].state='finished';window.fixtureVideo[0].path='C:/Fixture/video.mp4';window.fixtureVideo[0].formatId='720p';window.fixtureVideo[0].validation={status:'verified',hasAudio:true,samplesDecoded:true};window.fixtureMusic[0].state='finished';window.fixtureMusic[0].path='C:/Fixture/song.mp3';});
  await advance(2100);
  await page.getByRole('button',{name:'Video',exact:true}).click();
  await page.getByRole('tab',{name:'Finished 1',exact:true}).click();
  await page.getByText('Download details · Video fixture',{exact:true}).click();
  await page.getByText('Audio track confirmed',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Music',exact:true}).click();
  await page.getByRole('tab',{name:'Finished 1',exact:true}).click();
  await page.getByText('Download details · song.mp3',{exact:true}).click();
  await page.locator('dd').getByText('mp3',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Settings',exact:true}).click();
  await page.getByText('Engine responding',{exact:true}).waitFor();
  await page.getByText('No paired browser recorded',{exact:true}).waitFor();
  assert.equal(await page.locator('#section-setup-health').getByText(/Write access confirmed/).count(),1);
  await advance(6500);
  await page.screenshot({path:path.join(output,'settings-desktop.png'),fullPage:false});
  const search=page.getByRole('searchbox',{name:'Find a setting',exact:true});
  await search.fill('delete');await search.press('Enter');await advance(100);
  assert.equal(await page.evaluate(()=>document.activeElement.getAttribute('aria-label')),'Delete local file with trash button');
  await search.fill('columns');
  await page.getByRole('button',{name:'Download columns Appearance',exact:true}).click();await advance(100);
  assert.equal(await page.getByRole('tab',{name:'Appearance',exact:true}).getAttribute('aria-selected'),'true');
  assert.equal(await page.evaluate(()=>document.activeElement.id),'section-register-columns-row-presentation');
  await search.fill('nonexistent');await page.getByText('No settings found. Try another term.',{exact:true}).waitFor();await search.press('Escape');
  await page.getByRole('tab',{name:'General',exact:true}).click();
  await page.evaluate(()=>{window.healthFailure=true;window.folderBlocked=true;});
  await page.locator('#section-setup-health').getByRole('button',{name:'Check again',exact:true}).first().click();
  await page.getByText('Check failed: Error: Engine fixture disconnected',{exact:true}).waitFor();
  await page.getByText('C:/Fixture/Downloads — Access denied',{exact:true}).waitFor();
  await page.locator('#section-setup-health').getByRole('button',{name:'Choose folder',exact:true}).click();await advance(100);
  assert.equal(await page.evaluate(()=>document.activeElement.id),'section-engine-defaults');
  await page.getByRole('searchbox',{name:'Find a setting',exact:true}).fill('health');await search.press('Enter');await advance(100);
  await page.locator('#section-setup-health').getByRole('button',{name:'Pair / manage',exact:true}).click();
  await page.getByRole('dialog',{name:'Browser Extension Setup Guide',exact:true}).waitFor();
  await page.getByRole('button',{name:'Close extension setup',exact:true}).click();
  await page.locator('#section-setup-health').getByRole('button',{name:'Media tools',exact:true}).click();await advance(100);
  assert.equal(await page.evaluate(()=>document.activeElement.id),'section-media-processing-codecs-ffmpeg');
  await page.locator('#section-setup-health').getByRole('button',{name:'Manage sign-ins',exact:true}).click();await advance(100);
  assert.equal(await page.getByRole('tab',{name:'Extension',exact:true}).getAttribute('aria-selected'),'true');
  assert.equal(await page.evaluate(()=>document.activeElement.id),'section-cookie-source');
  await page.getByRole('tab',{name:'General',exact:true}).click();
  await page.setViewportSize({width:760,height:900});
  await page.getByRole('searchbox',{name:'Find a setting',exact:true}).fill('folder');await advance(300);
  await page.evaluate(()=>document.getElementById('settings-search').closest('.overflow-y-auto').scrollTop=0);
  await page.screenshot({path:path.join(output,'settings-narrow.png'),fullPage:false});
  await page.setViewportSize({width:1360,height:900});await search.fill('');
  await page.emulateMedia({colorScheme:'dark'});await advance(300);
  await page.evaluate(()=>document.getElementById('settings-search').closest('.overflow-y-auto').scrollTop=0);
  await page.screenshot({path:path.join(output,'settings-dark.png'),fullPage:false});
  assert.deepEqual(await page.evaluate(()=>window.unexpected),[]);
  assert.deepEqual(errors,[]);
  console.log('UX 5–8 checks passed: file lifecycle and Undo, shared details, cross-tab search, partial setup failures and repair routes.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
