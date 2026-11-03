import assert from 'node:assert/strict';
import path from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {readFile,mkdir} from 'node:fs/promises';
import {createServer} from 'node:http';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const {chromium}=await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const guide=await readFile(path.join(root,'scripts/templates/guide.html'),'utf8');
const log=JSON.parse(await readFile(path.join(root,'CHANGELOG.json'),'utf8'));
const currentVersion=log.development.version;
const release=log.releases[currentVersion];
let theme={colorScheme:'dark',variables:{'--panel':'#161a17','--panel-face':'#1e2320','--ink':'#e9ede7','--ink-dim':'#a6aea4'}};
const server=createServer((req,res)=>{
 if(req.url==='/guide/theme'){res.setHeader('Content-Type','application/json');res.end(JSON.stringify(theme));}
 else if(req.url==='/health'){res.setHeader('Content-Type','application/json');res.end('{"version":"0.2.5"}');}
 else{res.setHeader('Content-Type','text/html');res.end(guide);}
});await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true});
try{
 const page=await browser.newPage({viewport:{width:1366,height:1000}}),errors=[];
 page.on('pageerror',error=>errors.push(error.message));
 // The guide's existing health probe targets the fixed local engine port.
 await page.route('http://127.0.0.1:1450/health',route=>route.fulfill({contentType:'application/json',body:'{"version":"0.2.5"}'}));
 await page.goto('http://127.0.0.1:'+server.address().port+'/guide?view=changelog');
 const content=page.locator('#changelogPage');await content.waitFor();
 assert.equal(await page.locator('.layout-frame').isVisible(),false);
 assert.equal(await page.getByRole('link',{name:'Changelog',exact:true}).getAttribute('aria-current'),'page');
 await page.getByLabel('Version',{exact:true}).selectOption(currentVersion);
 const initial=await content.locator('.change-entry:visible').count();assert.equal(initial,release.changes.length);
 const fixed=release.changes.filter(change=>change.category==='Fixed').length;
 await page.getByLabel('Category',{exact:true}).selectOption('Fixed');assert.equal(await content.locator('.change-entry:visible').count(),fixed);
 await page.getByRole('button',{name:'Expand details',exact:true}).click();assert.equal(await content.locator('.change-entry[open]:visible').count(),fixed);
 await page.getByRole('button',{name:'Reset filters',exact:true}).click();
 await page.getByLabel('Search changes',{exact:true}).fill('no-such-feature');assert.equal(await page.locator('#changeEmpty').isVisible(),true);
 await page.getByRole('button',{name:'Reset filters',exact:true}).click();
 if(log.historyStartVersion){
 assert.deepEqual(await page.getByLabel('Version',{exact:true}).locator('option').allTextContents(),['All versions',currentVersion]);
 assert.equal(await content.locator('.change-release').count(),1,'first standalone release has one consolidated page');
 assert.ok(!/licens|activation|attestation/i.test(await content.innerText()),'standalone notes contain no retired access-system text');
 }else{
 await page.getByLabel('Search changes',{exact:true}).fill('0.2.4');
 await page.getByLabel('Version',{exact:true}).selectOption('0.2.4');assert.equal(await content.locator('.change-release:visible').count(),1);
 assert.equal(await content.locator('.change-release:visible .change-status').textContent(),'historical');
 await page.getByRole('button',{name:'Reset filters',exact:true}).click();
 await page.getByLabel('Version',{exact:true}).selectOption('0.2.5');
 assert.equal(await content.locator('.change-release:visible .change-status').textContent(),'superseded');
 assert.equal(await content.locator('.change-entry:visible').count(),0,'transferred notes are not duplicated');
 assert.equal(await page.locator('#changeEmpty').isVisible(),false,'superseded marker stays visible in history');
 }
 await page.getByRole('button',{name:'Reset filters',exact:true}).click();
 const shots=path.join(root,'scratch/changelog-guide');await mkdir(shots,{recursive:true});
 await page.screenshot({path:path.join(shots,'dark-desktop.png')});
 theme={colorScheme:'light',variables:{'--panel':'#d7dcd2','--panel-face':'#e6eae0','--ink':'#161a14','--ink-dim':'#3c4239'}};
 await page.waitForFunction(()=>document.documentElement.dataset.theme==='light');
 assert.equal(await page.evaluate(()=>getComputedStyle(document.documentElement).getPropertyValue('--c-panel').trim()),'#d7dcd2');
 await page.setViewportSize({width:390,height:900});
 await page.screenshot({path:path.join(shots,'light-mobile.png')});
 assert.equal(await content.evaluate(el=>el.scrollWidth<=el.clientWidth),true,'changelog fits narrow screens');
 assert.equal(await page.locator('header.masthead').evaluate(el=>el.scrollWidth<=el.clientWidth),true,'navbar fits narrow screens');
 await content.locator('.change-release[data-version="'+currentVersion+'"] h2 a').click();
 assert.ok(page.url().endsWith('?view=changelog#release-'+currentVersion),'version link preserves changelog route');
 await page.getByRole('link',{name:'Guide',exact:true}).click();await page.locator('.layout-frame').waitFor();
 assert.equal(await content.isVisible(),false,'guide remains available');
 assert.deepEqual(errors,[]);
 console.log('Passed: guide/changelog navigation, categories, version/text filters, details, permalinks, historical labels, live theme and mobile layout.');
}finally{await browser.close();await new Promise(resolve=>server.close(resolve));}
