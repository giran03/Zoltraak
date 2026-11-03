// Run with PLAYWRIGHT_MODULE pointing at an installed playwright-core/index.mjs.
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import path from 'node:path';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const browser = await chromium.launch({ headless: true });
const artifacts = path.resolve('scratch/post-fallback-browser');
await mkdir(artifacts, { recursive: true });
const identity = await readFile('extension/video-identity.js', 'utf8');
const content = await readFile('extension/content.js', 'utf8');
const posts = [
  'https://www.instagram.com/reel/ABC123/',
  'https://www.facebook.com/reel/123',
  'https://x.com/user/status/123',
  'https://www.reddit.com/r/test/comments/abc123/',
  'https://www.tiktok.com/@user/video/123',
];
try {
  for (const [index, url] of posts.entries()) {
    const page = await browser.newPage({ viewport: { width: index === 0 ? 380 : 1100, height: 800 } });
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await page.route('**/*', (route) => route.fulfill({ contentType: 'text/html',
      body: `<!doctype html><html><title>Post recovery fixture</title><body style="margin:0;background:#181818"><article><video style="width:100%;max-width:640px;height:500px"></video><a href="${url}">Post permalink</a></article></body></html>` }));
    await page.goto(url);
    await page.evaluate(() => {
      globalThis.messages = [];
      globalThis.chrome = {
        runtime: { id: 'fixture', getURL: (p) => new URL(p, location.href).href,
          onMessage: { addListener() {} }, sendMessage: async (request) => {
            messages.push(request);
            if (request.type === 'zoltraak-health') return { ok: true, up: true };
            if (request.type === 'zoltraak-tab-media') return { ok: true, media: [] };
            if (request.type === 'zoltraak-analyze-best') return { ok: false, error: 'Instagram extraction failed: media API could not reach Instagram' };
            return { ok: true };
          } },
        storage: { local: { get: async (defaults, callback) => {
          await Promise.resolve();
          const result = { ...defaults, showMediaButton: true, showSessionPill: false };
          callback?.(result); return result;
        }, set: async () => {} }, onChanged: { addListener() {} } },
      };
    });
    await page.addScriptTag({ content: identity });
    await page.addScriptTag({ content: await readFile('extension/pill-placement.js', 'utf8') });
    await page.addScriptTag({ content: content + `
      globalThis.fixtureFind = async () => {
        await checkAndRefresh();
        await triggerFindVideo(shadow.getElementById('panel'), true);
      };` });
    await page.evaluate(() => fixtureFind());
    const button = page.locator('#zoltraak-extension-host').getByRole('button', { name: 'Download post media', exact: true });
    await button.waitFor();
    assert.match(await page.locator('#zoltraak-extension-host').locator('.status-row').innerText(), /find its photos and videos/);
    const bounds = await button.boundingBox();
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= (index === 0 ? 380 : 1100), 'recovery button fits viewport');
    if (index === 0) await page.screenshot({ path: path.join(artifacts, 'instagram-recovery.png') });
    await button.click();
    assert.equal(await page.evaluate(() => messages.find((m) => m.type === 'zoltraak-prefill-post').url), url);
    await page.locator('#zoltraak-extension-host').getByRole('button', { name: 'Opened in Images' }).waitFor();
    assert.deepEqual(errors, []);
    await page.close();
  }
  console.log('Post recovery browser checks passed for Instagram, Facebook, X, Reddit and TikTok.');
} finally { await browser.close(); }
