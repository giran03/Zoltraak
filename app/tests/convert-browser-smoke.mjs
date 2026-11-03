// Real-browser checks with IPC fixtures; processing recipes are covered by Rust integration tests.
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { mkdtemp, readFile, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createServer } from 'node:http';
import { build } from '../node_modules/esbuild/lib/main.js';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = await mkdtemp(path.join(tmpdir(), 'zoltraak-convert-ui-'));
const bundle = await build({ stdin: { resolveDir: root, sourcefile: 'fixture.tsx', loader: 'tsx', contents: `
import React from 'react';
import { createRoot } from 'react-dom/client';
import { ConvertView } from './src/components/ConvertView.tsx';
import { useConvert } from './src/lib/useConvert.ts';
import { CONVERSION_FORMATS } from './src/lib/conversion.ts';
const files=[{path:'C:/media/clip.mp4',extension:'mp4',videoCodec:'h264',audioCodec:'aac',width:160,height:120,durationSeconds:10,bytes:12345,unprobed:false,audioTracks:[{index:0,codec:'aac',channels:2,sampleRate:48000,language:'eng'}]},
{path:'C:/media/song.wav',extension:'wav',videoCodec:null,audioCodec:'pcm_s16le',width:null,height:null,durationSeconds:10,bytes:23456,unprobed:false,audioTracks:[{index:0,codec:'pcm_s16le',channels:2,sampleRate:48000,language:null}]}];
window.calls=[];window.jobs=[{id:'complete',input:'C:/media/completed.mp4',filename:'completed.mp4',container:'mp4',state:'finished',stage:'Done',percent:100,output:'C:/media/completed.mp4',outputBytes:12345}];
window.__TAURI_INTERNALS__={invoke:async(name,args)=>{
window.calls.push({name,args});
if(name==='media_capabilities')return {formats:CONVERSION_FORMATS.map(f=>f.value),ffprobe:true};
if(name==='choose_media_files')return structuredClone(files);
if(name==='list_remux_jobs')return structuredClone(window.jobs);
if(name==='plan_conversion')return args.paths.map(path=>{
const audio=['mp3','m4a','alac','flac','wav','aiff','opus','ogg'].includes(args.settings.format);
const error=!audio&&path.endsWith('.wav')?'This output needs a picture or video source':null;
return {path,error,plan:error?null:{streams:{video:audio?'absent':'encode',audio:audio?'encode':'absent'},description:audio?'Encodes the selected audio track':'Encodes an animation',warnings:['Fixture plan'],durationSeconds:5,outputExtension:args.settings.format}};
});
if(name==='start_conversions') {window.jobs=args.paths.map((input,i)=>({id:'job'+i,input,filename:input.split('/').pop(),container:'mp4',conversion:args.settings,state:'failed',error:'Fixture failure',stage:null,percent:0,output:null}));return window.jobs.map(j=>j.id);}
if(name==='retry_remux_job')return null;
if(name==='open_remux_output')return null;
if(name==='clear_remux_done'){window.jobs=[];return 1;}
if(name.startsWith('plugin:'))return null;
if(name==='remux_queue_state')return {paused:false,parallelism:2,queued:window.waiting||[]};
if(name==='list_watch_rules')return {rules:[],statuses:[]};
if(name==='conversion_thumbnail')return 'data:image/svg+xml;utf8,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="160" height="120"><rect width="160" height="120" fill="#456"/></svg>');
if(name==='conversion_waveform')return Array.from({length:args.buckets},(_, i)=>(i%7)/7);
if(name==='move_remux_job') {const index=window.waiting.indexOf(args.id);const to=args.movement==='top'?0:args.movement==='up'?index-1:index+1;window.waiting.splice(index,1);window.waiting.splice(to,0,args.id);return null;}
if(['set_remux_paused','set_remux_parallelism','save_watch_rules'].includes(name))return null;
throw new Error('Unexpected command: '+name);
}};
function Fixture(){const[working,setWorking]=React.useState('C:/Downloads/Zoltraak');const convert=useConvert(true,working);window.setWorking=setWorking;return <ConvertView convert={convert}/>;}
createRoot(document.getElementById('root')).render(<Fixture/>);
` }, bundle: true, write: false, format: 'iife', platform: 'browser', jsx: 'automatic' });
const assets = path.join(root, 'dist', 'assets');
const cssName = (await readdir(assets)).find(name => name.endsWith('.css'));
const server = createServer(async (request, response) => {
  if (request.url === '/fixture.js') { response.setHeader('Content-Type', 'text/javascript'); response.end(bundle.outputFiles[0].text); }
  else if (request.url.startsWith('/assets/')) {
    const name = path.basename(request.url); response.setHeader('Content-Type', name.endsWith('.css') ? 'text/css' : 'font/woff2');
    try { response.end(await readFile(path.join(assets, name))); } catch { response.writeHead(404); response.end(); }
  } else { response.setHeader('Content-Type', 'text/html'); response.end('<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/assets/' + cssName + '"><div id="root"></div><script src="/fixture.js"></script>'); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const browser = await chromium.launch({ headless: true, channel: process.env.CHROME_CHANNEL || 'chrome' });
try {
  for (const width of [1200, 768, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 900 } });
    const errors = []; page.on('pageerror', error => errors.push(error.message));
    await page.goto('http://127.0.0.1:' + server.address().port);
    await page.getByRole('button', { name: 'Choose files', exact: true }).click();
    await page.getByText('1 ready · 1 excluded; excluded files stay in the list', { exact: true }).waitFor();
    const fieldRows = await page.locator('.conversion-view .grid').evaluateAll(grids => grids.flatMap(grid => {
      const rows = new Map();
      for (const field of grid.children) {
        const input = field.querySelector('input, select');
        if (!input || !input.getClientRects().length) continue;
        const row = field.getBoundingClientRect().top;
        const rect = input.getBoundingClientRect();
        rows.set(row, [...(rows.get(row) || []), { label: input.getAttribute('aria-label'), y: rect.top, height: rect.height }]);
      }
      return [...rows.values()];
    }));
    for (const row of fieldRows) for (const field of row) {
      assert.ok(Math.abs(field.y - row[0].y) < 0.5, `input tops align at ${width}px: ${JSON.stringify(row)}`);
      assert.equal(field.height, row[0].height, 'input heights match for text, number and select controls');
    }
    const destination = page.getByLabel('Destination folder', { exact: true });
    assert.equal(await destination.getAttribute('placeholder'), 'C:/Downloads/Zoltraak/Converted');
    await page.evaluate(() => window.setWorking('D:/Work'));
    await page.waitForFunction(() => document.querySelector('[aria-label="Destination folder"]').placeholder === 'D:/Work/Converted');
    await destination.fill('E:/Custom output');
    await page.evaluate(() => window.setWorking('D:/New work'));
    assert.equal(await destination.inputValue(), 'E:/Custom output', 'working-folder updates preserve explicit overrides');
    await destination.fill('');
    assert.equal(await destination.getAttribute('placeholder'), 'D:/New work/Converted');
    const folder = page.getByRole('button', { name: 'Reveal completed.mp4', exact: true });
    const trash = page.getByRole('button', { name: 'Remove completed.mp4 from history', exact: true });
    await folder.scrollIntoViewIfNeeded();
    const before = await folder.boundingBox();
    await folder.hover();
    await page.waitForFunction(() => getComputedStyle(document.querySelector('[aria-label="Reveal completed.mp4"] > .entry-icon-glyph')).transform !== 'none');
    const after = await folder.boundingBox();
    assert.equal(after.width, before.width); assert.equal(after.height, before.height, 'hover never moves the hit target');
    await trash.hover();
    assert.equal(await trash.evaluate(element => {
      const reference = document.createElement('span'); reference.style.color = 'var(--c-danger)';
      element.append(reference);
      const matches = getComputedStyle(element).color === getComputedStyle(reference).color;
      reference.remove(); return matches;
    }), true, 'remove hover uses the theme danger color');
    await page.emulateMedia({ reducedMotion: 'reduce' });
    assert.equal(await trash.locator('.entry-icon-glyph').evaluate(element => getComputedStyle(element).transform), 'none');
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await page.evaluate(() => document.documentElement.dataset.motion = 'reduced');
    await folder.hover();
    assert.equal(await folder.locator('.entry-icon-glyph').evaluate(element => getComputedStyle(element).transform), 'none');
    await page.evaluate(() => delete document.documentElement.dataset.motion);
    await folder.click();
    assert.equal(await page.evaluate(() => window.calls.filter(call => call.name === 'open_remux_output').length), 1);
    assert.equal(await page.locator('#section-files').count(), 0, 'large file intake is hidden after staging');
    assert.equal(await page.locator('[data-conversion-source-path]').getAttribute('data-conversion-source-path'), 'C:/media/clip.mp4');
    assert.equal(await page.getByLabel('Paths to add', { exact: true }).count(), 0, 'pasted paths are tucked away');
    await page.getByLabel('Selected file', { exact: true }).selectOption('C:/media/song.wav');
    assert.equal(await page.locator('[data-conversion-source-path]').getAttribute('data-conversion-source-path'), 'C:/media/song.wav');
    await page.getByRole('button', { name: 'Change files', exact: true }).click();
    assert.equal(await page.locator('#convert-file-manager').isVisible(), true);
    const pathsInput = page.getByLabel('Paths to add', { exact: true });
    const emptyHeight = await pathsInput.evaluate(element => element.getBoundingClientRect().height);
    assert.equal(await pathsInput.evaluate(element => getComputedStyle(element).resize), 'none');
    await pathsInput.fill('C:/media/one.mp4\nC:/media/two.mp4\nC:/media/three.mp4');
    assert.ok(await pathsInput.evaluate(element => element.getBoundingClientRect().height) > emptyHeight, 'paths box grows with its text');
    await pathsInput.fill(Array.from({ length: 40 }, (_, index) => `C:/media/clip-${index}.mp4`).join('\n'));
    assert.equal(await pathsInput.evaluate(element => element.getBoundingClientRect().height), 192, 'long path lists stop growing at the height cap');
    assert.equal(await pathsInput.evaluate(element => element.scrollHeight > element.clientHeight && getComputedStyle(element).overflowY === 'auto'), true, 'long path lists scroll inside the box');
    await pathsInput.fill('');
    assert.equal(await pathsInput.evaluate(element => element.getBoundingClientRect().height), emptyHeight, 'clearing the paths shrinks the box');
    await page.getByRole('button', { name: 'Done', exact: true }).click();
    assert.equal(await page.locator('#convert-file-manager').count(), 0);
    await page.getByRole('button', { name: 'Add files', exact: true }).click();
    assert.equal(await page.locator('[data-conversion-path]').count(), 2, 'adding existing inputs does not duplicate them');
    const initialSettings = await page.evaluate(() => window.calls.findLast(call => call.name === 'plan_conversion').args.settings);
    const outputFormat = page.getByLabel('Output format', { exact: true });
    await outputFormat.click();
    const formatMenu = page.getByRole('listbox', { name: 'Output format', exact: true });
    await formatMenu.waitFor();
    for (const heading of ['Video', 'Animation', 'Image', 'Audio']) {
      await formatMenu.getByText(heading, { exact: true }).waitFor();
    }
    assert.match(await formatMenu.getByRole('option', { name: /MP4/ }).textContent(), /MP4\(\.mp4\)•H\.264 \/ AAC/);
    await formatMenu.getByRole('option', { name: /GIF/ }).click();
    assert.equal(await formatMenu.count(), 0, 'choosing an option closes the custom dropdown menu');
    await page.waitForFunction(() => window.calls.findLast(call => call.name === 'plan_conversion')?.args.settings.format === 'gif');
    assert.deepEqual(await page.evaluate(() => window.calls.findLast(call => call.name === 'plan_conversion').args.settings), { ...initialSettings, format: 'gif' }, 'choosing GIF does not apply its balanced preset');
    await page.getByLabel('Preset', { exact: true }).selectOption('gif');
    const preset = page.getByLabel('Preset', { exact: true });
    assert.equal(await preset.inputValue(), 'gif', 'the chosen preset remains selected');
    assert.match(await preset.locator('option:checked').textContent(), /Balanced GIF/);
    await page.getByLabel('Maximum width (px)', { exact: true }).fill('320');
    assert.equal(await preset.locator('option:checked').textContent(), 'Custom', 'editing preset settings shows Custom');
    await page.getByLabel('Maximum width (px)', { exact: true }).fill('640');
    assert.equal(await preset.inputValue(), 'gif', 'restoring the exact recipe restores the preset label');
    await page.getByRole('switch', { name: 'Use separate settings for this file', exact: true }).click();
    await preset.selectOption('quality');
    assert.equal(await preset.inputValue(), 'quality');
    await page.getByLabel('Selected file', { exact: true }).selectOption('C:/media/clip.mp4');
    assert.equal(await preset.inputValue(), 'gif', 'the shared recipe retains its preset');
    await page.getByLabel('Selected file', { exact: true }).selectOption('C:/media/song.wav');
    assert.equal(await preset.inputValue(), 'quality', 'individual recipes retain their preset');
    await page.getByRole('switch', { name: 'Use separate settings for this file', exact: true }).click();
    assert.equal(await preset.inputValue(), 'gif', 'returning to shared settings restores the shared preset');
    await page.getByLabel('Selected file', { exact: true }).selectOption('C:/media/clip.mp4');
    await page.getByText('Advanced settings', { exact: true }).click();
    const metadata = page.getByRole('switch', { name: 'Remove metadata', exact: true });
    assert.equal(await page.getByText('Remove metadata', { exact: true }).isVisible(), true);
    assert.match(await metadata.getAttribute('aria-describedby'), /.+/);
    assert.match(await metadata.evaluate(element => document.getElementById(element.getAttribute('aria-describedby')).textContent), /On: omit copied titles/);
    const helpButton = page.getByRole('button', { name: 'About GIF dithering', exact: true });
    await helpButton.scrollIntoViewIfNeeded();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await page.mouse.move(0, 0);
    await helpButton.hover();
    const helpTooltip = page.getByRole('tooltip').filter({ hasText: 'About GIF dithering' });
    await helpTooltip.waitFor();
    await helpButton.click();
    const help = page.getByRole('dialog', { name: 'GIF dithering', exact: true });
    await help.waitFor();
    await help.getByText('Smooth · Sierra', { exact: true }).waitFor();
    await help.getByText('Ordered · Bayer', { exact: true }).waitFor();
    await help.getByText('None', { exact: true }).waitFor();
    const helpBounds = await help.boundingBox();
    assert.ok(helpBounds.x >= 0 && helpBounds.x + helpBounds.width <= width);
    await page.keyboard.press('Escape');
    assert.equal(await help.count(), 0);
    assert.equal(await helpButton.evaluate(element => element === document.activeElement), true, 'closing help restores focus');
    assert.equal(await helpTooltip.count(), 0, 'restored focus does not revive the tooltip');
    await helpButton.scrollIntoViewIfNeeded();
    await page.mouse.move(0, 0);
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await helpButton.hover();
    await helpTooltip.waitFor();
    assert.equal(await page.getByLabel('GIF dithering', { exact: true }).inputValue(), 'sierra2_4a', 'help does not change settings');
    assert.equal(await page.getByLabel('GIF dithering', { exact: true }).isDisabled(), false, 'GIF dithering is enabled for GIF output');
    assert.equal(await page.getByLabel('Quality (1–100)', { exact: true }).isDisabled(), true, 'GIF output disables the quality field');
    assert.equal(await page.getByLabel('Target file size (MB)', { exact: true }).isDisabled(), true, 'GIF output disables target file size');
    assert.equal(await page.getByLabel('Audio bitrate (kbps)', { exact: true }).isDisabled(), true, 'GIF output disables audio bitrate');
    assert.equal(await page.getByLabel('Sample rate', { exact: true }).isDisabled(), true, 'GIF output disables sample rate');
    assert.equal(await page.getByLabel('Processing', { exact: true }).locator('option[value="copy"]').isDisabled(), true, 'GIF output disables stream copy option');
    assert.equal(await preset.locator('option[value="voice"]').isDisabled(), true, 'GIF output disables Opus preset option');
    await helpButton.click();
    await help.getByRole('button', { name: 'Got it', exact: true }).click();
    assert.equal(await help.count(), 0);
    assert.equal(await helpTooltip.count(), 0, 'the close button also leaves the tooltip hidden');
    await page.keyboard.press('Tab');
    await helpButton.focus();
    await helpTooltip.waitFor();
    await metadata.click();
    await page.getByLabel('Maximum width (px)', { exact: true }).fill('320');
    const editor = page.getByTestId('trim-crop-editor');
    const frameImg = editor.locator('img');
    await frameImg.waitFor();
    assert.equal(await frameImg.evaluate(el => el.style.transform), '', 'initial preview frame is unrotated');
    await editor.getByRole('button', { name: 'Rotate 90°', exact: true }).click();
    assert.match(await frameImg.evaluate(el => el.style.transform), /rotate\(90deg\)/, 'rotating 90° updates preview transform');
    await editor.getByRole('button', { name: 'Flip ↔', exact: true }).click();
    assert.match(await frameImg.evaluate(el => el.style.transform), /scale\(-1, 1\) rotate\(90deg\)/, 'horizontal flip composes with rotation on preview');
    await editor.getByRole('button', { name: 'Flip ↕', exact: true }).click();
    assert.match(await frameImg.evaluate(el => el.style.transform), /scale\(-1, -1\) rotate\(90deg\)/, 'vertical flip composes with rotation on preview');
    await editor.getByRole('button', { name: 'Reset all', exact: true }).click();
    assert.equal(await frameImg.evaluate(el => el.style.transform), '', 'resetting orientation clears preview transform');
    await page.getByRole('button', { name: 'Convert 1 compatible file', exact: true }).click();
    const start = await page.evaluate(() => window.calls.find(call => call.name === 'start_conversions').args);
    assert.deepEqual(start.paths, ['C:/media/clip.mp4']);
    assert.equal(start.settings.format, 'gif'); assert.equal(start.settings.width, 320); assert.equal(start.replaceSource, false);
    assert.equal(start.settings.stripMetadata, true, 'labelled metadata switch still controls the queued recipe');
    assert.equal(start.directory, 'D:/New work/Converted', 'an empty destination resolves the current working-folder subfolder');
    await page.getByRole('button', { name: 'Retry clip.mp4', exact: true }).click();
    assert.equal(await page.evaluate(() => window.calls.filter(call => call.name === 'retry_remux_job').length), 1);
    const customSettings = await page.evaluate(() => window.calls.findLast(call => call.name === 'plan_conversion').args.settings);
    await page.getByLabel('Output format', { exact: true }).selectOption('mp3');
    await page.waitForFunction(() => window.calls.findLast(call => call.name === 'plan_conversion')?.args.settings.format === 'mp3');
    assert.deepEqual(await page.evaluate(() => window.calls.findLast(call => call.name === 'plan_conversion').args.settings), { ...customSettings, format: 'mp3' }, 'changing format preserves customized settings');
    assert.equal(await page.getByLabel('Maximum width (px)', { exact: true }).isDisabled(), true, 'audio format disables maximum width');
    assert.equal(await page.getByLabel('GIF dithering', { exact: true }).isDisabled(), true, 'audio format disables GIF dithering');
    assert.equal(await page.getByLabel('Audio bitrate (kbps)', { exact: true }).isDisabled(), false, 'MP3 enables audio bitrate');
    assert.equal(await page.getByLabel('Processing', { exact: true }).locator('option[value="copy"]').isDisabled(), false, 'MP3 enables stream copy option');
    await page.getByLabel('Output format', { exact: true }).selectOption('opus');
    assert.equal(await page.getByLabel('Sample rate', { exact: true }).locator('option[value="44100"]').isDisabled(), true, 'Opus disables unsupported 44100 Hz sample rate');
    assert.equal(await page.getByLabel('Sample rate', { exact: true }).locator('option[value="48000"]').isDisabled(), false, 'Opus enables supported 48000 Hz sample rate');
    await page.getByLabel('Output format', { exact: true }).selectOption('mp3');
    await page.waitForFunction(() => window.calls.findLast(call => call.name === 'plan_conversion')?.args.settings.format === 'mp3');
    await page.getByText('1 ready · 0 excluded', { exact: true }).waitFor();
    await page.getByLabel('Processing', { exact: true }).selectOption('encode');
    await page.getByLabel('Audio bitrate (kbps)', { exact: true }).fill('128');
    await page.getByLabel('Preset name', { exact: true }).fill('My audio');
    await page.getByRole('button', { name: 'Save preset', exact: true }).click();
    assert.equal(await preset.inputValue(), 'saved:My audio', 'saving settings selects the saved preset');
    await page.getByLabel('Preset', { exact: true }).selectOption('saved:My audio');
    await page.getByLabel('Audio bitrate (kbps)', { exact: true }).fill('160');
    assert.equal(await preset.locator('option:checked').textContent(), 'Custom');
    await page.getByLabel('Audio bitrate (kbps)', { exact: true }).fill('128');
    assert.equal(await preset.inputValue(), 'saved:My audio', 'saved presets also track changes and restoration');
    await page.getByLabel('Start time', { exact: true }).fill('01:99');
    await page.getByText('Enter valid numeric settings and timestamps', { exact: true }).waitFor();
    assert.equal(await page.getByRole('button', { name: /Convert 0 compatible files/ }).isDisabled(), true);
    await page.getByLabel('Start time', { exact: true }).fill('00:00:01.250');
    await page.getByText('1 ready · 0 excluded', { exact: true }).waitFor();
    await page.getByRole('switch', { name: 'Normalize loudness', exact: true }).click();
    await page.getByRole('switch', { name: 'Measure loudness first', exact: true }).click();
    await page.getByLabel('Loudness target (LUFS)', { exact: true }).fill('-20');
    await page.waitForFunction(() => window.calls.some(call => call.name === 'plan_conversion' && call.args.settings.normalizeTwoPass && call.args.settings.loudnessLufs === -20));
    await page.evaluate(() => {
      window.waiting=['waiting-a','waiting-b','waiting-c'];
      window.jobs=window.waiting.map((id,i)=>({id,input:'C:/media/queued'+i+'.mp4',filename:'queued'+i+'.mp4',container:'mp4',state:'queued',percent:0}));
    });
    const moveUp=page.getByRole('button',{name:'Move queued0.mp4 up',exact:true});
    await moveUp.waitFor();
    assert.equal(await moveUp.isDisabled(),true);
    assert.equal(await page.getByRole('button',{name:'Move queued2.mp4 down',exact:true}).isDisabled(),true);
    await page.getByRole('button',{name:'Move queued1.mp4 up',exact:true}).click();
    await page.waitForFunction(()=>window.waiting[0]==='waiting-b');
    await page.getByRole('button',{name:'Move queued1.mp4 down',exact:true}).click();
    await page.waitForFunction(()=>window.waiting[1]==='waiting-b');
    assert.deepEqual(await page.evaluate(()=>window.calls.filter(call=>call.name==='move_remux_job').map(call=>call.args.movement)),['up','down']);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
    await page.screenshot({ path: path.join(output, 'convert-' + width + '.png'), fullPage: true });
    await page.getByRole('button', { name: 'Change files', exact: true }).click();
    await page.locator('#convert-file-manager').getByRole('button', { name: 'Clear', exact: true }).click();
    assert.equal(await page.locator('[data-conversion-source-path]').count(), 0);
    assert.equal(await page.getByRole('button', { name: 'Choose files', exact: true }).isVisible(), true, 'empty intake returns after clearing all inputs');
    assert.deepEqual(errors, []);
    await page.close();
  }
  console.log('Convert browser checks passed at desktop and mobile widths. Screenshots: ' + output);
} finally { await browser.close(); server.close(); }
