import assert from 'node:assert/strict';
import path from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {createServer} from 'node:http';
import {build} from '../node_modules/esbuild/lib/main.js';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const {chromium}=await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const bundle=await build({stdin:{resolveDir:root,loader:'tsx',contents:`
import React,{useState} from 'react';import{createRoot}from'react-dom/client';
import{VideoView}from'./src/components/VideoView';import{ImagesView}from'./src/components/ImagesView';
import{ErrorStrip}from'./src/components/Controls';import{useVideo}from'./src/lib/useVideo';
window.active=true;document.hasFocus=()=>window.active;
window.pending=false;window.resolveAnalysis=null;
window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
if(name==='list_video_jobs')return [];
if(name==='open_url'){window.openedUrl=args.url;return;}
const result=name==='analyze_video'?{title:'Fixture video',webpageUrl:args.url,formats:[]}:{title:'Fixture album',sourceUrl:args.url,items:[],warnings:[]};
if(name==='analyze_video'||name==='analyze_images'){
if(window.pending)await new Promise(resolve=>window.resolveAnalysis=resolve);return name==='analyze_images'?(window.imageResult||result):result;}
throw Error('Unexpected '+name);}};
function Fixture(){const video=useVideo();const[error,setError]=useState('Fixture fault');const[tab,setTab]=useState('video');return <>
<button onClick={()=>setTab('video')}>Video fixture</button><button onClick={()=>setTab('images')}>Images fixture</button>
<button onClick={()=>setError('Replacement fault')}>Replace feedback</button>
{error&&<ErrorStrip message={error} onDismiss={()=>setError(null)}/>}
{tab==='video'?<VideoView video={video}/>:<ImagesView/>}</>}
createRoot(document.getElementById('root')).render(<Fixture/>);
`},bundle:true,write:false,platform:'browser',format:'iife',jsx:'automatic'});
const server=createServer((req,res)=>{if(req.url==='/fixture.js'){res.setHeader('Content-Type','text/javascript');res.end(bundle.outputFiles[0].text);}else res.end('<!doctype html><div id="root"></div><script src="/fixture.js"></script>');});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true});
try{
 const page=await browser.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.clock.install();await page.goto('http://127.0.0.1:'+server.address().port);
 await page.getByText('Fixture fault',{exact:true}).waitFor();
 await page.clock.runFor(3000);
 await page.evaluate(()=>{window.active=false;window.dispatchEvent(new Event('blur'))});
 await page.clock.runFor(10000);assert.equal(await page.getByText('Fixture fault',{exact:true}).count(),1,'background feedback remains');
 await page.evaluate(()=>{window.active=true;window.dispatchEvent(new Event('focus'))});
 await page.clock.runFor(4000);assert.equal(await page.getByText('Fixture fault',{exact:true}).count(),1);
 await page.getByRole('button',{name:'Replace feedback'}).click();
 await page.clock.runFor(2000);assert.equal(await page.getByText('Replacement fault',{exact:true}).count(),1,'new message gets a fresh timer');
 await page.clock.runFor(3000);assert.equal(await page.getByText('Replacement fault',{exact:true}).count(),0);
 for(const [tab,label,action,title] of [['video','Video page or media URL','analyze','Fixture video'],['images','Post or media URL','extract','Fixture album']]){
   await page.getByRole('button',{name:tab==='video'?'Video fixture':'Images fixture'}).click();
   const input=page.getByRole('textbox',{name:label});
   await input.fill('https://example.com/post');await page.getByRole('button',{name:action,exact:true}).click();
   await page.getByText(title,{exact:true}).waitFor();
   await input.fill('   ');assert.equal(await page.getByText(title,{exact:true}).count(),0,'whitespace clears '+tab+' results');
   await page.evaluate(()=>window.pending=true);
   await input.fill('https://example.com/slow');await page.getByRole('button',{name:action,exact:true}).click();
   await page.waitForFunction(()=>!!window.resolveAnalysis);await input.fill('');
   await page.evaluate(()=>{window.pending=false;window.resolveAnalysis();window.resolveAnalysis=null});
   await page.waitForTimeout(50);assert.equal(await page.getByText(title,{exact:true}).count(),0,'late response cannot restore '+tab+' results');
   assert.equal(await page.getByRole('button',{name:action,exact:true}).isEnabled(),true);
 }
 await page.evaluate(()=>{window.imageResult={title:'Partial Facebook gallery',sourceUrl:'https://www.facebook.com/user/posts/pfbidSelected',expectedCount:7,completeness:'partial',warnings:['Facebook did not expose the next attachment.'],items:[1,2,3,4,5].map(id=>({id:'facebook:'+id,url:'data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg"/>',extension:'jpg',kind:'image'}))}});
 await page.getByRole('textbox',{name:'Post or media URL'}).fill('https://www.facebook.com/user/posts/pfbidSelected');
 await page.getByRole('button',{name:'extract',exact:true}).click();
 await page.getByText('Partial Facebook gallery',{exact:true}).waitFor();
 await page.getByRole('button',{name:'Open Facebook post',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.openedUrl),'https://www.facebook.com/user/posts/pfbidSelected');
 await page.evaluate(()=>{window.imageResult={...window.imageResult,expectedCount:5,completeness:'complete',warnings:[]}});
 await page.getByRole('button',{name:'extract',exact:true}).click();
 await page.getByRole('button',{name:'Open Facebook post',exact:true}).waitFor({state:'detached'});
 assert.deepEqual(errors,[]);console.log('Passed: focus dismissal, empty inputs, late responses and incomplete Facebook gallery recovery action.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
