// Run with PLAYWRIGHT_MODULE pointing at an existing playwright-core/index.mjs.
import assert from 'node:assert/strict';
import path from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {readFile,readdir,mkdir} from 'node:fs/promises';
import {createServer} from 'node:http';
import {build} from '../node_modules/esbuild/lib/main.js';
const {chromium}=await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const output=path.resolve(root,'../scratch/download-entry-actions');await mkdir(output,{recursive:true});
const bundle=await build({stdin:{resolveDir:root,loader:'tsx',contents:`
import React,{useState} from 'react';import{createRoot}from'react-dom/client';
import{DownloadsView}from'./src/components/DownloadsView';import{DiscordSendDialog}from'./src/components/DiscordSendDialog';import{videoJobToSnapshot}from'./src/lib/snapshots';
const jobs=['finished','paused','downloading','failed'].map(state=>({id:state,title:'Fixture '+state,webpageUrl:'https://media.test/'+state,state,downloadedBytes:100,totalBytes:200,bytesPerSecond:0,etaSeconds:null,error:null,path:state==='finished'?'C:/Downloads/finished.webm':null,createdAt:'2026-10-01T00:00:00Z'}));
window.calls=[];window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
if(name==='download_file_status')return {path:'C:/Downloads/finished.webm',status:'present',detail:null};
if(name==='download_thumbnail')return null;
if(name==='discord_settings')return {mode:'webhook',enabled:true,compressToFit:true};
if(name==='discord_status')return {connected:true,uploadLimitBytes:20000000,channelName:'downloads'};
throw Error('Unexpected command '+name);}};
navigator.clipboard.writeText=async value=>{window.copied=value;};
function Fixture(){const[send,setSend]=useState(null),[composing,setComposing]=useState(false);const record=(name,...args)=>window.calls.push({name,args});
const actions={toggle:item=>record('toggle',item.id),pause:async item=>record('pause',item.id),resume:async item=>record('resume',item.id),remove:(items,deleteFile)=>record('remove',items.map(i=>i.id),deleteFile),reveal:id=>record('reveal',id),open:id=>record('open',id),sendToDiscord:ids=>{record('discord',ids);setSend(ids)},convert:items=>record('convert',items.map(i=>i.path)),remux:(ids,container)=>record('remux',ids,container)};
return <><DownloadsView active items={[]} videoJobs={jobs} pendingRemoval={new Set()} status="live" error={null} onClearError={()=>{}} onAdd={async()=>true} actions={actions} destination="C:/Downloads" composing={composing} onComposingChange={setComposing}/>{send&&<DiscordSendDialog items={jobs.filter(j=>send.includes(j.id)).map(videoJobToSnapshot)} onClose={()=>setSend(null)} onSettings={()=>setSend(null)} onQueued={()=>setSend(null)}/>}</>}
createRoot(document.getElementById('root')).render(<Fixture/>);
`},bundle:true,write:false,format:'iife',platform:'browser',jsx:'automatic'});
const assets=path.join(root,'dist','assets'),css=(await readdir(assets)).find(n=>n.endsWith('.css'));
const server=createServer(async(req,res)=>{
if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text)}
else if(req.url.startsWith('/assets/')){res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))))}
else res.end('<html><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');
});await new Promise(r=>server.listen(0,'127.0.0.1',r));
const browser=await chromium.launch({headless:true,channel:process.env.CHROME_CHANNEL});
try{
const page=await browser.newPage({viewport:{width:1366,height:1000}}),errors=[];page.on('pageerror',e=>errors.push(e.message));await page.goto('http://127.0.0.1:'+server.address().port);
const finished=page.locator('[data-row-id="finished"]');
const folderControl=finished.getByRole('button',{name:'Show Fixture finished in folder',exact:true});
const discordControl=finished.getByRole('button',{name:'Send Fixture finished to Discord',exact:true});
await discordControl.hover();
assert.equal(await discordControl.evaluate(element=>getComputedStyle(element).color),'rgb(88, 101, 242)','Discord uses blurple');
assert.ok(await discordControl.evaluate(element=>getComputedStyle(element,'::before').clipPath.startsWith('polygon(')),'entry hover background uses the shared squircle');
await folderControl.hover();
await page.waitForFunction(()=>getComputedStyle(document.querySelector('[aria-label="Show Fixture finished in folder"] > .entry-icon-glyph')).transform!=='none');
assert.equal((await folderControl.boundingBox()).width,28,'download controls keep their compact hit targets');
await finished.locator('.download-row').screenshot({path:path.join(output,'controls-hover.png')});
await page.emulateMedia({reducedMotion:'reduce'});
assert.equal(await folderControl.locator('.entry-icon-glyph').evaluate(element=>getComputedStyle(element).transform),'none');
await page.emulateMedia({reducedMotion:'no-preference'});
const disabledDiscord=page.locator('[data-row-id="paused"]').getByRole('button',{name:'Send Fixture paused to Discord',exact:true});
await disabledDiscord.hover();
assert.equal(await disabledDiscord.evaluate(element=>getComputedStyle(element,'::before').opacity),'0','disabled actions do not animate');
await page.mouse.move(0,0);
// Grouping must preserve sorting, selection safety and the existing entry actions.
const grouping=page.getByRole('combobox',{name:'Group downloads',exact:true});
await grouping.selectOption('type');
assert.deepEqual(await page.locator('h3 button').allTextContents(),['Video3 entries','WEBM1 entry']);
await page.getByRole('combobox',{name:'Sort downloads',exact:true}).selectOption('name');
await page.getByRole('button',{name:'Reverse sort direction',exact:true}).click();
assert.deepEqual(await page.getByRole('region',{name:'Video downloads',exact:true}).locator('[data-row-id]').evaluateAll(rows=>rows.map(row=>row.dataset.rowId)),['downloading','failed','paused']);
await grouping.selectOption('status');
assert.deepEqual(await page.locator('h3 button').allTextContents(),['Downloading1 entry','Paused1 entry','Failed1 entry','Finished1 entry']);
await page.getByRole('button',{name:/^Paused\s+1 entry$/}).click();
assert.equal(await page.locator('[data-row-id="paused"]').count(),0,'collapsed entries are removed from interaction');
await page.locator('[data-row-id="downloading"]').getByRole('checkbox').click();
await finished.getByRole('checkbox').click({modifiers:['Shift']});
assert.equal(await page.getByRole('checkbox',{checked:true}).count(),3,'shift selection follows grouped visible order');
await page.getByRole('button',{name:'Remove',exact:true}).click();
assert.deepEqual(await page.evaluate(()=>window.calls.at(-1).args[0]),['downloading','failed','finished'],'bulk actions exclude collapsed entries');
await page.getByRole('button',{name:'Collapse all groups',exact:true}).click();
assert.equal(await page.locator('[data-row-id]').count(),0);
assert.equal(await page.getByRole('button',{name:'Expand all groups',exact:true}).isEnabled(),true);
await page.getByRole('button',{name:'Expand all groups',exact:true}).click();
assert.equal(await page.locator('[data-row-id]').count(),4);
await page.getByRole('button',{name:/^Paused\s+1 entry$/}).click();
await page.evaluate(()=>document.activeElement.blur());await page.keyboard.press('Control+a');
assert.equal(await page.getByRole('checkbox',{checked:true}).count(),3,'select-all excludes collapsed rows');
await page.getByRole('button',{name:'Clear',exact:true}).click();
await page.locator('[data-row-id="downloading"]').getByRole('checkbox').click();
await page.keyboard.press('ArrowDown');
assert.equal(await page.locator('[data-row-id="failed"]').getByRole('checkbox').getAttribute('aria-checked'),'true','keyboard skips collapsed groups');
await page.getByRole('button',{name:'Clear',exact:true}).click();
await grouping.selectOption('source');
assert.deepEqual(await page.locator('h3 button').allTextContents(),['media.test4 entries']);
await page.getByRole('textbox',{name:'Search downloads',exact:true}).fill('finished');
assert.deepEqual(await page.locator('h3 button').allTextContents(),['media.test1 entry'],'group counts reflect search');
await page.getByRole('textbox',{name:'Search downloads',exact:true}).fill('');
await grouping.selectOption('folder');
assert.deepEqual(await page.locator('h3 button').allTextContents(),['C:/Downloads1 entry','No destination3 entries']);
await page.reload();await finished.waitFor();
assert.equal(await grouping.inputValue(),'folder','grouping persists across relaunch');
await grouping.selectOption('status');
for(const width of [1366,390]){
  await page.setViewportSize({width,height:1000});
  if(width===390)await page.evaluate(()=>document.documentElement.setAttribute('data-theme','dark'));
  await page.screenshot({path:path.join(output,'grouped-'+width+'.png')});
  const toolbar=grouping.locator('xpath=../..');
  assert.equal(await toolbar.evaluate(el=>el.scrollWidth<=el.clientWidth),true,'grouping toolbar wraps on narrow screens');
}
await page.setViewportSize({width:1366,height:1000});
await page.evaluate(()=>document.documentElement.setAttribute('data-theme','light'));
await page.getByText('Arrange columns',{exact:true}).click();
await page.getByRole('button',{name:'Reset view',exact:true}).click();
assert.equal(await grouping.inputValue(),'none');
assert.equal(await page.locator('h3 button').count(),0,'reset removes grouping');
await page.getByText('Arrange columns',{exact:true}).click();
await page.evaluate(()=>window.calls=[]);
await finished.getByRole('button',{name:'Send Fixture finished to Discord',exact:true}).click();
await page.getByRole('dialog',{name:'Send to Discord',exact:true}).waitFor();assert.equal(await finished.getAttribute('data-open'),'false','row Discord button does not expand details');
await page.getByRole('button',{name:'Close send dialog'}).click();
for(const state of ['finished','paused','downloading','failed']){
const row=page.locator('[data-row-id="'+state+'"]');
assert.equal(await row.getByRole('button',{name:'Send Fixture '+state+' to Discord',exact:true}).isDisabled(),state!=='finished');
await row.getByRole('button',{name:'Open Fixture '+state,exact:true}).click();
const group=row.getByRole('group',{name:'Actions for Fixture '+state,exact:true});await group.waitFor();
await row.locator('[data-download-name]').click({button:'right'});
const menu=page.getByRole('menu');await menu.waitFor();
const menuLabels=await menu.getByRole('menuitem').allTextContents();const actionLabels=await group.getByRole('button').allTextContents();
assert.deepEqual(actionLabels.map(s=>s.trim()),menuLabels.map(s=>s.trim()),'detail buttons match every right-click action for '+state);
await page.keyboard.press('Escape');await menu.waitFor({state:'hidden'});
if(await row.getAttribute('data-open')!=='true')await row.getByRole('button',{name:'Open Fixture '+state,exact:true}).click();
assert.equal(await group.getByRole('button',{name:'Send to Discord',exact:true}).isDisabled(),state!=='finished');
if(state==='finished'){
await group.getByRole('button',{name:'Send to Discord',exact:true}).click();await page.getByRole('dialog',{name:'Send to Discord',exact:true}).waitFor();await page.getByRole('button',{name:'Close send dialog'}).click();
for(const label of ['Open file','Show in folder','Convert…','Quick convert to MP4','Remove from list','Delete entry and file'])await group.getByRole('button',{name:label,exact:true}).click();
await group.getByRole('button',{name:'Copy download link',exact:true}).click();assert.equal(await page.evaluate(()=>window.copied),'https://media.test/finished');
await group.getByRole('button',{name:'Copy file path',exact:true}).click();assert.equal(await page.evaluate(()=>window.copied),'C:/Downloads/finished.webm');
for(const width of [1366,390]){await page.setViewportSize({width,height:1000});await group.scrollIntoViewIfNeeded();await page.screenshot({path:path.join(output,'actions-'+width+'.png')});assert.equal(await group.evaluate(el=>el.scrollWidth<=el.clientWidth),true,'expanded actions fit their available width');}await page.setViewportSize({width:1366,height:1000});
}else await group.getByRole('button',{name:state==='downloading'?'Pause':state==='failed'?'Retry download':'Resume',exact:true}).click();
}
const calls=await page.evaluate(()=>window.calls);
assert.ok(calls.some(c=>c.name==='remove'&&c.args[1]===false));assert.ok(calls.some(c=>c.name==='remove'&&c.args[1]===true));assert.ok(calls.some(c=>c.name==='remux'));assert.ok(calls.some(c=>c.name==='pause'));assert.ok(calls.some(c=>c.name==='resume'));
assert.deepEqual(calls.find(c=>c.name==='convert').args[0],['C:/Downloads/finished.webm']);
assert.deepEqual(errors,[]);console.log('Passed: grouping modes, sorting, collapse/expand, visible selection and keyboard order, persistence, reset, real Discord modal, menu parity, entry actions, and responsive layout.');
}finally{await browser.close();await new Promise(r=>server.close(r));}
