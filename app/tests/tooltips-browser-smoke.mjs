import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'tooltip-fixture.tsx', loader: 'tsx', contents: `
import React,{useState,useRef,useEffect} from 'react';
import {createRoot} from 'react-dom/client';
import {TooltipGroup} from './src/components/Tooltip';
import {ReleaseNotesDialog} from './src/components/ReleaseNotesDialog';
const long='A longer browser extension notice that must remain completely readable, wrap within the available window width, and never be truncated by the narrow group of icons.';
function NativeFixture({onClose}){const dialog=useRef(null);useEffect(()=>{dialog.current.showModal()},[]);return <dialog ref={dialog} style={{position:'fixed',top:200,left:100,right:'auto',bottom:'auto',margin:0,padding:8,overflow:'hidden',width:300,height:120,transform:'scale(1)'}} onClose={onClose}><TooltipGroup><button className="t-tt-trigger" data-tooltip="Tooltip inside another native dialog">Modal action</button></TooltipGroup><button onClick={()=>dialog.current.close()}>Close fixture dialog</button></dialog>}
function Fixture(){const[release,setRelease]=useState(false),[native,setNative]=useState(false);return <>
<button onClick={()=>setRelease(true)}>Open release fixture</button><button onClick={()=>setNative(true)}>Open native fixture</button>
{release&&<ReleaseNotesDialog onClose={()=>setRelease(false)}/>} {native&&<NativeFixture onClose={()=>setNative(false)}/>}
<div style={{position:'fixed',right:0,top:0,width:60,height:36,overflow:'hidden',transform:'translateZ(0)'}}>
<TooltipGroup side="bottom"><button className="t-tt-trigger" data-tooltip="Extension: Connected to the desktop app">Extension</button></TooltipGroup></div>
<div style={{position:'fixed',bottom:0,left:0,width:80,height:30,overflow:'hidden'}}>
<TooltipGroup side="bottom"><button className="t-tt-trigger" data-tooltip={long}>Long text</button><button className="t-tt-trigger" data-tooltip={'https://example.com/'+ 'a'.repeat(300)}>URL</button></TooltipGroup></div>
</>};createRoot(document.getElementById('root')).render(<Fixture/>);
` }, bundle:true, write:false, format:'iife', platform:'browser', jsx:'automatic' });
const assets=path.join(root,'dist','assets'), css=(await readdir(assets)).find(name=>name.endsWith('.css'));
const server=createServer(async(req,res)=>{
  if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
  else if(req.url.startsWith('/assets/')){res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))));}
  else{res.setHeader('Content-Type','text/html');res.end('<!doctype html><html lang="en" data-theme="dark"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
try{
 const page=await browser.newPage({viewport:{width:1366,height:768}}),errors=[];
 page.on('pageerror',error=>errors.push(error.message));
 await page.goto('http://127.0.0.1:'+server.address().port);
 const check=async(name)=>{
   const button=page.getByRole('button',{name,exact:true});await button.focus();
   const tip=page.locator('.t-tt[data-show="true"]');await tip.waitFor();
   await page.waitForTimeout(400);
   assert.equal(await tip.textContent(),await button.getAttribute('data-tooltip'));
   const metrics=await tip.evaluate(el=>{const r=el.getBoundingClientRect(),text=el.firstElementChild;return {x:r.x,y:r.y,right:r.right,bottom:r.bottom,width:r.width,height:r.height,scroll:el.scrollWidth,client:el.clientWidth,textScroll:text.scrollWidth,textClient:text.clientWidth,ellipsis:getComputedStyle(text).textOverflow,parent:el.parentElement.parentElement.tagName,vw:innerWidth,vh:innerHeight,side:el.dataset.side};});
   assert.equal(metrics.parent,'BODY','tooltips escape clipping ancestors');
   assert.ok(metrics.x>=7&&metrics.y>=7&&metrics.right<=metrics.vw-7&&metrics.bottom<=metrics.vh-7,JSON.stringify(metrics));
   assert.ok(metrics.scroll<=metrics.client+1&&metrics.textScroll<=metrics.textClient+1,'text must wrap instead of clipping');
   assert.notEqual(metrics.ellipsis,'ellipsis');
   assert.equal(await button.getAttribute('aria-describedby'),await tip.getAttribute('id'));
   return metrics;
 };
 const license=await check('Extension');assert.ok(license.width>120,'tooltip must outgrow narrow icon group');
 assert.equal((await check('Long text')).side,'top','bottom-edge tooltip flips up');
 await check('URL');
 await page.setViewportSize({width:320,height:568});
 const narrow=await check('Long text');assert.ok(narrow.height>40,'long text wraps on narrow windows');
 await check('URL');
 await page.keyboard.press('Escape');assert.equal(await page.locator('.t-tt[data-show="true"]').count(),0);
 assert.equal(await page.getByRole('button',{name:'URL',exact:true}).getAttribute('aria-describedby'),null);
 await page.getByRole('button',{name:'Extension',exact:true}).hover();
 await page.waitForTimeout(400);
 const hoverTip=page.locator('.t-tt[data-show="true"]');
 assert.equal(await hoverTip.evaluate(el=>getComputedStyle(el).pointerEvents),'none','tooltip ignores the pointer');
 const bubble=await hoverTip.boundingBox();
 await page.mouse.move(bubble.x+bubble.width/2,bubble.y+bubble.height/2);
 assert.equal(await page.locator('.t-tt[data-show="true"]').count(),0,'moving onto the tooltip dismisses it');
 await page.setViewportSize({width:1366,height:900});
 await page.getByRole('button',{name:'Open release fixture',exact:true}).click();
 await page.getByRole('button',{name:'Detailed changelog',exact:true}).hover();
 await page.waitForTimeout(400);
 const modalTip=page.locator('.t-tt[data-show="true"]');
 assert.equal(await modalTip.evaluate(el=>el.matches(':popover-open')),true,'modal tooltip uses the browser top layer');
 assert.equal(await modalTip.evaluate(el=>Boolean(el.closest('dialog[open]'))),true,'tooltip belongs to the active dialog');
 // Temporarily hit-test the painted bubble; production tooltips ignore pointer input.
 assert.equal(await modalTip.evaluate(el=>{const rect=el.getBoundingClientRect();el.style.pointerEvents='auto';const hit=document.elementFromPoint(rect.left+rect.width/2,rect.top+rect.height/2);el.style.pointerEvents='';return el===hit||el.contains(hit);}),true,'tooltip paints over the modal and backdrop');
 await page.getByRole('button',{name:'Close release notes',exact:true}).click();
 assert.equal(await page.locator('.t-tt:popover-open').count(),0,'closing modal cleans up its tooltip layer');
 await page.getByRole('button',{name:'Open native fixture',exact:true}).click();
 await page.getByRole('button',{name:'Modal action',exact:true}).hover();
 await page.waitForTimeout(400);
 const nativeTip=page.locator('.t-tt[data-show="true"]');
 assert.equal(await nativeTip.evaluate(el=>el.matches(':popover-open')),true);
 const bounds=await nativeTip.evaluate(el=>({tip:el.getBoundingClientRect().top,dialog:el.closest('dialog').getBoundingClientRect().top,trigger:el.closest('dialog').querySelector('.t-tt-trigger').getBoundingClientRect().top}));
 assert.ok(bounds.tip<bounds.dialog,'tooltip escapes modal overflow clipping: '+JSON.stringify(bounds));
 await page.keyboard.press('Escape');
 assert.equal(await page.getByRole('dialog').count(),0);
 assert.equal(await page.locator('.t-tt:popover-open').count(),0);
 assert.deepEqual(errors,[]);
 console.log('Tooltip checks passed: full text, natural sizing, wrapping, long URLs, clipping ancestors, edge flipping, narrow window, keyboard description, Escape and hover.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
