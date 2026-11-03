// Run with PLAYWRIGHT_MODULE pointing to an existing playwright-core/index.mjs.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { mkdir, readFile, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = process.env.DISCORD_SCREENSHOT_DIR || path.join(root, 'test-results', 'discord-send');
await mkdir(output, { recursive: true });
const fixture = `
import React from 'react';
import {createRoot} from 'react-dom/client';
import {DiscordSendDialog} from './src/components/DiscordSendDialog';
import {DiscordSettings} from './src/components/DiscordSettings';
const query = new URLSearchParams(location.search);
document.documentElement.dataset.theme = query.get('theme') || 'dark';
const settings = {enabled:true,mode:query.get('mode')||'webhook',autoSend:false,guildId:'g',channelId:'c',guildName:'Fixture server',channelName:'downloads',botUsername:'Server webhook',uploadLimitBytes:null,botUploadLimitBytes:20000000,compressToFit:true,compressMaxHeight:720,messageTemplate:'',includeSourceUrl:false,notifyOnDelivery:true,webhookUsername:'My sender',webhookAvatarUrl:'https://example.com/avatar.png',discordUserId:'123456789012345678',pingSender:false};
const status = { ...settings, destinationRef:'webhook:123',hasCredential:true,credentialHint:'1234',connected:true,uploadLimitBytes:20000000,uploadLimitLabel:'19.1 MiB',uploadLimitDetected:false,lastError:null};
window.toasts=[];window.failDelete=false;window.deliveries=query.has("deletion")?[1,2].map(n=>({id:"delivery-"+n,source:"manual",sourceId:null,path:"/fixture/file.txt",filename:"file-"+n+".txt",bytes:10,state:"sent",reason:null,messageUrl:"https://discord.com/channels/3/2/1",sentBytes:10,attempts:0,createdAt:new Date().toISOString(),updatedAt:new Date().toISOString()})):[];
window.calls=[]; window.settings=settings; window.discordStatus=status; window.failSend=false;
window.botConfigured=true;window.failModeSwitch=false;
window.failProfile=false;
window.__TAURI_INTERNALS__ = {invoke: async (name,args) => {
 window.calls.push({name,args});
 if(name==='discord_settings')return structuredClone(settings);
 if(name==='discord_status')return structuredClone(status);
 if(name==='discord_deliveries')return structuredClone(window.deliveries);
 if(name==='open_discord_message'){if(window.failOpen)throw Error('Could not open the Discord message.');return;}
 if(name==='discord_delete_message'){await new Promise(r=>setTimeout(r,300));if(window.failDelete)throw Error('Original webhook token is no longer valid');window.deliveries.forEach(d=>{d.state='deleted';d.reason='Message deleted from Discord';});return true;}
 if(name==='discord_guilds')return [{id:'g',name:'Fixture server'}];
 if(name==='discord_channels')return [{id:'c',name:'downloads',kind:0,position:0}];
 if(name==='update_discord_settings'){
  if(window.failModeSwitch&&args.settings.mode!==settings.mode)throw Error('Fixture method switch failure');
  Object.assign(settings,args.settings);
  Object.assign(status,{mode:settings.mode,hasCredential:settings.mode==='webhook'||window.botConfigured,connected:settings.mode==='webhook'||window.botConfigured,destinationRef:settings.mode==='bot'?(window.botConfigured?'bot:g:c':null):'webhook:123'});
  return structuredClone(settings);
 }
 if(name==='discord_bot_profile'){if(window.failProfile)throw Error('Fixture profile unavailable');return args.request.scope==='server'?{nick:'Fixture server bot',avatar:null,banner:null}:{id:'42',username:'Fixture bot',avatar:'data:image/svg+xml;base64,'+btoa('<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="#5865f2"/></svg>'),banner:'data:image/svg+xml;base64,'+btoa('<svg xmlns="http://www.w3.org/2000/svg" width="400" height="160"><rect width="400" height="160" fill="#597869"/></svg>')};}
 if(name==='discord_send'){if(window.failSend)throw Error('Fixture send failure'); await new Promise(r=>setTimeout(r,80));return args.ids.length;}
 throw Error('Unexpected command '+name);
}};
const item={id:'a',state:'finished',filename:'A very long original download filename that should remain legible in the attachment list.jpg',url:'https://example.com/original',totalBytes:24000000,downloadedBytes:24000000,directory:'',connections:1,bytesPerSecond:0,etaSeconds:null,priority:0,error:null,createdAt:''};
function App(){const [open,setOpen]=React.useState(true);return query.has('settings')?React.createElement(DiscordSettings,{addToast:t=>window.toasts.push(t)}):React.createElement(React.Fragment,null,React.createElement('button',{id:'outside'},'Outside'),open&&React.createElement(DiscordSendDialog,{items:[item,{...item,id:'b',state:'downloading'}],onClose:()=>{window.dialogClosed=true;setOpen(false)},onSettings:()=>{window.openedSettings=true;setOpen(false)},onQueued:count=>{window.queued=count;setOpen(false)}}));}
createRoot(document.getElementById('root')).render(React.createElement(App));`;
const bundle = await build({stdin:{contents:fixture,resolveDir:root,sourcefile:'fixture.tsx',loader:'tsx'},bundle:true,write:false,format:'iife',platform:'browser',jsx:'automatic'});
const assets = path.join(root,'dist','assets');
const css = await readFile(path.join(assets,(await readdir(assets)).find(name=>name.endsWith('.css'))));
const server=createServer(async(req,res)=>{
 if(req.url==='/avatar.png'){res.setHeader('Content-Type','image/png');res.end(Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aFzwAAAAASUVORK5CYII=','base64'));}
 else if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
 else if(req.url==='/fixture.css'){res.setHeader('Content-Type','text/css');res.end(css);}
 else if(req.url.startsWith('/assets/')){try{res.end(await readFile(path.join(assets,path.basename(req.url))));}catch{res.writeHead(404);res.end();}}
 else {res.setHeader('Content-Type','text/html');res.end('<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/fixture.css"><div id="root"></div><script src="/fixture.js"></script>');}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true});
try {
 for(const theme of ['light','dark'])for(const width of [1100,390]){
  const page=await browser.newPage({viewport:{width,height:800},reducedMotion:width===390?'reduce':'no-preference'});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:'+server.address().port+'/?theme='+theme);
  await page.getByText('1 unfinished entries excluded.').waitFor();
  const send=page.getByRole('button',{name:'Queue for Discord',exact:true});
  await send.waitFor();assert.equal(await send.isEnabled(),true);
  const message=page.getByLabel('Message (optional)'); await message.fill('Literal {filename} @everyone');
  await page.locator('[data-message-preview]').filter({hasText:'Literal {filename} @everyone'}).waitFor();
  const method=page.getByRole('group',{name:'Delivery method',exact:true});
  await method.getByRole('button',{name:'Discord bot',exact:true}).click();
  await page.waitForFunction(()=>window.settings.mode==='bot');
  await page.getByRole('img',{name:'Fixture server bot profile banner',exact:true}).waitFor();
  assert.equal(await message.inputValue(),'Literal {filename} @everyone','method switching preserves the draft');
  assert.equal(await method.getByRole('button',{name:'Discord bot',exact:true}).getAttribute('aria-pressed'),'true');
  await method.getByRole('button',{name:'Webhook URL',exact:true}).click();
  await page.waitForFunction(()=>window.settings.mode==='webhook');
  await page.evaluate(()=>window.botConfigured=false);
  await method.getByRole('button',{name:'Discord bot',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Not configured'}).waitFor();
  assert.equal(await send.isDisabled(),true,'unconfigured methods cannot queue');
  await method.getByRole('button',{name:'Webhook URL',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Configured'}).waitFor();
  await page.evaluate(()=>window.botConfigured=true);
  if(theme==='light'&&width===1100){
    await page.evaluate(()=>window.failProfile=true);await method.getByRole('button',{name:'Discord bot',exact:true}).click();
    await page.getByText('Bot profile unavailable. Delivery configuration is checked separately.',{exact:true}).waitFor();
    assert.equal(await send.isEnabled(),true,'profile lookup failure does not block a configured destination');
    await method.getByRole('button',{name:'Webhook URL',exact:true}).click();await page.getByRole('status').filter({hasText:'Configured'}).waitFor();
    await page.evaluate(()=>window.failProfile=false);
    await page.evaluate(()=>window.failModeSwitch=true);await method.getByRole('button',{name:'Discord bot',exact:true}).click();
    await page.getByRole('alert').filter({hasText:'Fixture method switch failure'}).waitFor();
    assert.equal(await message.inputValue(),'Literal {filename} @everyone');assert.equal(await send.isDisabled(),true);
    await page.evaluate(()=>window.failModeSwitch=false);await method.getByRole('button',{name:'Discord bot',exact:true}).click();
    await page.getByRole('img',{name:'Fixture server bot profile banner',exact:true}).waitFor();
    await page.screenshot({path:path.join(output,'dialog-bot-profile.png')});
    await method.getByRole('button',{name:'Webhook URL',exact:true}).click();await page.getByRole('status').filter({hasText:'Configured'}).waitFor();
  }
  await page.getByRole('switch',{name:'Ping me',exact:true}).click();
  await page.getByRole('switch',{name:'Include source links',exact:true}).click();
  await page.waitForTimeout(250);
  await page.screenshot({path:path.join(output,'dialog-'+theme+'-'+width+'.png')});
  const preview=page.getByRole('complementary',{name:'Discord message preview'});
  const layout=await preview.evaluate(el=>{const a=el.getBoundingClientRect(),b=el.previousElementSibling.getBoundingClientRect();return {beside:a.left>=b.right,below:a.top>=b.bottom};});
  assert.equal(width===390?layout.below:layout.beside,true,'preview switches between right column and stacked layout');
  assert.equal(await page.locator('dialog').evaluate(el=>el.scrollWidth<=el.clientWidth),true,'preview does not cause horizontal overflow');
  if(width===390){await preview.scrollIntoViewIfNeeded();await page.screenshot({path:path.join(output,'preview-'+theme+'-'+width+'.png')});}
  const bounds=await page.locator('dialog').boundingBox(); assert.ok(bounds.x>=0 && bounds.x+bounds.width<=width+1);
  await page.evaluate(()=>{window.discordStatus.destinationRef='webhook:456';});
  await send.click(); await page.getByRole('alert').waitFor();
  assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.name==='discord_send').length),0);
  await page.evaluate(()=>{window.failSend=true;}); await send.click();
  await page.getByText('Fixture send failure',{exact:true}).waitFor();
  assert.equal(await message.inputValue(),'Literal {filename} @everyone');
  await page.evaluate(()=>{window.failSend=false;}); await send.dblclick();
  await page.waitForFunction(()=>window.queued===1);
  const calls=await page.evaluate(()=>window.calls.filter(c=>c.name==='discord_send'));
  assert.equal(calls.length,2); // one rejected request, one successful submission
  assert.deepEqual(calls[1].args.options,{caption:'Literal {filename} @everyone',pingSender:true,includeSourceUrl:true,compressToFit:true,expectedDestination:'webhook:456'});
  assert.equal(errors.length,0,errors.join('\n'));await page.close();
 }
 for(const mode of ['webhook','bot']){
  const page=await browser.newPage({viewport:{width:1100,height:900}});
  await page.goto('http://127.0.0.1:'+server.address().port+'/?settings&mode='+mode);
  await page.getByLabel('Upload limit (megabytes)',{exact:true}).waitFor();
  assert.equal(await page.getByLabel('Upload limit (megabytes)',{exact:true}).inputValue(),'20');
  assert.equal(await page.getByLabel('Sender name',{exact:true}).count(),mode==='webhook'?1:0);
  await page.getByLabel('Ping message template',{exact:true}).fill('sent by {discord user id}');
  await page.getByRole('button',{name:'Save sender',exact:true}).click();
  await page.waitForFunction(()=>window.settings.pingMessageTemplate==='sent by {discord user id}');
  if(mode==='webhook'){
    assert.equal(await page.getByLabel('Select avatar image',{exact:true}).count(),0,'standalone builds have no hosted avatar upload');
    assert.equal(await page.getByLabel('Avatar image URL',{exact:true}).inputValue(),'https://example.com/avatar.png');
  }
  await page.getByLabel('Discord user ID',{exact:true}).fill('0');
  assert.equal(await page.getByRole('button',{name:'Save sender',exact:true}).isDisabled(),true);
  await page.getByLabel('Discord user ID',{exact:true}).fill('');
  await page.getByRole('button',{name:'Save sender',exact:true}).click();
  await page.waitForFunction(()=>window.settings.discordUserId==='');
  assert.equal(await page.getByRole('switch',{name:'Ping me when sending',exact:true}).isDisabled(),true);
  await page.getByLabel('Upload limit (megabytes)',{exact:true}).fill('30');
  await page.getByRole('button',{name:'Save limit',exact:true}).click();
  await page.waitForFunction(mode=>mode==='bot'?window.settings.botUploadLimitBytes===30000000:window.settings.uploadLimitBytes===30000000,mode);
  await page.waitForTimeout(250);
  await page.screenshot({path:path.join(output,'settings-'+mode+'.png'),fullPage:true});
  if(mode==='webhook'){
    for(const theme of ['light','dark'])for(const width of [1100,390]){
      await page.setViewportSize({width,height:900});await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);
      await page.screenshot({path:path.join(output,'avatar-settings-'+theme+'-'+width+'.png'),fullPage:true});
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true,'avatar settings fit the narrow viewport');
    }
  }
  await page.close();
 }
 const deletionPage=await browser.newPage({viewport:{width:1100,height:900}});
 await deletionPage.goto('http://127.0.0.1:'+server.address().port+'/?settings&deletion');
 await deletionPage.getByRole('button',{name:'Open',exact:true}).first().click();
 assert.deepEqual(await deletionPage.evaluate(()=>window.calls.find(c=>c.name==='open_discord_message').args),{url:'https://discord.com/channels/3/2/1'});
 await deletionPage.evaluate(()=>window.failOpen=true);
 await deletionPage.getByRole('button',{name:'Open',exact:true}).first().click();
 await deletionPage.getByText('Could not open the Discord message.',{exact:true}).waitFor();
 await deletionPage.evaluate(()=>window.failOpen=false);
 await deletionPage.getByRole('button',{name:'Open',exact:true}).first().click();
 await deletionPage.getByRole('button',{name:'Delete',exact:true}).first().click();
 await deletionPage.getByRole('button',{name:'Cancel',exact:true}).click();assert.equal(await deletionPage.evaluate(()=>window.calls.filter(c=>c.name==='discord_delete_message').length),0);
 await deletionPage.evaluate(()=>window.failDelete=true);
 await deletionPage.getByRole('button',{name:'Delete',exact:true}).first().click();
 await deletionPage.getByRole('button',{name:'Delete message',exact:true}).click();
 await deletionPage.getByText('Deleting message from Discord...',{exact:true}).first().waitFor();
 assert.equal(await deletionPage.getByRole('button',{name:'Deleting...',exact:true}).first().isDisabled(),true);
 await deletionPage.getByText('Could not delete message: Original webhook token is no longer valid',{exact:true}).first().waitFor();
 assert.equal(await deletionPage.evaluate(()=>window.toasts.at(-1).type),'error');assert.equal(await deletionPage.evaluate(()=>window.deliveries[0].state),'sent');
 await deletionPage.evaluate(()=>window.failDelete=false);
 await deletionPage.getByRole('button',{name:'Delete',exact:true}).first().click();await deletionPage.getByRole('button',{name:'Delete message',exact:true}).click();
 await deletionPage.getByText('Message and its attachments deleted from Discord. Downloaded files were preserved.',{exact:true}).first().waitFor();
 await deletionPage.waitForFunction(()=>document.querySelectorAll('button').length && !Array.from(document.querySelectorAll('button')).some(b=>b.textContent==='Delete'));
 assert.equal(await deletionPage.evaluate(()=>window.toasts.at(-1).type),'success');assert.equal(await deletionPage.evaluate(()=>window.calls.filter(c=>c.name==='discord_delete_message').length),2);
 for(const theme of ['light','dark'])for(const width of [1100,390]){await deletionPage.setViewportSize({width,height:900});await deletionPage.evaluate(t=>document.documentElement.dataset.theme=t,theme);assert.ok(await deletionPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await deletionPage.screenshot({path:path.join(output,'deleted-message-'+theme+'-'+width+'.png'),fullPage:true});}
 await deletionPage.close();
 const page=await browser.newPage();await page.goto('http://127.0.0.1:'+server.address().port+'/');
 await page.getByRole('button',{name:'Queue for Discord',exact:true}).waitFor();await page.keyboard.press('Escape');await page.waitForFunction(()=>window.dialogClosed===true);await page.close();
 console.log('Discord dialog checks passed; screenshots: '+output);
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
