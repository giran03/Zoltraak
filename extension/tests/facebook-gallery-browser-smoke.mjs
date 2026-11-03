// Real browser: cloned Fetch responses, XHR, MAIN/isolated bridge and viewer
// fallback. No Facebook account or live network required.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const browser = await chromium.launch({ headless: true });
const helper = await readFile('extension/facebook-post.js', 'utf8');
const post = 'https://www.facebook.com/user/posts/pfbidGallery';
const photo = (id, set = 'pcb.12345') => ({ __typename: 'Photo', id: String(id),
  url: `https://www.facebook.com/photo/?fbid=${id}&set=${set}`,
  image: { uri: `https://scontent.fbcdn.net/${id}.jpg`, width: 1000, height: 1000 } });
const initial = { __typename: 'Story', post_id: '12345', permalink_url: post,
  attachments: [{ all_subattachments: { count: 7, nodes: [1,2,3,4,5].map((id) => photo(id)) } }],
  comments: [{ media: photo(99) }], token: 'must-not-cross' };
try {
  for (const width of [380, 1100]) {
    const initialDocument = width === 380 ? { data: { node_v2: { ...initial, permalink_url: undefined } } } : initial;
    const page = await browser.newPage({ viewport: { width, height: 800 } });
    const errors = []; let viewerFetches = 0;
    page.on('pageerror', (e) => errors.push(e.message));
    await page.route('**/*', async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname === '/api/graphql/') {
        const id = url.searchParams.get('photo') === '7' ? 7 : 6;
        await route.fulfill({ contentType: 'application/json', body: `for (;;);${JSON.stringify({ data: { mediaSetToken: 'pcb.12345', media: photo(id), nextMediaAfterNodeId: id === 6 ? { __typename: 'Photo', id: '7' } : null }, cookie: 'must-not-cross' })}` });
      } else if (url.pathname === '/photo/') {
        viewerFetches++;
        const id = Number(url.searchParams.get('fbid'));
        const data = { mediaSetToken: 'pcb.12345', media: photo(id), nextMediaAfterNodeId: id < 7 ? { __typename: 'Photo', id: String(id + 1) } : null };
        await route.fulfill({ contentType: 'text/html', body: `<script type="application/json">${JSON.stringify(data)}</script>` });
      } else {
        await route.fulfill({ contentType: 'text/html', body: `<article><a href="${post}">Post</a><script type="application/json">${JSON.stringify(initialDocument)}</script></article>` });
      }
    });
    await page.goto(post);
    await page.addScriptTag({ content: helper });
    await page.evaluate(() => ZoltraakFacebook.installProbe());
    await page.evaluate(async () => {
      const text = await (await fetch('/api/graphql/?photo=6')).text();
      if (!text.includes('must-not-cross')) throw new Error('probe changed the page response');
      await new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest(); xhr.open('GET', '/api/graphql/?photo=7');
        xhr.onload = resolve; xhr.onerror = reject; xhr.send();
      });
    });
    const documents = await page.evaluate((post) => ZoltraakFacebook.collect(post), post);
    const nodes = documents[0].attachments[0].all_subattachments.nodes;
    assert.deepEqual(nodes.map((n) => n.id), ['1','2','3','4','5','6','7']);
    assert.ok(!JSON.stringify(documents).includes('must-not-cross'));
    assert.equal(viewerFetches, 0, 'observed gallery responses avoid extra requests');
    await page.evaluate(() => history.pushState(null, '', '/photo/?fbid=7&set=pcb.12345'));
    const gallery = await page.evaluate(() => ZoltraakFacebook.collect('https://www.facebook.com/permalink.php?story_fbid=12345'));
    assert.equal(gallery[0].attachments[0].all_subattachments.nodes.length, 7, 'gallery navigation retains the bound original Story');
    assert.deepEqual(await page.evaluate(() => ZoltraakFacebook.collect('https://facebook.com/other/posts/pfbidOther')), []);
    assert.deepEqual(errors, []);
    await page.close();

    // Without observed gallery responses, other known photos seed navigation.
    const fallback = await browser.newPage();
    let requests = 0;
    await fallback.route('**/*', (route) => {
      const url = new URL(route.request().url());
      if (url.pathname === '/photo/') {
        requests++;
        const id = Number(url.searchParams.get('fbid'));
        const data = { mediaSetToken: 'pcb.12345', media: photo(id), ...(id >= 5 && id < 7 ? { nextMediaAfterNodeId: { __typename: 'Photo', id: String(id + 1) } } : {}) };
        return route.fulfill({ contentType: 'text/html', body: `<script>${JSON.stringify(data)}</script>` });
      }
      return route.fulfill({ contentType: 'text/html', body: `<script>${JSON.stringify(initialDocument)}</script>` });
    });
    await fallback.goto(post);
    await fallback.addScriptTag({ content: helper });
    const recovered = await fallback.evaluate((post) => ZoltraakFacebook.collect(post), post);
    assert.equal(recovered[0].attachments[0].all_subattachments.nodes.length, 7);
    assert.equal(requests, 7);
    await fallback.close();
  }
  console.log('Facebook gallery Fetch/XHR capture, metadata bridge and missing-navigation recovery passed.');
} finally { await browser.close(); }
