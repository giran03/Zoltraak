import assert from 'node:assert/strict';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {readFile,readdir,mkdir} from 'node:fs/promises';
import {createServer} from 'node:http';
import {build} from '../node_modules/esbuild/lib/main.js';
const {chromium}=await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const app=path.resolve(import.meta.dirname,'..');
const bundle=await build({stdin:{resolveDir:app,loader:'tsx',contents:`
import React,{useState} from 'react';
import {createRoot} from 'react-dom/client';
import {DiscordSenderSettings} from './src/components/DiscordSenderSettings';
window.avatarCommits=[];
function Fixture(){
 const [settings,setSettings]=useState({mode:'webhook',webhookAvatarUrl:null,discordUserId:null,pingSender:false});
 const [key,setKey]=useState(0),[disabled,setDisabled]=useState(false);
 window.remountSender=()=>setKey(k=>k+1);
 window.disableSender=setDisabled;
 window.setSenderMode=mode=>setSettings(s=>({...s,mode}));
 return <main data-sender-instance={key} className="mx-auto max-w-3xl p-6"><DiscordSenderSettings key={key} settings={settings} disabled={disabled} commit={async patch=>{
  window.avatarCommits.push(patch);
  if(window.rejectAvatarCommit)return false;
  setSettings(s=>({...s,...patch}));return true;
 }}/></main>;
}
createRoot(document.getElementById('root')).render(<Fixture/>);
`},bundle:true,write:false,format:'iife',platform:'browser',jsx:'automatic'});
const assets=path.join(app,'dist/assets'),css=(await readdir(assets)).find(n=>n.endsWith('.css'));
const server=createServer(async(req,res)=>{
 if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
 else if(req.url.startsWith('/assets/')){res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))));}
 else{res.setHeader('Content-Type','text/html');res.end(`<!doctype html><html lang="en" data-theme="dark"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/${css}"><div id="root"></div><script src="/fixture.js"></script></html>`);}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
try{
 const page=await browser.newPage({viewport:{width:1280,height:900}}),errors=[];
 page.on('pageerror',e=>errors.push(e.message));
 await page.goto('http://127.0.0.1:'+server.address().port);
 const mode=page.getByLabel('Webhook avatar',{exact:true});
 const url=page.getByLabel('Avatar image URL',{exact:true});
 const save=page.getByRole('button',{name:'Save sender',exact:true});
 const remount=async()=>{
  const previous=await page.locator('main').getAttribute('data-sender-instance');
  await page.evaluate(()=>window.remountSender());
  await page.waitForFunction(previous=>document.querySelector('main').dataset.senderInstance!==previous,previous);
 };
 const assertAligned=async()=>{
  const nameBox=await page.getByLabel('Sender name',{exact:true}).boundingBox(),avatarBox=await mode.boundingBox();
  assert.ok(Math.abs(nameBox.y-avatarBox.y)<1,'sender name and avatar controls share a baseline');
  assert.ok(Math.abs(nameBox.height-avatarBox.height)<1,'sender name and avatar controls share a height');
  const idBox=await page.getByLabel('Discord user ID',{exact:true}).boundingBox(),pingBox=await page.getByLabel('Ping message template',{exact:true}).boundingBox();
  assert.ok(Math.abs(idBox.y-pingBox.y)<1,'user ID and ping-template controls share a baseline');
 };
 assert.equal(await mode.inputValue(),'default');assert.equal(await url.count(),0);
 await assertAligned();
 assert.equal(await page.getByRole('dialog').count(),0,'guide starts hidden');
 await mode.selectOption('url');assert.equal(await url.inputValue(),'');assert.ok(await save.isDisabled());await assertAligned();
 await url.fill('file:///avatar.png');assert.ok(await save.isDisabled());
 const image='https://cdn.discordapp.com/attachments/123/456/avatar.png?ex=1234&is=abcd&hm=signature';
 await url.fill(image);assert.ok(await save.isEnabled());
 await page.evaluate(()=>window.rejectAvatarCommit=true);await save.click();
 assert.equal(await url.inputValue(),image,'failed saves retain the draft');assert.ok(await save.isEnabled());
 await page.evaluate(()=>window.rejectAvatarCommit=false);await save.click();
 await remount();assert.equal(await mode.inputValue(),'url');assert.equal(await url.inputValue(),image);
 await mode.selectOption('default');await url.waitFor({state:'detached'});await save.click();
 const cleared=await page.evaluate(()=>window.avatarCommits.at(-1));
 assert.equal(cleared.webhookAvatarUrl,null);assert.equal(cleared.webhookAvatarAssetId,null);
 await remount();await url.waitFor({state:'detached'});assert.equal(await mode.inputValue(),'default');
 const info=page.getByRole('button',{name:'About webhook avatars',exact:true});
 const shots=path.join(app,'../scratch/discord-avatar');await mkdir(shots,{recursive:true});
 for(const [theme,width] of [['dark',1280],['light',390]]){
  await page.setViewportSize({width,height:800});await page.evaluate(t=>document.documentElement.setAttribute('data-theme',t),theme);
  await info.focus();await page.keyboard.press('Enter');
  const guide=page.getByRole('dialog',{name:'Webhook avatar guide'});await guide.waitFor();
  assert.ok((await guide.innerText()).includes('Copy Image Address'));
  assert.ok((await guide.innerText()).includes('rather than the message link'));
  assert.ok(await guide.evaluate(el=>el.scrollWidth<=el.clientWidth));
  await guide.screenshot({path:path.join(shots,theme+'-guide.png')});
  await page.keyboard.press('Escape');await guide.waitFor({state:'detached'});assert.ok(await info.evaluate(el=>el===document.activeElement));
  await info.click();await guide.getByRole('button',{name:'Got it',exact:true}).click();await guide.waitFor({state:'detached'});
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
  await page.screenshot({path:path.join(shots,theme+'-sender.png')});
 }
 await page.evaluate(()=>window.disableSender(true));await page.waitForFunction(()=>document.querySelector('select[aria-label="Webhook avatar"]').disabled);assert.ok(await save.isDisabled());
 await page.evaluate(()=>window.setSenderMode('bot'));await mode.waitFor({state:'detached'});assert.equal(await info.count(),0);
 assert.deepEqual(errors,[]);
 console.log('Passed: avatar modes, URL validation, saved/reopened preferences, failed saves, default reset, keyboard help, themes, mobile layout and bot separation.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
