// Uses the project's real hook, extension settings, and toast queue with fake
// browser heartbeats. No extension is installed into a user's browser.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';
const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'extension-fixture.tsx', loader: 'tsx', contents: `
import React, {useState} from 'react';
import {createRoot} from 'react-dom/client';
import {engine} from './src/lib/engine';
import {useExtensionUpdates} from './src/lib/useExtensionUpdates';
import {useToasts} from './src/lib/useToasts';
import {ExtensionSettings} from './src/components/ExtensionSettings';
import {ToastContainer} from './src/components/Toast';
import {PanelHeader} from './src/components/PanelHeader';
import {AddDownload} from './src/components/AddDownload';
window.__TAURI_INTERNALS__={};
window.fixtureVersion='0.1.41';window.failUpdate=false;window.nativeUpdates=[];
window.profiles=['profile-a','profile-b'].map(clientId=>({clientId,browserId:'zen',browserName:'Zen',extensionVersion:'0.1.40',lastSeenAtMs:Date.now(),online:true,preferred:false,active:false,temporary:false}));
engine.apiStatus=async()=>({listening:true,port:1450,token:'fixture-not-a-real-token'});
engine.getAppVersion=async()=> '0.2.1';
engine.detectBrowsers=async()=>[{id:'zen',name:'Zen',kind:'firefox',running:true,installed:true,executablePath:'C:/Fixture/zen.exe'}];
engine.getExtensionVersion=engine.getFirefoxExtensionVersion=async()=>window.fixtureVersion;
engine.extensionSourceState=async()=>({revision:1,preferredClientId:null,activeClientId:null,clients:window.profiles.map(p=>({...p}))});
engine.sessions=async()=>[];
engine.reloadExtension=async browserId=>{window.nativeUpdates.push(browserId);if(window.failUpdate)throw new Error('Zen installer could not open');return [{browserId:'zen',browserName:'Zen',success:true,message:'Confirm the signed installation in Zen.',extensionPath:'C:/Fixture/zoltraak-0.1.41.xpi',nextStep:'browser_confirmation'}];};
engine.openExtensionManager=async browserId=>{window.openedManager=browserId;};
function Fixture(){
 const [settings,setSettings]=useState(false);const toasts=useToasts();const updates=useExtensionUpdates(toasts.add,()=>setSettings(true));
 window.extensionFixture=()=>({pending:updates.pending,busy:updates.busy,history:toasts.history,toasts:toasts.toasts.map(t=>({id:t.id,title:t.title,type:t.type})),settings});
 window.refreshExtensions=updates.refresh;window.updateExtension=updates.update;window.openSettings=()=>setSettings(true);
 return <div className="flex min-h-dvh flex-col"><PanelHeader active="Downloads" onSelect={()=>{}} status="live" throughput={0} extensionUpdates={updates} onOpenExtension={()=>setSettings(true)}/><main className="mx-auto w-full max-w-5xl flex-1 p-6"><button onClick={()=>setSettings(!settings)}>Toggle extension settings</button>{settings?<ExtensionSettings updates={updates} onAddToast={toasts.add}/>:<h1>Downloads fixture</h1>}<ToastContainer toasts={toasts.toasts} onDismiss={toasts.dismiss}/></main><AddDownload open={false} onOpenChange={()=>{}} onSubmit={async()=>({added:[],failed:[]})} destination="C:/Downloads/Zoltraak"/></div>;
}createRoot(document.getElementById('root')).render(<Fixture/>);
` }, bundle:true, write:false, format:'iife', platform:'browser', jsx:'automatic' });
const assets=path.join(root,'dist','assets'), css=(await readdir(assets)).find(name=>name.endsWith('.css'));
const server=createServer(async(req,res)=>{
  if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
  else if(req.url.startsWith('/assets/')){try{res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))));}catch{res.writeHead(404);res.end();}}
  else{res.setHeader('Content-Type','text/html');res.end('<!doctype html><html lang="en" data-theme="dark"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
const screenshotDir=process.env.QA_SCREENSHOT_DIR;
try{
  const page=await browser.newPage({viewport:{width:1366,height:900}}),errors=[];page.on('pageerror',error=>errors.push(error.message));
  await page.goto('http://127.0.0.1:'+server.address().port);
  await page.getByText('Extension update available',{exact:true}).waitFor();
  await page.getByLabel('App version 0.2.1',{exact:true}).waitFor();
  assert.equal(await page.getByLabel('App version 0.2.1').evaluate(el=>el.parentElement.querySelector('button[aria-label^="Destination"]')!==null),true,'version belongs on destination row');
  const versionBounds=await page.getByLabel('App version 0.2.1').boundingBox();
  assert.ok(versionBounds.x<30 && versionBounds.y>850,'version should sit at bottom left');
  const extensionButton=()=>page.getByRole('button',{name:/extension.*open setup/i});
  assert.equal(await extensionButton().locator('svg').evaluate(el=>el.classList.contains('text-caution')),true,'outdated connected extensions should show amber');
  assert.equal(await page.getByText('line open',{exact:true}).count(),0);
  assert.equal(await page.getByText('total',{exact:true}).count(),0);
  assert.equal(await page.evaluate(()=>window.extensionFixture().settings),false,'notification must work outside extension settings');
  await page.getByRole('button',{name:'Update extension',exact:true}).click();
  await page.waitForFunction(()=>window.extensionFixture().pending.length===1);
  assert.equal(await page.evaluate(()=>window.extensionFixture().history.some(t=>t.title==='Extension updated')),false,'staging cannot report success');
  await page.evaluate(()=>window.openSettings());
  await page.getByText('waiting for browser confirmation',{exact:true}).waitFor();
  await page.getByRole('button',{name:"Open Zen's Add-ons manager"}).click();
  assert.equal(await page.evaluate(()=>window.openedManager),'zen');
  if(screenshotDir){await mkdir(screenshotDir,{recursive:true});await page.screenshot({path:path.join(screenshotDir,'extension-update-desktop.png')});}
  await page.setViewportSize({width:390,height:844});
  assert.equal(await page.getByLabel('App version 0.2.1',{exact:true}).isVisible(),true);
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true,'extension settings overflow at mobile width');
  if(screenshotDir)await page.screenshot({path:path.join(screenshotDir,'extension-update-mobile.png')});
  await page.setViewportSize({width:1366,height:900});
  await page.reload();
  await page.waitForFunction(()=>window.extensionFixture()?.pending.length===1);
  assert.equal(await page.getByText('Extension update available',{exact:true}).count(),0,'a restart must not repeat the same version notice');
  await page.evaluate(async()=>{window.profiles[0].extensionVersion='0.1.41';window.profiles[0].lastSeenAtMs=Date.now()+10;await window.refreshExtensions();});
  assert.equal(await page.evaluate(()=>window.extensionFixture().pending.length),1,'one updated profile is not enough');
  await page.evaluate(async()=>{window.profiles[1].extensionVersion='0.1.41';window.profiles[1].lastSeenAtMs=Date.now()+10;await window.refreshExtensions();});
  await page.getByText('Extension updated',{exact:true}).waitFor();
  assert.equal(await page.evaluate(()=>window.extensionFixture().pending.length),0);
  await page.getByRole('button',{name:'Browser extension connected — open setup'}).waitFor();
  assert.equal(await extensionButton().locator('svg').evaluate(el=>el.classList.contains('text-clear')),true,'confirmed connection should show green');
  await page.evaluate(async()=>{window.profiles.forEach(p=>p.online=false);await window.refreshExtensions();});
  await page.getByRole('button',{name:'No browser extension connected — open setup'}).waitFor();
  assert.equal(await extensionButton().locator('svg').evaluate(el=>el.classList.contains('text-danger')),true,'offline profiles should show red');
  await extensionButton().click();
  assert.equal(await page.evaluate(()=>window.extensionFixture().settings),true);
  await page.evaluate(async()=>{window.profiles.forEach(p=>p.online=true);await window.refreshExtensions();});
  assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.extension-updates.pending.v1')).length),0);
  const calls=await page.evaluate(()=>window.nativeUpdates.length);
  await page.evaluate(()=>window.updateExtension('zen'));
  await page.getByText('Extension already up to date',{exact:true}).waitFor();
  assert.equal(await page.evaluate(()=>window.nativeUpdates.length),calls,'current versions must not trigger another installer');
  await page.evaluate(async()=>{window.fixtureVersion='0.1.42';window.failUpdate=true;await window.refreshExtensions();await window.updateExtension('zen').catch(()=>{});});
  await page.getByText('Extension update failed',{exact:true}).waitFor();
  await page.evaluate(async()=>{window.failUpdate=false;await window.updateExtension('zen');});
  await page.waitForFunction(()=>window.extensionFixture().pending.length===1);
  await page.clock.install();await page.clock.setSystemTime(Date.now()+100000);
  await page.evaluate(()=>window.refreshExtensions());
  await page.waitForFunction(()=>window.extensionFixture().pending[0]?.phase==='needs_attention');
  await page.getByText('Extension update needs confirmation',{exact:true}).waitFor();
  assert.deepEqual(errors,[]);
  console.log('Extension browser checks passed: global notification, explicit pending status, durable tracking, profile confirmation, success toast, deduplication, retry, timeout, Add-ons fallback, responsive layout.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
