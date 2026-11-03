// Real download rows, settings and Undo hook; only native filesystem commands are mocked.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.resolve(root, '../scratch/download-removal');
await mkdir(output, { recursive: true });
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'fixture.tsx', loader: 'tsx', contents: `
import React, {useCallback, useMemo, useState} from 'react';
import { createRoot } from 'react-dom/client';
import { DownloadsView } from './src/components/DownloadsView';
import { VideoView } from './src/components/VideoView';
import { MusicView } from './src/components/MusicView';
import { groupJobs } from './src/lib/useMusic';
import { musicJobToSnapshot, videoJobToSnapshot } from './src/lib/snapshots';
import { useSettings } from './src/lib/useSettings';
import { SettingsView } from './src/components/SettingsView';
import { useAutosaveSettings } from './src/lib/useAutosaveSettings';
import { useSoftRemove } from './src/lib/useSoftRemove';
import { DEFAULT_ENGINE_SETTINGS, engine } from './src/lib/engine';
let settings = {...DEFAULT_ENGINE_SETTINGS};
window.calls=[];
window.failRemoval=false;
window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
  if(name==='get_settings')return {...settings};
  if(name==='update_settings'){settings={...args.settings};return;}
  if(name==='get_app_version')return '0.1.8';
  if(name==='music_tools'||name==='update_progress')return null;
  if(name.startsWith('plugin:event|'))return 1;
  if(['remove_download','remove_video_job','remove_music_job'].includes(name)) {
    window.calls.push({name,...args});
    if(window.failRemoval)throw {message:'Access denied: fixture file is locked'};
    return;
  }
  throw new Error('Unexpected fixture command: '+name);
}};
const queueRow={id:'queue',filename:'Report.zip',url:'https://fixture.test/report.zip',directory:'C:/Downloads',state:'finished',downloadedBytes:1200,totalBytes:1200,bytesPerSecond:0,etaSeconds:null,connections:1,priority:0,error:null};
const videoRow={id:'video',title:'Video clip',webpageUrl:'https://fixture.test/watch',state:'finished',path:'C:/Downloads/clip.mp4',downloadedBytes:1200,totalBytes:1200,bytesPerSecond:0,etaSeconds:null};
const musicRow={id:'music',track:{title:'Music track',artists:['Fixture artist'],sourceUrl:'https://fixture.test/music'},state:'finished',path:'C:/Downloads/song.mp3',downloadedBytes:1200,totalBytes:1200,bytesPerSecond:0,etaSeconds:null};
function Fixture() {
  const [tab,setTab]=useState('downloads');
  const settingsState=useAutosaveSettings();
  const {settings:sharedSettings}=useSettings();
  const deleteFilesOnRemove=sharedSettings?.deleteFilesOnRemove ?? true;
  const [removed,setRemoved]=useState(new Set());
  const [toast,setToast]=useState(null);
  const [error,setError]=useState(null);
  const addToast=useCallback(t=>setToast(t),[]);
  const softRemove=useSoftRemove({addToast,commit:async(item,deleteFile)=>{
    try {
      await (item.isVideo?engine.removeVideoJob(item.id,deleteFile):item.isMusic?engine.removeMusicJob(item.id,deleteFile):engine.remove(item.id,deleteFile));
      setRemoved(prev=>new Set([...prev,item.id]));
      return true;
    } catch(e) {setError(e.message);return false;}
  }});
  const actions=useMemo(()=>({remove:softRemove.remove,toggle:()=>{},pause:async()=>{},resume:async()=>{},reveal:()=>{}}),[softRemove.remove]);
  return <div className="p-6"><nav className="flex gap-4 mb-6"><button onClick={()=>setTab('downloads')}>Downloads fixture</button><button onClick={()=>setTab('settings')}>General settings fixture</button><button onClick={()=>setTab('video')}>Video fixture</button><button onClick={()=>setTab('music')}>Music fixture</button></nav>
    {tab==='settings'?<SettingsView settingsState={settingsState}/>:tab==='video'?<VideoView video={{metadata:null,error:null,analyzing:false,jobs:(removed.has('video')||softRemove.pending.has('video'))?[]:[videoRow]}} onRemove={job=>softRemove.remove([videoJobToSnapshot(job)],deleteFilesOnRemove)} deleteFilesOnRemove={deleteFilesOnRemove}/>:tab==='music'?<MusicView music={{settings:null,source:null,selectedTracks:[],groups:groupJobs((removed.has('music')||softRemove.pending.has('music'))?[]:[musicRow]),jobs:(removed.has('music')||softRemove.pending.has('music'))?[]:[musicRow]}} onRemove={job=>softRemove.remove([musicJobToSnapshot(job)],deleteFilesOnRemove)} deleteFilesOnRemove={deleteFilesOnRemove}/>:<DownloadsView items={removed.has('queue')?[]:[queueRow]} videoJobs={(removed.has('video')||softRemove.pending.has('video'))?[]:[videoRow]} musicJobs={(removed.has('music')||softRemove.pending.has('music'))?[]:[musicRow]} pendingRemoval={softRemove.pending} status="live" error={error} onClearError={()=>setError(null)} onAdd={async()=>true} actions={actions} deleteFilesOnRemove={deleteFilesOnRemove} destination={null} composing={false} onComposingChange={()=>{}}/>}
    {toast&&<aside aria-label="Removal notification" className="plate p-4 mt-4"><p>{toast.title}</p><p>{toast.message}</p><button onClick={()=>{toast.action.onClick();setToast(null);}}>Undo</button></aside>}
  </div>;
}
createRoot(document.getElementById('root')).render(<Fixture/>);
` }, bundle: true, write: false, format: 'iife', platform: 'browser', jsx: 'automatic' });
const assets = path.join(root, 'dist', 'assets');
const css = (await readdir(assets)).find(name => name.endsWith('.css'));
const server = createServer(async (req, res) => {
  if (req.url === '/fixture.js') {
    res.setHeader('Content-Type', 'text/javascript'); res.end(bundle.outputFiles[0].text);
  } else if (req.url.startsWith('/assets/')) {
    res.setHeader('Content-Type', req.url.endsWith('.css') ? 'text/css' : 'font/woff2');
    try {res.end(await readFile(path.join(assets,path.basename(req.url))));}
    catch {res.writeHead(404);res.end();}
  } else {
    res.setHeader('Content-Type','text/html');
    res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');
  }
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true});
try {
  const page=await browser.newPage({viewport:{width:1200,height:900}});
  const errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  const url='http://127.0.0.1:'+server.address().port;
  const calls=()=>page.evaluate(()=>window.calls);
  const advance=async()=>{await page.clock.runFor(6100);await page.waitForTimeout(50);};
  await page.goto(url);
  await page.clock.install();
  await page.getByRole('button',{name:'Delete Report.zip and local file',exact:true}).click();
  assert.equal((await calls()).length,0,'file must remain during Undo window');
  await page.getByText('The local file will be permanently deleted when Undo expires.').waitFor();
  await page.getByRole('button',{name:'Undo',exact:true}).click();
  await advance();
  assert.equal((await calls()).length,0,'Undo must cancel filesystem deletion');
  await page.getByRole('button',{name:'Delete Report.zip and local file',exact:true}).waitFor();
  await page.getByText('Report.zip',{exact:true}).click({button:'right'});
  await page.getByRole('menuitem',{name:'Delete entry and file',exact:true}).waitFor();
  await page.screenshot({path:path.join(output,'context-menu.png'),fullPage:true});
  await page.getByRole('menuitem',{name:'Remove from list',exact:true}).click();
  await advance();
  assert.deepEqual(await calls(),[{name:'remove_download',id:'queue',deleteFile:false}]);

  await page.getByRole('button',{name:'General settings fixture'}).click();
  const toggle=page.getByRole('switch',{name:'Delete local file with trash button',exact:true});
  assert.equal(await toggle.getAttribute('aria-checked'),'true');
  await toggle.click();
  assert.equal(await toggle.getAttribute('aria-checked'),'false');
  await page.screenshot({path:path.join(output,'general-setting.png'),fullPage:true});
  await page.getByRole('button',{name:'Downloads fixture',exact:true}).click();
  await page.getByRole('button',{name:'Remove Video clip from list',exact:true}).waitFor();
  // Explicit right-click deletion overrides the disabled default for every pipeline.
  await page.getByText('Video clip',{exact:true}).click({button:'right'});
  await page.getByRole('menuitem',{name:'Delete entry and file',exact:true}).click();
  await advance();
  assert.deepEqual((await calls()).at(-1),{name:'remove_video_job',id:'video',deleteFile:true});
  await page.getByRole('button',{name:'Remove song.mp3 from list',exact:true}).click();
  await advance();
  assert.deepEqual((await calls()).at(-1),{name:'remove_music_job',id:'music',deleteFile:false});

  await page.reload();
  await page.getByRole('button',{name:'Delete Report.zip and local file',exact:true}).click();
  await advance();
  assert.deepEqual((await calls()).at(-1),{name:'remove_download',id:'queue',deleteFile:true});
  // Locked files surface the error and restore the row.
  await page.reload();
  await page.evaluate(()=>{window.failRemoval=true;});
  await page.getByRole('button',{name:'Delete Report.zip and local file',exact:true}).click();
  await advance();
  await page.getByText('Access denied: fixture file is locked',{exact:true}).waitFor();
  await page.getByRole('button',{name:'Delete Report.zip and local file',exact:true}).waitFor();
  await page.reload();
  await page.getByRole('button',{name:'Video fixture',exact:true}).click();
  await page.getByRole('tab',{name:'Finished 1',exact:true}).click();
  await page.getByRole('button',{name:'Delete Video clip and local file',exact:true}).click();
  assert.equal(await page.getByRole('button',{name:'Delete Video clip and local file',exact:true}).count(),0);
  await page.getByRole('button',{name:'Undo',exact:true}).click();
  await page.getByRole('button',{name:'Delete Video clip and local file',exact:true}).waitFor();
  assert.equal((await calls()).length,0);
  await page.getByRole('button',{name:'Delete Video clip and local file',exact:true}).click();
  await advance();
  assert.deepEqual((await calls()).at(-1),{name:'remove_video_job',id:'video',deleteFile:true});
  await page.getByRole('button',{name:'Music fixture',exact:true}).click();
  await page.getByRole('tab',{name:'Finished 1',exact:true}).click();
  await page.getByRole('button',{name:'Delete Music track and local file',exact:true}).click();
  assert.equal(await page.getByRole('button',{name:'Delete Music track and local file',exact:true}).count(),0);
  await page.getByRole('button',{name:'Undo',exact:true}).click();
  await page.getByRole('button',{name:'Delete Music track and local file',exact:true}).waitFor();
  await page.getByRole('button',{name:'Delete Music track and local file',exact:true}).click();
  await advance();
  assert.deepEqual((await calls()).at(-1),{name:'remove_music_job',id:'music',deleteFile:true});
  assert.deepEqual(errors,[]);
  console.log('Download removal browser checks passed: defaults, settings sync, explicit choices, pipeline dispatch, Undo, and failure recovery.');
} finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
