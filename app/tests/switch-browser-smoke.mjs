import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.resolve(root, '../scratch/switches');
await mkdir(output, { recursive: true });
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'fixture.tsx', loader: 'tsx', contents: `
import React, {useState} from 'react';
import {createRoot} from 'react-dom/client';
import {Switch} from './src/components/Controls';
function Fixture() {
  const [off,setOff]=useState(false),[on,setOn]=useState(true);
  return <main className="mx-auto max-w-xl p-6 text-ink"><h1 className="text-xl">Download settings</h1>
    <div className="mt-6 flex items-center justify-between gap-6 border-b border-edge py-4"><span>Launch at startup</span><Switch label="Launch at startup" checked={off} onChange={setOff}/></div>
    <div className="flex items-center justify-between gap-6 border-b border-edge py-4"><span>Browser integration</span><Switch label="Browser integration" checked={on} onChange={setOn}/></div>
    <div className="flex items-center justify-between gap-6 border-b border-edge py-4"><span>Unavailable setting</span><Switch label="Unavailable setting" checked={false} onChange={()=>{throw new Error('Disabled switch fired');}} disabled/></div>
  </main>;
}
createRoot(document.getElementById('root')).render(<Fixture/>);
` }, bundle:true, write:false, format:'iife', platform:'browser', jsx:'automatic' });
const assets=path.join(root,'dist','assets');
const css=(await readdir(assets)).find(name=>name.endsWith('.css'));
const server=createServer(async(req,res)=>{
  if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
  else if(req.url.startsWith('/assets/')){
    try{res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))));}
    catch{res.writeHead(404);res.end();}
  }else{res.setHeader('Content-Type','text/html');res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
try{
  const page=await browser.newPage({viewport:{width:1200,height:800}});
  const errors=[];
  page.on('pageerror',error=>errors.push(error.message));
  await page.goto('http://127.0.0.1:'+server.address().port);
  const off=page.getByRole('switch',{name:'Launch at startup'});
  const on=page.getByRole('switch',{name:'Browser integration'});
  const styles=async control=>control.evaluate(el=>{
    const track=getComputedStyle(el),thumb=getComputedStyle(el.firstElementChild),rect=el.getBoundingClientRect();
    return {width:rect.width,height:rect.height,radius:track.borderRadius,thumbWidth:thumb.width,thumbHeight:thumb.height,translate:thumb.translate,animation:thumb.animationName,duration:thumb.animationDuration,hitArea:getComputedStyle(el,'::before').top};
  });
  assert.equal((await styles(off)).animation,'none');
  assert.equal((await styles(on)).animation,'none');
  assert.equal((await styles(on)).translate,'14.66px');
  assert.deepEqual(await off.evaluate(el=>({checked:el.getAttribute('aria-checked'),on:el.dataset.on})),{checked:'false',on:'false'});
  const geometry=await styles(off);
  assert.equal(geometry.width,44);assert.equal(geometry.height,22);assert.equal(geometry.radius,'122px');
  assert.equal(geometry.thumbWidth,'22px');assert.ok(Math.abs(parseFloat(geometry.thumbHeight)-14.67)<0.02);assert.equal(geometry.hitArea,'-11px');
  await off.click();
  assert.equal(await off.getAttribute('aria-checked'),'true');
  assert.equal((await styles(off)).animation,'t-toggle-on');
  assert.equal((await styles(off)).duration,'0.35s');
  await page.waitForTimeout(400);
  assert.equal((await styles(off)).translate,'14.66px');
  await off.press('Space');
  assert.equal(await off.getAttribute('aria-checked'),'false');
  assert.equal((await styles(off)).animation,'t-toggle-off');
  await page.waitForTimeout(400);
  assert.equal((await styles(off)).translate,'0px');
  await off.press('Enter');
  await off.click();await off.click();await off.click();
  await page.waitForTimeout(400);
  assert.equal(await off.getAttribute('aria-checked'),'false');assert.equal((await styles(off)).translate,'0px');
  assert.equal(await page.getByRole('switch',{name:'Unavailable setting'}).isDisabled(),true);
  for(const [theme,width] of [['light',1200],['dark',1200],['dark',390]]){
    await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);
    await page.setViewportSize({width,height:800});
    await page.screenshot({path:path.join(output,theme+'-'+width+'.png')});
    assert.equal(await on.evaluate(el=>el.getBoundingClientRect().right<=innerWidth),true);
  }
  await page.emulateMedia({reducedMotion:'reduce'});
  await off.click();assert.equal((await styles(off)).animation,'none');assert.equal((await styles(off)).translate,'14.66px');
  await page.emulateMedia({reducedMotion:'no-preference'});
  await page.evaluate(()=>document.documentElement.dataset.motion='reduced');
  await off.click();assert.equal((await styles(off)).animation,'none');assert.equal((await styles(off)).translate,'0px');
  await page.evaluate(()=>document.documentElement.dataset.motion='subtle');
  await off.click();assert.equal((await styles(off)).duration,'0.18s');
  assert.deepEqual(errors,[]);
  console.log('Passed: reference geometry, no mount bounce, both animations, rapid toggles, keyboard, disabled state, themes, narrow layout, and OS/app reduced motion.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
