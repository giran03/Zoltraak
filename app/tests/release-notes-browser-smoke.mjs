import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { readFile, readdir, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bundle = await build({ stdin: { resolveDir: root, loader: 'tsx', contents: `
import React,{useState} from 'react';
import {createRoot} from 'react-dom/client';
import {ReleaseNotesDialog} from './src/components/ReleaseNotesDialog';
import {useAppearance} from './src/lib/useAppearance';
import {hasNewRelease,releaseVersion,RELEASE_SEEN_KEY,currentRelease} from './src/lib/releaseNotes';
Object.assign(window,{hasNewRelease,releaseVersion,RELEASE_SEEN_KEY,highlightCount:currentRelease.changes.length});
function Fixture(){const [manual,setManual]=useState(false);useAppearance();return <>
<button onClick={()=>setManual(true)}>Read notes again</button>
<ReleaseNotesDialog autoOpen/>
{manual&&<ReleaseNotesDialog onClose={()=>setManual(false)}/>}</>}
createRoot(document.getElementById('root')).render(<React.StrictMode><Fixture/></React.StrictMode>);
` }, bundle:true, write:false, format:'iife', platform:'browser', jsx:'automatic', loader:{'.png':'dataurl'} });
const assets = path.join(root,'dist','assets');
const css = (await readdir(assets)).find(name=>name.endsWith('.css'));
const server = createServer(async(req,res)=>{
  if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}
  else if(req.url.startsWith('/assets/')){res.setHeader('Content-Type',req.url.endsWith('.css')?'text/css':'font/woff2');res.end(await readFile(path.join(assets,path.basename(req.url))));}
  else{res.setHeader('Content-Type','text/html');res.end('<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/'+css+'"><div id="root"></div><script src="/fixture.js"></script></html>');}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser = await chromium.launch({headless:true,...(process.env.CHROME_PATH?{executablePath:process.env.CHROME_PATH}:{})});
const shots = path.join(root,'../scratch/release-notes');
await mkdir(shots,{recursive:true});
try {
  const page = await browser.newPage({viewport:{width:1366,height:900}}), errors=[];
  page.on('pageerror',error=>errors.push(error.message));
  await page.goto('http://127.0.0.1:'+server.address().port);
  const dialog = page.getByRole('dialog');
  await dialog.waitFor();
  const highlightCount=await page.evaluate(()=>highlightCount);
  assert.equal(await dialog.locator('li').count(),highlightCount,'brief view has highlights only');
  assert.equal(await page.evaluate(()=>hasNewRelease('0.2.5','0.2.4')),true);
  assert.equal(await page.evaluate(()=>hasNewRelease('0.2.5','0.2.5')),false);
  assert.equal(await page.evaluate(()=>hasNewRelease('0.2.5','0.3.0')),false);
  assert.equal(await page.evaluate(()=>hasNewRelease('0.2.5','invalid')),true);
  assert.equal(await page.evaluate(()=>hasNewRelease('0.2.5','0.2.5-rc.2')),true);
  assert.equal(await page.evaluate(()=>hasNewRelease('0.2.5-rc.10','0.2.5-rc.2')),true);
  assert.equal(await page.evaluate(()=>hasNewRelease('0.2.5-rc.2','0.2.5')),false);
  await page.waitForTimeout(350);
  await page.screenshot({path:path.join(shots,'light-brief.png')});
  await page.getByRole('button',{name:'Detailed changelog',exact:true}).click();
  assert.equal(await page.getByRole('button',{name:'Detailed changelog',exact:true}).getAttribute('aria-pressed'),'true');
  assert.ok(await dialog.locator('li').count()>highlightCount);
  assert.equal(await page.evaluate(()=>localStorage.getItem(RELEASE_SEEN_KEY)),null,'view toggle does not mark notes read');
  await page.screenshot({path:path.join(shots,'light-detailed.png')});
  await page.keyboard.press('Escape');
  assert.equal(await dialog.count(),0);
  assert.equal(await page.evaluate(()=>localStorage.getItem(RELEASE_SEEN_KEY)),await page.evaluate(()=>releaseVersion));
  await page.reload();
  await page.getByRole('button',{name:'Read notes again'}).waitFor();
  assert.equal(await dialog.count(),0,'same release stays dismissed on relaunch');
  await page.getByRole('button',{name:'Read notes again'}).click();
  await dialog.waitFor();
  await page.getByRole('button',{name:'Continue',exact:true}).click();
  assert.equal(await page.getByRole('button',{name:'Read notes again'}).evaluate(el=>el===document.activeElement),true,'focus returns after manual close');
  await page.evaluate(()=>{localStorage.setItem(RELEASE_SEEN_KEY,'0.2.4');localStorage.setItem('zoltraak_appearance',JSON.stringify({theme:'dark'}));});
  await page.reload();
  await dialog.waitFor();
  // Theme changes use the same inherited tokens as the app.
  await page.evaluate(()=>document.documentElement.setAttribute('data-theme','dark'));
  const darkBackground = await dialog.evaluate(el=>getComputedStyle(el).backgroundColor);
  await page.getByRole('button',{name:'Detailed changelog',exact:true}).click();
  await page.waitForTimeout(350);
  await page.screenshot({path:path.join(shots,'dark-detailed.png')});
  await page.setViewportSize({width:390,height:600});
  await page.screenshot({path:path.join(shots,'mobile-detailed.png')});
  const metrics = await dialog.evaluate(el=>{
    const rect=el.getBoundingClientRect(),body=el.querySelector('[id][class*="overflow-y-auto"]'),footer=el.querySelector('footer').getBoundingClientRect();
    return {width:rect.width,right:rect.right,bottom:rect.bottom,footerBottom:footer.bottom,scroll:body.scrollHeight,client:body.clientHeight,scrollWidth:el.scrollWidth,clientWidth:el.clientWidth};
  });
  assert.ok(metrics.right<=390&&metrics.bottom<=600&&metrics.footerBottom<=600,JSON.stringify(metrics));
  assert.ok(metrics.scroll>metrics.client,'long notes scroll inside the body');
  assert.ok(metrics.scrollWidth<=metrics.clientWidth+1,'mobile modal has no horizontal overflow');
  await page.evaluate(()=>document.documentElement.setAttribute('data-theme','light'));
  assert.notEqual(await dialog.evaluate(el=>getComputedStyle(el).backgroundColor),darkBackground,'open dialog follows theme changes');
  await page.getByRole('button',{name:'Close release notes',exact:true}).click();
  await page.evaluate(()=>localStorage.setItem(RELEASE_SEEN_KEY,'9.0.0'));
  await page.reload();
  await page.getByRole('button',{name:'Read notes again'}).waitFor();
  assert.equal(await dialog.count(),0,'downgrades do not announce an older release');
  await page.getByRole('button',{name:'Read notes again'}).click();
  await page.getByRole('button',{name:'Continue',exact:true}).click();
  assert.equal(await page.evaluate(()=>localStorage.getItem(RELEASE_SEEN_KEY)),'9.0.0','manual viewing preserves newer seen version');
  assert.deepEqual(errors,[]);
  console.log('Passed: new-version launch, dismissal/relaunch, details toggle, manual viewing, focus, themes, mobile scrolling, prereleases and downgrades.');
} finally { await browser.close(); await new Promise(resolve=>server.close(resolve)); }
