// PLAYWRIGHT_MODULE points at an installed playwright-core/index.mjs.
// Exercise real right-click events and post/player DOM boundaries offline.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const browser = await chromium.launch({ headless: true });
const post = 'https://www.facebook.com/juliarusziell/posts/pfbid0umUguDQtAMzFeybyTtq4P4Sgno5Nwgfrwj5hkaHx9ZikgRoj81D8mQhTwq5v51hml';
const share = 'https://www.facebook.com/share/p/1DmzsRpa8N/';
const watch = 'https://www.facebook.com/watch/?v=1234567890';
const identity = await readFile('extension/video-identity.js', 'utf8');
const content = await readFile('extension/content.js', 'utf8');
try {
  for (const width of [380, 1100]) {
    const page = await browser.newPage({ viewport: { width, height: 800 } });
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await page.route('**/*', (route) => route.fulfill({ contentType: 'text/html', body: `<!doctype html><html><body>
      <main id="feed">
        <article id="post"><div id="container"><a id="profile" href="/juliarusziell">Profile</a>
          <div id="photo"><a href="/photo/?fbid=9876543210"><span id="image">Photo</span></a></div>
          <div id="player"><video src="https://video.fbcdn.net/picture.mp4?oh=a%2Fb+z&bytestart=0&byteend=99"></video></div>
          <span id="text">Post text</span><a id="timestamp" href="${post}?__cft__=tracking">Timestamp</a></div></article>
        <article id="neighbor"><video></video><a href="/user/posts/pfbidNeighbor">Other post</a></article>
        <article id="empty"><span>Missing permalink</span></article>
        <article id="shared"><div><a href="${share}">Share</a></div></article>
        <article id="carousel"><video></video><video></video><a href="${post}">Post</a></article>
        <article id="legacy" data-ft='{"top_level_post_id":"12345","content_owner_id_new":"67890"}'><span>Legacy post</span></article>
      </main></body></html>` }));
    await page.goto('https://www.facebook.com/');
    await page.evaluate(() => {
      globalThis.chrome = {
        runtime: { id: 'fixture', getURL: (p) => new URL(p, location.href).href,
          onMessage: { addListener: (listener) => { globalThis.fixtureListener = listener; } },
          sendMessage: async (request) => request.type === 'zoltraak-tab-media'
            ? { ok: true, media: [{ url: 'https://video.fbcdn.net/unrelated.mp4', kind: 'file' }] } : { ok: true, up: true } },
        storage: { local: { get: async (defaults, callback) => {
          await Promise.resolve();
          const prefs = { ...defaults, showMediaButton: false, showSessionPill: false };
          callback?.(prefs); return prefs;
        }, set: async () => {} }, onChanged: { addListener() {} } },
      };
    });
    await page.addScriptTag({ content: identity });
    await page.addScriptTag({ content: await readFile('extension/pill-placement.js', 'utf8') });
    await page.addScriptTag({ content: content + `
      globalThis.fixture = { nearestPermalink, candidateTargets, directSrc };
      globalThis.contextPost = (id) => {
        document.getElementById(id).dispatchEvent(new MouseEvent('contextmenu', { bubbles: true }));
        let result;
        fixtureListener({ type: 'zoltraak-post-permalink' }, {}, (r) => { result = r; });
        return result.url;
      };
    ` });
    for (const id of ['post', 'container', 'profile', 'photo', 'image', 'player', 'text', 'timestamp']) {
      assert.equal(await page.evaluate((id) => contextPost(id), id), post, `${id} at ${width}px`);
    }
    assert.equal(await page.evaluate(() => contextPost('shared')), share);
    assert.equal(await page.evaluate(() => contextPost('feed')), null, 'feed cannot inherit its first post');
    assert.equal(await page.evaluate(() => contextPost('empty')), null, 'missing permalink cannot leak into neighbor');
    assert.equal(await page.evaluate(() => contextPost('carousel')), post);
    assert.equal(await page.evaluate(() => contextPost('legacy')), 'https://www.facebook.com/permalink.php?story_fbid=12345&id=67890');
    assert.deepEqual(await page.evaluate(() => fixture.candidateTargets(document.querySelector('#post video'))), [post]);
    assert.equal(await page.evaluate(() => fixture.directSrc(document.querySelector('#post video'))),
      'https://video.fbcdn.net/picture.mp4?oh=a%2Fb+z');
    await page.evaluate((watch) => {
      const a = document.createElement('a'); a.href = watch; a.textContent = 'Video permalink';
      document.getElementById('player').appendChild(a);
    }, watch);
    assert.deepEqual(await page.evaluate(() => fixture.candidateTargets(document.querySelector('#post video'))), [watch]);
    assert.equal(await page.evaluate(() => contextPost('player')), post, 'post action keeps the surrounding Story');
    await page.evaluate(() => {
      const gallery = document.createElement('div'); gallery.id = 'gallery';
      gallery.innerHTML = '<a href="/photo/?fbid=99&set=pcb.12345"><span id="gallery-photo">Gallery photo</span></a>';
      document.body.appendChild(gallery);
    });
    assert.equal(await page.evaluate(() => contextPost('gallery-photo')), 'https://www.facebook.com/permalink.php?story_fbid=12345', 'gallery photo recovers its parent post');
    await page.evaluate(() => document.getElementById('player').setAttribute('data-video-id', '9876543210'));
    assert.deepEqual(await page.evaluate(() => fixture.candidateTargets(document.querySelector('#post video'))),
      ['https://www.facebook.com/watch/?v=9876543210']);
    await page.evaluate(() => {
      const a = document.createElement('a'); a.href = '/other/posts/pfbidConflict';
      document.getElementById('container').appendChild(a);
    });
    assert.equal(await page.evaluate(() => contextPost('text')), null, 'conflicting post links are not guessed');
    assert.deepEqual(errors, []);
    await page.close();
  }
  console.log('Facebook right-click and overlay browser regressions passed at 380px and 1100px.');
} finally { await browser.close(); }
