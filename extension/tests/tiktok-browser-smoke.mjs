// Real DOM boundaries, adjacent player controls and XHR-backed blob ownership.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const browser = await chromium.launch({ headless: true });
const post = 'https://www.tiktok.com/@creator/video/7671362008486251790';
const media = 'https://v16-webapp-prime.tiktok.com/video/clip.mp4?token=a%2Fb+z';
const [identity, content, generic, probe, movie] = await Promise.all([
  readFile('extension/video-identity.js', 'utf8'), readFile('extension/content.js', 'utf8'),
  readFile('extension/generic-media.js', 'utf8'), readFile('extension/media-probe.js', 'utf8'),
  readFile('engine/core/tests/fixtures/verified-av.mp4'),
]);
try {
  for (const width of [380, 1100]) {
    const page = await browser.newPage({ viewport: { width, height: 800 } });
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.route('**/*', (route) => route.request().url() === media
      ? route.fulfill({ contentType: 'video/mp4', body: movie, headers: { 'access-control-allow-origin': '*' } })
      : route.fulfill({ contentType: 'text/html', body: `<!doctype html><body>
        <main id="feed"><article id="post" data-e2e="recommend-list-item-container">
          <div id="player"><video id="clip" muted style="width:280px;height:350px"></video></div>
          <div id="controls"><a id="profile" href="/@creator">Creator</a><span id="share">Share</span>
            <input value="${post}?is_from_webapp=1&sender_device=pc"></div>
        </article><article id="neighbor"><video></video><a href="/@other/video/7777777777777777777">Other</a></article>
        <article id="missing"><span>No post link</span></article></main></body>` }));
    await page.addInitScript({ content: probe });
    await page.goto('https://www.tiktok.com/foryou');
    await page.evaluate(() => {
      globalThis.fixtureListeners = [];
      globalThis.messages = [];
      globalThis.chrome = {
        runtime: { id: 'fixture', getURL: (p) => new URL(p, location.href).href,
          onMessage: { addListener: (listener) => fixtureListeners.push(listener) },
          sendMessage: async (request) => {
            messages.push(request);
            if (request.type === 'zoltraak-health') return { ok: true, up: true };
            if (request.type === 'zoltraak-tab-media') return { ok: true, media: [{ url: 'https://cdn.test/neighbor.mp4', kind: 'file' }] };
            if (request.type === 'zoltraak-analyze-best') return { ok: true, metadata: {
              title: 'Selected TikTok', webpageUrl: request.videoContext.permalink,
              formats: [{ id: 'tiktok', container: 'mp4', videoUrl: request.videoContext.currentSrc, hasAudio: true }],
            } };
            return { ok: true };
          } },
        storage: { local: { get: async (defaults, callback) => {
          await Promise.resolve();
          const prefs = { ...defaults, showMediaButton: true, showSessionPill: false };
          callback?.(prefs); return prefs;
        }, set: async () => {} }, onChanged: { addListener() {} } },
      };
    });
    await page.addScriptTag({ content: identity });
    await page.addScriptTag({ content: await readFile('extension/pill-placement.js', 'utf8') });
    await page.addScriptTag({ content: generic });
    await page.addScriptTag({ content: content + `
      globalThis.fixture = { candidateTargets, getVideoContext, videoPermalink };
      globalThis.contextPost = (id) => {
        document.getElementById(id).dispatchEvent(new MouseEvent('contextmenu', { bubbles: true }));
        let result;
        for (const listener of fixtureListeners) listener({ type: 'zoltraak-post-permalink' }, {}, (r) => { result = r; });
        return result.url;
      };
      globalThis.findSelected = async () => { await checkAndRefresh(); await triggerFindVideo(shadow.getElementById('panel'), true); };
    ` });
    for (const id of ['player', 'clip', 'controls', 'profile', 'share']) {
      assert.equal(await page.evaluate((id) => contextPost(id), id), post, `${id} at ${width}px`);
    }
    assert.equal(await page.evaluate(() => contextPost('feed')), null);
    assert.equal(await page.evaluate(() => contextPost('missing')), null);
    assert.deepEqual(await page.evaluate(() => fixture.candidateTargets(document.getElementById('clip'))), [post]);
    await page.evaluate(async (media) => {
      const response = await new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest(); xhr.open('GET', media); xhr.responseType = 'blob';
        xhr.onload = () => resolve(xhr.response); xhr.onerror = reject; xhr.send();
      });
      const wrapped = new Blob([response], { type: 'video/mp4' });
      document.getElementById('clip').src = URL.createObjectURL(wrapped);
    }, media);
    await page.waitForFunction((media) => ZoltraakGenericMedia.sourcesFor(document.getElementById('clip')).some((s) => s.url === media), media);
    assert.deepEqual(await page.evaluate(() => fixture.candidateTargets(document.getElementById('clip'))), [post, media]);
    assert.equal(await page.evaluate(() => fixture.getVideoContext(document.getElementById('clip')).currentSrc), media);
    await page.evaluate(() => findSelected());
    const request = await page.evaluate(() => messages.find((m) => m.type === 'zoltraak-analyze-best'));
    assert.deepEqual(request.candidates, [post, media]);
    assert.equal(request.videoContext.permalink, post);
    assert.equal(request.videoContext.currentSrc, media);
    assert.ok(await page.locator('#zoltraak-extension-host').getByText('Selected TikTok', { exact: true }).count());
    await page.evaluate(() => {
      const input = document.createElement('input'); input.value = 'https://www.tiktok.com/@other/video/7777777777777777777';
      document.getElementById('controls').append(input);
    });
    assert.equal(await page.evaluate(() => contextPost('share')), null, 'conflicting links are not guessed');
    assert.deepEqual(errors, []);
    await page.close();
  }
  console.log('TikTok adjacent controls, feed isolation, blob playback and overlay checks passed at 380px and 1100px.');
} finally { await browser.close(); }
