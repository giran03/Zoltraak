// Run with PLAYWRIGHT_MODULE pointing at an existing playwright-core/index.mjs.
// Exercises the actual content script in a real DOM with a fixture runtime;
// worker authorization is covered separately by capture-all.test.mjs.
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import path from 'node:path';

const engines = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const engine = process.env.BROWSER_ENGINE || 'chromium';
const browser = await engines[engine].launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
const errors = [];
page.on('pageerror', (error) => errors.push(error.message));
const artifacts = path.resolve(process.env.RELEASE1_ARTIFACTS || 'scratch/release1-browser');
await mkdir(artifacts, { recursive: true });
try {
  await page.route('https://fixture.test/**', (route) => route.fulfill({
    contentType: 'text/html', body: '<!doctype html><html lang="en"><title>Video fixture</title><body style="background:#181818;padding:32px"><video style="width:640px;height:360px"></video></body></html>',
  }));
  await page.goto('https://fixture.test/');
  await page.evaluate(() => {
    globalThis.fixtureMessages = [];
    globalThis.chrome = {
      runtime: { id: 'fixture', getURL: (name) => `https://fixture.test/${name}`,
        onMessage: { addListener: (listener) => { globalThis.fixtureListener = listener; } },
        sendMessage: async (request) => {
          fixtureMessages.push(request);
          if (request.type === 'zoltraak-health') return { ok: true, up: true };
          if (request.type === 'zoltraak-tab-media') return { ok: true, media: [] };
          if (request.type === 'zoltraak-analyze-best') {
            if (globalThis.fixtureDelay) await new Promise((resolve) => { globalThis.fixtureResolve = resolve; });
            return { ok: false, choices: ['A', 'B'].map((id) => ({
              url: `https://cdn.test/play?id=${id}`,
              metadata: { title: `Clip ${id}`, duration: 10, webpageUrl: `https://fixture.test/clip/${id}`,
                formats: [{ id: '720p', height: 720, videoUrl: `https://cdn.test/play?id=${id}`, hasAudio: true }] },
            })) };
          }
          return { ok: true, id: 'fixture-download' };
        },
      },
      storage: { local: {
        get: async (defaults, callback) => {
          await Promise.resolve();
          const values = { ...defaults, showMediaButton: true, showSessionPill: false };
          callback?.(values);
          return values;
        }, set: async () => {},
      }, onChanged: { addListener() {} } },
    };
  });
  await page.addScriptTag({ content: await readFile('extension/video-identity.js', 'utf8') });
  await page.addScriptTag({ content: await readFile('extension/pill-placement.js', 'utf8') });
  await page.addScriptTag({ content: await readFile('extension/content.js', 'utf8') + `
    globalThis.fixture = { playerBinding, getVideoContext, triggerFindVideo, nearestPermalink,
      async find() { await checkAndRefresh(); return triggerFindVideo(shadow.getElementById('panel'), true); },
      getPanel() { return shadow.getElementById('panel'); } };
  ` });
  await page.evaluate(() => fixture.find());
  const choices = page.locator('#zoltraak-extension-host').locator('.stream-choice');
  await choices.first().waitFor();
  assert.equal(await choices.count(), 2);
  assert.match(await choices.first().innerText(), /Clip A.*10 seconds.*720p/);
  assert.equal(await page.evaluate(() => fixtureMessages.some((m) => m.type === 'zoltraak-download')), false);
  assert.equal(await choices.first().evaluate((button) => button === button.getRootNode().activeElement), true);
  await page.screenshot({ path: path.join(artifacts, `video-choices-${engine}.png`) });
  await choices.nth(1).click();
  await page.locator('#zoltraak-extension-host').locator('.dl-btn').click();
  assert.equal(await page.evaluate(() => fixtureMessages.find((m) => m.type === 'zoltraak-download').format.videoUrl),
    'https://cdn.test/play?id=B');

  // A recycled element keeps its identity but advances its playback generation.
  const stale = await page.evaluate(() => fixture.playerBinding(document.querySelector('video')));
  await page.evaluate(() => document.querySelector('video').dispatchEvent(new Event('loadstart')));
  assert.equal(await page.evaluate((binding) => {
    let result;
    fixtureListener({ type: 'zoltraak-check-video-identity', playerBinding: binding }, {}, (r) => { result = r; });
    return result.ok;
  }, stale), false);

  // A response arriving after the player changes must not render stale choices.
  await page.evaluate(() => { globalThis.fixtureDelay = true; void fixture.find(); });
  await page.waitForFunction(() => Boolean(globalThis.fixtureResolve));
  await page.evaluate(() => {
    document.querySelector('video').dispatchEvent(new Event('emptied'));
    globalThis.fixtureDelay = false;
    fixtureResolve();
  });
  await page.waitForFunction(() => !document.querySelector('#zoltraak-extension-host')?.shadowRoot.querySelector('.stream-choice'));
  assert.equal(await page.evaluate(() => {
    const container = document.createElement('div');
    container.innerHTML = '<video></video><video></video><a href="https://youtube.com/watch?v=neighbor">Shared link</a>';
    document.body.appendChild(container);
    const result = fixture.nearestPermalink(container.querySelector('video'));
    container.remove();
    return result;
  }), null);
  assert.equal(await page.evaluate(() => {
    const container = document.createElement('div');
    container.innerHTML = '<video></video><a href="https://youtube.com/shorts/A">First</a><a href="https://youtube.com/shorts/B">Second</a>';
    document.body.appendChild(container);
    const result = fixture.nearestPermalink(container.querySelector('video'));
    container.remove();
    return result;
  }), null);
  assert.deepEqual(errors, []);
  console.log(`Release 1 ${engine} browser checks passed. Artifacts: ${artifacts}`);
} finally {
  await browser.close();
}
