// Run with PLAYWRIGHT_MODULE pointing at an existing playwright-core/index.mjs.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'fixture.tsx', loader: 'tsx', contents: `
import React, {useState} from 'react';
import { createRoot } from 'react-dom/client';
import { ToastContainer } from './src/components/Toast';
import { useToasts } from './src/lib/useToasts';
function Fixture() {
  const notifications=useToasts();
  const [hidden,setHidden]=useState(false);
  window.hideToasts=setHidden;
  window.addToast=notifications.add;
  window.toastState=()=>({visible:notifications.toasts.map(t=>t.id),history:notifications.history});
  return <ToastContainer toasts={notifications.toasts} onDismiss={notifications.dismiss} hidden={hidden}/>;
}
const root=createRoot(document.getElementById('root'));
window.unmountToasts=()=>root.unmount();
root.render(<Fixture/>);
` }, bundle:true, write:false, format:'iife', platform:'browser', jsx:'automatic' });
const assets=path.join(root,'dist','assets');
const css=(await readdir(assets)).find(name=>name.endsWith('.css'));
const server=createServer(async(req,res)=>{
  if(req.url==='/fixture.js') {res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
  else if(req.url.startsWith('/assets/')) {
    try {res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))));}
    catch {res.writeHead(404);res.end();}
  } else {
    res.setHeader('Content-Type','text/html');
    res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');
  }
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH ? {executablePath:process.env.CHROME_PATH} : {})});
try {
  const page=await browser.newPage({viewport:{width:1200,height:800}});
  const errors=[];
  page.on('pageerror',e=>errors.push(e.message));
  await page.goto('http://127.0.0.1:'+server.address().port);
  await page.waitForFunction(()=>typeof window.addToast==='function');
  await page.clock.install();
  const visible=()=>page.evaluate(()=>window.toastState().visible);
  const advance=async(ms)=>{await page.clock.runFor(ms);await page.waitForTimeout(20);};
  const add=async(toasts)=>{
    await page.evaluate(items=>{for(const item of items)window.addToast({...item,title:item.id,message:'Fixture message'});},toasts);
    await page.waitForFunction(id=>window.toastState().visible.includes(id),toasts.at(-1).id);
  };
  await add([{id:'success',type:'success'},{id:'error',type:'error'},{id:'info',type:'info'},{id:'update',type:'info',durationMs:12000}]);
  assert.equal((await visible()).length,4);
  await advance(6001);
  assert.deepEqual(await visible(),['update'],'errors must expire along with success and info');
  await advance(6000);
  assert.deepEqual(await visible(),[],'update prompts must expire too');
  assert.equal(await page.getByRole('status').count(),0,'the rendered toast stack must disappear');
  assert.equal(await page.evaluate(()=>window.toastState().history.length),4,'history remains available');
  assert.equal(await page.evaluate(()=>JSON.parse(localStorage.getItem('zoltraak.notifications.v1')).length),4);

  // Replacing the same ID restarts its timer, including success -> error.
  await add([{id:'replaced',type:'success'}]);
  await advance(3000);
  await add([{id:'replaced',type:'error'}]);
  await advance(3001);
  assert.deepEqual(await visible(),['replaced']);
  await advance(3000);
  assert.deepEqual(await visible(),[]);
  assert.equal(await page.evaluate(()=>window.toastState().history.filter(t=>t.id==='replaced').length),1);

  // Invalid/legacy caller options cannot create a sticky toast.
  await page.evaluate(()=>{
    for(const [id,durationMs] of [['infinity',Infinity],['nan',NaN],['zero',0],['negative',-1]])
      window.addToast({id,type:'info',title:id,message:'Invalid duration',durationMs,persistent:true});
  });
  await page.waitForFunction(()=>window.toastState().visible.length===4);
  await advance(6001);
  assert.deepEqual(await visible(),[]);

  // Undo expires at exactly the same six-second window as the removal hook.
  await add([{id:'undo',type:'info',durationMs:6000}]);
  await advance(5000);
  assert.deepEqual(await visible(),['undo']);
  await advance(1100);
  assert.deepEqual(await visible(),[]);
  await add([{id:'manual',type:'error'}]);
  await page.getByRole('button',{name:'Dismiss notification',exact:true}).click();
  await advance(300);
  assert.deepEqual(await visible(),[]);
  await page.evaluate(()=>window.addToast({id:'action',type:'info',title:'Action fixture',message:'Click to dismiss',action:{label:'Run fixture action',onClick:()=>{window.actionRan=true;}}}));
  await page.getByRole('button',{name:'Run fixture action',exact:true}).click();
  await advance(300);
  assert.equal(await page.evaluate(()=>window.actionRan),true);
  assert.deepEqual(await visible(),[]);

  await add(Array.from({length:5},(_,i)=>({id:'batch-'+i,type:'error'})));
  assert.equal((await visible()).length,4);
  await advance(6001);
  assert.deepEqual(await visible(),[]);

  // History can hide the popup stack without extending the toast's lifetime.
  await add([{id:'history-hidden',type:'error'}]);
  await page.evaluate(()=>window.hideToasts(true));
  assert.equal(await page.locator('.t-toast').count(),0);
  await advance(6100);
  await page.evaluate(()=>window.hideToasts(false));
  assert.deepEqual(await visible(),[]);
  assert.equal(await page.locator('.t-toast').count(),0);

  // Unmount must cancel the hook's pending timeout handles.
  await page.evaluate(()=>{
    const schedule=window.setTimeout;
    const cancel=window.clearTimeout;
    window.trackedToastTimers=new Set();
    window.setTimeout=(callback,delay,...args)=>{
      const id=schedule(callback,delay,...args);
      if(delay===6000)window.trackedToastTimers.add(id);
      return id;
    };
    window.clearTimeout=id=>{window.trackedToastTimers.delete(id);cancel(id);};
  });
  await add([{id:'unmount-error',type:'error'},{id:'unmount-info',type:'info'}]);
  assert.equal(await page.evaluate(()=>window.trackedToastTimers.size),2);
  await page.evaluate(()=>window.unmountToasts());
  assert.equal(await page.evaluate(()=>window.trackedToastTimers.size),0);
  assert.deepEqual(errors,[]);
  console.log('Toast browser checks passed: all types expire, finite update prompts, custom/invalid durations, replacement, Undo, manual/action dismiss, stacking, history and unmount cleanup.');
} finally {await browser.close();await new Promise(resolve=>server.close(resolve));}
