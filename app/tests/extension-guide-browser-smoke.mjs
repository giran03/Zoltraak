import assert from 'node:assert/strict';
import path from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {readFile,readdir,mkdir} from 'node:fs/promises';
import {createServer} from 'node:http';
import {build} from '../node_modules/esbuild/lib/main.js';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const {chromium}=await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const bundle=await build({stdin:{resolveDir:root,loader:'tsx',contents:`
import React,{useState} from 'react';import{createRoot}from'react-dom/client';
import{ExtensionGuideModal}from'./src/components/ExtensionGuideModal';import{useExtensionUpdates}from'./src/lib/useExtensionUpdates';
window.calls=[];window.bundled='0.2.5';window.signed='0.2.5';window.failAction=false;
window.browsers=[{id:'zen',name:'Zen Browser',kind:'firefox',running:true,installed:true},{id:'chrome',name:'Google Chrome',kind:'chromium',running:true,installed:true},{id:'edge',name:'Microsoft Edge',kind:'chromium',running:false,installed:true},{id:'brave',name:'Brave Browser',kind:'chromium',running:false,installed:false}].map(b=>({...b,executablePath:'C:/Browsers/'+b.id}));
window.clients=[{clientId:'zen-profile',browserId:'zen',browserName:'Zen Browser',extensionVersion:'0.2.4',online:true,active:true,preferred:true,temporary:false}];
window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
window.calls.push({name,args});
if(name==='detect_browsers')return structuredClone(window.browsers);
if(name==='api_status')return {listening:true,port:1450,token:'fixture-token',detail:null};
if(name==='extension_source_state')return {revision:1,preferredClientId:null,activeClientId:'zen-profile',clients:window.clients.map(c=>({...c,lastSeenAtMs:Date.now()+1}))};
if(name==='get_extension_version')return window.bundled;
if(name==='get_firefox_extension_version')return window.signed;
if(name==='get_extension_directory')return 'C:/Extensions/'+args.browserId;
if(name==='open_extension_manager'||name==='reveal_folder')return;
if(['install_extension','reload_extension','relaunch_browser'].includes(name)){
if(window.failAction)throw Error('Fixture action failed');
const result={browserId:args.browserId,browserName:args.browserId,success:true,message:'Await browser confirmation for '+args.browserId,extensionPath:'C:/Extensions/'+args.browserId,nextStep:'browser_confirmation'};
return name==='relaunch_browser'?result:[result];}
throw Error('Unexpected command '+name);}};
navigator.clipboard.writeText=async text=>{window.copied=text};
function Fixture(){const[open,setOpen]=useState(true);const updates=useExtensionUpdates(()=>{},()=>{});window.refresh=()=>updates.refresh(true);return <ExtensionGuideModal open={open} onClose={()=>setOpen(false)} onOpenSettings={()=>window.settingsOpened=true} updates={updates}/>}
createRoot(document.getElementById('root')).render(<Fixture/>);
`},bundle:true,write:false,platform:'browser',format:'iife',jsx:'automatic'});
const assets=path.join(root,'dist/assets'),css=(await readdir(assets)).find(file=>file.endsWith('.css'));
const server=createServer(async(req,res)=>{
 if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
 else if(req.url.startsWith('/assets/')){res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))));}
 else res.end('<!doctype html><html lang="en" data-theme="dark"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');
});await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true});
try{
 const page=await browser.newPage({viewport:{width:1366,height:900}}),errors=[];page.on('pageerror',error=>errors.push(error.message));
 await page.goto('http://127.0.0.1:'+server.address().port);
 const controls=page.getByRole('region',{name:'Installed extension controls'});await controls.waitFor();
 assert.equal(await page.getByText('AUTOMATIC ENTERPRISE POLICY SETUP',{exact:true}).count(),0,'installed browser hides setup steps');
 await page.getByRole('button',{name:'Update extension',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='reload_extension').at(-1).args.browserId),'zen');
 assert.equal(await page.getByRole('button',{name:'Update extension',exact:true}).isDisabled(),true,'update waits for confirmation');
 await page.evaluate(()=>{window.clients[0].extensionVersion='0.2.5';return window.refresh()});
 await page.getByText('Connected profiles are up to date.',{exact:true}).waitFor();
 assert.equal(await page.getByRole('button',{name:'Update extension',exact:true}).count(),0);
 await page.getByRole('button',{name:'Restart guidance',exact:true}).click();
 assert.ok((await page.getByRole('alert').textContent()).includes('close all Zen Browser windows'));
 await page.getByRole('button',{name:'Repair / reinstall',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='install_extension').length),0,'repair requires confirmation');
 await page.getByRole('button',{name:'Cancel',exact:true}).click();
 await page.getByRole('button',{name:'Repair / reinstall',exact:true}).click();await page.getByRole('button',{name:'Confirm repair',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='install_extension').at(-1).args.browserId),'zen');
 assert.equal(await page.evaluate(()=>window.calls.some(c=>c.name==='get_extension_directory'&&c.args.browserId==='zen')),false,'Gecko does not prepare Chromium folders');
 await page.getByRole('button',{name:/^Google Chrome/}).click();
 await page.getByText('Configuration found; waiting for an extension connection.',{exact:true}).waitFor();
 assert.equal(await page.getByText('Not yet reported',{exact:true}).count(),1,'configured install does not invent a live version');
 await page.getByRole('button',{name:'Open extension manager',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='open_extension_manager').at(-1).args.browserId),'chrome');
 await page.getByRole('button',{name:'Restart browser',exact:true}).click();await page.getByRole('button',{name:'Confirm restart',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='relaunch_browser').at(-1).args.browserId),'chrome');
 await page.getByText('Extension files',{exact:true}).click();
 await page.getByRole('button',{name:'Copy folder path',exact:true}).click();assert.equal(await page.evaluate(()=>window.copied),'C:/Extensions/chrome');
 await page.getByRole('button',{name:'Open extension folder',exact:true}).click();assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='reveal_folder').at(-1).args.path),'C:/Extensions/chrome');
 await page.getByRole('button',{name:/^Brave Browser/}).click();
 await page.getByText('CHROMIUM UNPACKED LOADING',{exact:true}).waitFor();assert.equal(await controls.count(),0,'uninstalled browser keeps setup instructions');
 await page.getByRole('button',{name:/^Zen Browser/}).click();
 await page.evaluate(()=>{window.signed=null;window.bundled='0.2.6';return window.refresh()});
 await page.getByRole('button',{name:'Manage signed package',exact:true}).waitFor();
 assert.equal(await page.getByRole('button',{name:'Update extension',exact:true}).count(),0,'missing signed package cannot trigger an update');
 await page.getByRole('button',{name:'Manage signed package',exact:true}).click();assert.equal(await page.evaluate(()=>window.settingsOpened),true);
 await page.evaluate(()=>window.failAction=true);
 await page.getByRole('button',{name:'Reload extension',exact:true}).click();await page.getByRole('alert').filter({hasText:'Fixture action failed'}).waitFor();
 const shots=path.join(root,'../scratch/extension-guide');await mkdir(shots,{recursive:true});
 for(const width of [1366,390]){await page.setViewportSize({width,height:900});await page.screenshot({path:path.join(shots,'management-'+width+'.png')});assert.equal(await page.getByRole('dialog').evaluate(el=>el.scrollWidth<=el.clientWidth),true,'management fits narrow screens');}
 assert.deepEqual(errors,[]);
 console.log('Passed: installed/setup switching, live vs configured state, update confirmation, selected-browser commands, repair/restart confirmation, signed-package gating, errors and responsive management layout.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
