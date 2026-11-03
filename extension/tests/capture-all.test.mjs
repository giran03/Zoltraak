// The browser copy is cancelled only after an authenticated connection check
// and durable draft storage. No engine download starts until the user submits.

import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';
import { test } from 'node:test';

// background.js is an ES module whose only import is ./api.js. Swapping that
// for a stub the test drives is enough to exercise the real listener — the
// alternative, reaching a live engine over HTTP, is the thing under test.
registerHooks({
  resolve(specifier, context, next) {
    if (specifier.endsWith('api.js')) return { url: 'test:api-stub', shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url !== 'test:api-stub') return next(url, context);
    const names = [
      'ensurePaired', 'health', 'enqueue', 'analyzeVideo', 'startVideoDownload',
      'getSnapshot', 'revealTarget', 'openTarget', 'setDownloadState',
      'removeDownload', 'cookieHeader', 'bestFormat', 'bestCompleteFormat',
      'prefillDownload', 'prefillVideo', 'prefillImages', 'prefillMusic', 'pushSession',
      'youtubeSessionCookie', 'getToken', 'verify', 'apiFetch',
      'setCookieSourceState', 'canUseBrowserCookies',
      'extensionStatusCursor', 'setExtensionStatusCursor',
    ];
    return {
      format: 'module',
      shortCircuit: true,
      source: 'export const ENGINE_ORIGIN = "http://127.0.0.1:1450";\n' + names
        .map((n) => n === 'apiFetch'
          ? `export const ${n} = (...a) => a[0].startsWith('/extension/status') ? new Promise(() => {}) : globalThis.__api.${n}(...a);`
          : `export const ${n} = (...a) => globalThis.__api.${n}(...a);`)
        .join('\n'),
    };
  },
});

/**
 * Point the api.js stub at one case's behaviour, over a full set of harmless
 * defaults. capture() schedules a badge refresh 2.5s out — long after the case
 * that triggered it has ended — and that refresh calls back into whatever
 * `__api` holds by then, which is some later case's stub.
 */
function useApi(overrides) {
  globalThis.__api = {
    health: async () => false,
    ensurePaired: async () => true,
    apiFetch: async () => ({ ok: true, json: async () => [] }),
    getToken: async () => 'token',
    verify: async () => true,
    cookieHeader: async () => null,
    pushSession: async () => ({ ok: true }),
    setCookieSourceState: async () => false,
    canUseBrowserCookies: async () => true,
    getSnapshot: async () => ({ ok: true, items: [], videoJobs: [] }),
    enqueue: async () => assert.fail('enqueue was not expected in this case'),
    ...overrides,
  };
}

/** A chrome just complete enough for background.js to load and fire events. */
function fakeChrome() {
  const store = { captureAll: true };
  const listeners = {};
  const event = (name) => {
    const handlers = [];
    const dispatch = (...args) => {
      let result;
      for (const handler of handlers) {
        const next = handler(...args);
        if (next !== undefined) result = next;
      }
      return result;
    };
    return {
      addListener: (fn) => {
        handlers.push(fn);
        listeners[name] = dispatch;
      },
      removeListener: (fn) => handlers.splice(handlers.indexOf(fn), 1),
      hasListener: (fn) => handlers.includes(fn),
    };
  };
  const chrome = {
    listeners,
    store,
    tabs: {
      query: async () => [{ id: 3, url: 'https://example.com/' }],
      get: async (id) => ({ id, active: true, url: 'https://example.com/' }),
      onActivated: event('tabActivated'),
      onUpdated: event('tabUpdated'),
      sendMessage: async () => ({ ok: true }),
      create: async () => ({ id: 4 }),
      reload: async (id) => { chrome.calls.reload.push(id); },
    },
    calls: { cancel: [], erase: [], reload: [], menuUpdates: [], menus: [], menuResets: 0 },
    runtime: {
      onInstalled: event('onInstalled'),
      onStartup: event('onStartup'),
      onMessage: event('onMessage'),
      getURL: (path) => `chrome-extension://test/${path}`,
      lastError: null,
    },
    contextMenus: {
      onClicked: event('menuClick'),
      create: (properties, callback) => { chrome.calls.menus.push(properties); callback?.(); },
      update: (id, properties, callback) => {
        chrome.calls.menuUpdates.push({ id, ...properties });
        callback?.();
      },
      removeAll: (cb) => { chrome.calls.menuResets++; chrome.calls.menus.length = 0; cb?.(); },
    },
    action: { setBadgeText() {}, setBadgeBackgroundColor() {}, setTitle() {} },
    notifications: { create() {} },
    storage: {
      local: {
        get: async (keys) => {
          if (typeof keys === 'string') return { [keys]: store[keys] };
          if (Array.isArray(keys)) return Object.fromEntries(keys.map((k) => [k, store[k]]));
          if (keys && typeof keys === 'object') {
            return Object.fromEntries(Object.entries(keys).map(([k, d]) => [k, store[k] ?? d]));
          }
          return { ...store };
        },
        set: async (values) => { Object.assign(store, values); },
        remove: async (key) => { delete store[key]; },
      },
      session: { get: async () => ({}), set: async () => {}, remove: async () => {} },
    },
    downloads: {
      onCreated: event('downloadCreated'),
      onChanged: event('downloadChanged'),
      cancel: async (id) => { chrome.calls.cancel.push(id); },
      erase: async ({ id }) => { chrome.calls.erase.push(id); },
      download: async () => 1,
    },
    webRequest: {
      onBeforeRequest: event('beforeRequest'),
      onHeadersReceived: event('headersReceived'),
    },
  };
  chrome.storage.session = chrome.storage.local;
  return chrome;
}

async function loadBackground(configure) {
  const chrome = fakeChrome();
  configure?.(chrome);
  globalThis.chrome = chrome;
  // Cache-busted so each case gets its own module instance and listener set.
  await import(`../background.js?case=${Math.random()}`);
  await new Promise((resolve) => setImmediate(resolve));
  return chrome;
}

test('reload preparation refreshes only tabs with an injected content script', async () => {
  useApi({ health: async () => false });
  const chrome = await loadBackground();
  const prepared = await message(chrome, { type: 'zoltraak-prepare-reload' });
  assert.equal(prepared.ok, true);
  assert.deepEqual(chrome.store.zoltraakRefreshTabsAfterReload, [3]);
});

test('a restarted worker consumes the queued tab refresh exactly once', async () => {
  useApi({ health: async () => false });
  const chrome = await loadBackground((candidate) => {
    candidate.store.zoltraakRefreshTabsAfterReload = [3];
  });
  assert.deepEqual(chrome.calls.reload, [3]);
  assert.equal(chrome.store.zoltraakRefreshTabsAfterReload, undefined);
});

test('the post context menu sends the selected permalink to Images', async () => {
  let prefilled;
  useApi({
    cookieHeader: async () => 'session=private',
    prefillImages: async (...args) => {
      prefilled = args;
      return { ok: true };
    },
  });
  const chrome = await loadBackground((candidate) => {
    candidate.tabs.sendMessage = async (_tabId, message) =>
      message.type === 'zoltraak-post-permalink'
        ? { url: 'https://www.instagram.com/p/selected/' }
        : { ok: true };
  });

  await chrome.listeners.menuClick(
    { menuItemId: 'zoltraak-post', frameId: 0, pageUrl: 'https://www.instagram.com/' },
    { id: 3, url: 'https://www.instagram.com/' },
  );

  assert.deepEqual(prefilled, [
    'https://www.instagram.com/p/selected/',
    [['Cookie', 'session=private']],
    'https://www.instagram.com/',
  ]);
});

test('the post context menu prefers the exact clicked Instagram link', async () => {
  let prefilled;
  let cookieRequest;
  useApi({
    cookieHeader: async (...args) => {
      cookieRequest = args;
      return 'session=private';
    },
    prefillImages: async (...args) => {
      prefilled = args;
      return { ok: true };
    },
  });
  const chrome = await loadBackground((candidate) => {
    candidate.tabs.sendMessage = async () => ({
      url: 'https://www.instagram.com/p/DcurpATidq7/',
    });
  });

  await chrome.listeners.menuClick(
    {
      menuItemId: 'zoltraak-post',
      frameId: 0,
      linkUrl: 'https://www.instagram.com/p/DcurpATidq7/c/18175797727434563/',
      pageUrl: 'https://www.instagram.com/p/DcurpATidq7/',
    },
    { id: 3, url: 'https://www.instagram.com/p/DcurpATidq7/', cookieStoreId: 'firefox-container-2' },
  );

  assert.deepEqual(cookieRequest, [
    'https://www.instagram.com/p/DcurpATidq7/c/18175797727434563/',
    'firefox-container-2',
  ]);
  assert.equal(prefilled[0], 'https://www.instagram.com/p/DcurpATidq7/c/18175797727434563/');
});

test('Instagram audio links resolve to the selected reel in the post menu', async () => {
  const reel = 'https://www.instagram.com/reel/DdKrjQHNY86/?utm_source=ig_web_copy_link&stkn=NTc4MTIwNjQ2YQ==';
  let prefilled;
  useApi({ prefillImages: async (url) => { prefilled = url; return { ok: true }; } });
  const chrome = await loadBackground((candidate) => {
    candidate.tabs.sendMessage = async () => ({ url: reel });
  });
  await chrome.listeners.menuClick({ menuItemId: 'zoltraak-post',
    linkUrl: 'https://www.instagram.com/reels/audio/28686481800976139/', pageUrl: reel }, { id: 3, url: reel });
  assert.equal(prefilled, reel);
});

test('verified Instagram post formats stop analysis before unrelated captures', async () => {
  const reel = 'https://www.instagram.com/reel/DdKrjQHNY86/';
  const calls = [];
  useApi({ analyzeVideo: async (url) => {
    calls.push(url);
    return { ok: true, metadata: { title: 'Reel', webpageUrl: reel, duration: 10,
      formats: [{ id: '1080p', videoUrl: 'https://cdn.test/hd.mp4', height: 1080, hasAudio: true },
        { id: '720p', videoUrl: 'https://cdn.test/selected.mp4', height: 720, hasAudio: true }] } };
  } });
  const chrome = await loadBackground();
  const result = await new Promise((resolve) => chrome.listeners.onMessage({ type: 'zoltraak-analyze-best',
    candidates: [reel, `${reel}?tracking=1`, 'https://cdn.test/unrelated.mp4'],
    videoContext: { permalink: reel, duration: 10, currentSrc: 'https://cdn.test/selected.mp4' } },
    { tab: { id: 3 }, frameId: 0 }, resolve));
  assert.equal(result.ok, true);
  assert.deepEqual(calls, [reel]);
  assert.equal(result.metadata.formats.length, 2);
  assert.equal(result.metadata.formats[0].id, '1080p');
});

test('Facebook profile and attachment clicks keep the post and original frame', async () => {
  const post = 'https://www.facebook.com/user/posts/pfbidSelected';
  let prefilled, scope;
  useApi({ prefillImages: async (url) => { prefilled = url; return { ok: true }; } });
  const chrome = await loadBackground((candidate) => {
    candidate.tabs.sendMessage = async (_tab, request, frame) => {
      if (request.type === 'zoltraak-post-permalink') { scope = frame; return { url: post }; }
      return { ok: true };
    };
  });
  for (const linkUrl of ['https://facebook.com/user', 'https://facebook.com/photo/?fbid=123', 'https://cdn.test/photo.jpg']) {
    await chrome.listeners.menuClick({ menuItemId: 'zoltraak-post', linkUrl,
      frameId: 5, pageUrl: 'https://facebook.com/' }, { id: 3, url: 'https://facebook.com/' });
    assert.equal(prefilled, post);
    assert.deepEqual(scope, { frameId: 5 });
  }
});

test('Facebook post actions forward browser gallery metadata from the selected frame and profile', async () => {
  const post = 'https://www.facebook.com/user/posts/pfbidSelected';
  const documents = [{ __typename: 'Story', permalink_url: post, attachments: [] }];
  let prefilled; const scopes = [];
  useApi({ canUseBrowserCookies: async () => true,
    prefillImages: async (...args) => { prefilled = args; return { ok: true }; } });
  const chrome = await loadBackground((c) => {
    c.tabs.sendMessage = async (tabId, message, scope) => {
      scopes.push([tabId, message.type, scope]);
      return message.type === 'zoltraak-facebook-attachments' ? { browserDocuments: documents } : { url: post };
    };
  });
  await chrome.listeners.menuClick({ menuItemId: 'zoltraak-post', frameId: 5, pageUrl: 'https://facebook.com/' }, { id: 3, url: 'https://facebook.com/' });
  assert.deepEqual(prefilled[3], documents);
  assert.ok(scopes.some(([id, type, scope]) => id === 3 && type === 'zoltraak-facebook-attachments' && scope.frameId === 5));
});

test('Facebook post video analysis retains paired formats and skips captured MP4 tracks', async () => {
  const post = 'https://www.facebook.com/user/posts/pfbidSelected';
  const calls = [];
  const format = { id: 'dash-1080p', videoUrl: 'https://video.fbcdn.net/video.mp4',
    audioUrl: 'https://video.fbcdn.net/audio.mp4', height: 1080, requiresMerge: true, hasAudio: false };
  useApi({ analyzeVideo: async (url) => {
    calls.push(url);
    return { ok: true, metadata: { title: 'Selected post', webpageUrl: post, formats: [format] } };
  } });
  const chrome = await loadBackground();
  const result = await message(chrome, { type: 'zoltraak-analyze-best',
    candidates: [post, 'https://video.fbcdn.net/audio.mp4'],
    videoContext: { permalink: post, currentSrc: 'blob:selected' } });
  assert.equal(result.ok, true);
  assert.deepEqual(calls, [post]);
  assert.deepEqual(result.metadata.formats, [format]);
});

test('X and Twitter show only the post context-menu action', async () => {
  useApi({ health: async () => false });
  const chrome = await loadBackground();
  chrome.calls.menuUpdates.length = 0;

  chrome.listeners.tabUpdated(
    3,
    { url: 'https://x.com/home' },
    { id: 3, active: true, url: 'https://x.com/home' },
  );
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(
    chrome.calls.menuUpdates,
    [{ id: 'zoltraak-post', visible: true }, ...['zoltraak-link', 'zoltraak-media', 'zoltraak-music', 'zoltraak-music-link']
      .map((id) => ({ id, visible: false }))],
  );

  chrome.calls.menuUpdates.length = 0;
  chrome.listeners.tabUpdated(
    3,
    { url: 'https://example.com/' },
    { id: 3, active: true, url: 'https://example.com/' },
  );
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(chrome.calls.menuUpdates.every(({ visible }) => visible));
});

test('install and startup rebuild menus without a video-only action', async () => {
  useApi({ health: async () => false,
    prefillDownload: async () => assert.fail('removed menu must not dispatch a download') });
  const chrome = await loadBackground();
  for (const event of ['onInstalled', 'onStartup']) {
    const resets = chrome.calls.menuResets;
    chrome.listeners[event]();
    assert.equal(chrome.calls.menuResets, resets + 1);
    assert.deepEqual(chrome.calls.menus.map((m) => m.id),
      ['zoltraak-link', 'zoltraak-media', 'zoltraak-post', 'zoltraak-music', 'zoltraak-music-link']);
    assert.ok(chrome.calls.menus.every((m) => !m.contexts.includes('video')));
  }
  await chrome.listeners.menuClick({ menuItemId: 'zoltraak-video', srcUrl: 'https://cdn.test/video.mp4' }, { id: 3 });
});

test('a download is left to the browser while the app is unreachable', async () => {
  useApi({
    health: async () => false,
    enqueue: async () => assert.fail('enqueue must not be attempted with the app down'),
  });
  const chrome = await loadBackground();

  await chrome.listeners.downloadCreated({ id: 7, url: 'https://example.com/report.pdf' });

  assert.deepEqual(chrome.calls.cancel, [], 'cancelled with nowhere to hand the download off to');
  assert.deepEqual(chrome.calls.erase, []);
});

test('an HLS segment is left alone too, rather than cancelled for nothing', async () => {
  useApi({ health: async () => false });
  const chrome = await loadBackground();

  await chrome.listeners.downloadCreated({ id: 8, url: 'https://cdn.example.com/v/seg-9.ts' });

  assert.deepEqual(chrome.calls.cancel, [], 'segments cancelled while capture-all was inert');
});

const item = { id: 10, url: 'https://example.com/report.pdf', referrer: 'https://example.com/', filename: 'report.pdf', tabId: 3 };
const pending = (chrome) => Object.entries(chrome.store).filter(([key]) => key.startsWith('intercept:')).map(([, d]) => d);
const message = (chrome, body, tabId = 3) => new Promise((resolve) => chrome.listeners.onMessage(body, { tab: { id: tabId }, url: 'https://example.com/' }, resolve));

test('capture persists a draft and cancels without starting the engine', async () => {
  useApi({ health: async () => true, cookieHeader: async () => 'sid=1' });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  assert.deepEqual(chrome.calls.cancel, [10]);
  assert.deepEqual(chrome.calls.erase, [10]);
  assert.equal(pending(chrome).length, 1);
  assert.deepEqual(pending(chrome)[0].headers, [['Cookie', 'sid=1']]);
});

test('unpaired and disabled capture leave browser downloads untouched', async () => {
  useApi({ health: async () => true, ensurePaired: async () => false });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  assert.deepEqual(chrome.calls.cancel, []);
  chrome.store.captureAll = false;
  useApi({ health: async () => true });
  await chrome.listeners.downloadCreated(item);
  assert.deepEqual(chrome.calls.cancel, []);
});

test('capture needs no license: a stale unlicensed flag from older builds is ignored', async () => {
  useApi({ health: async () => true });
  const chrome = await loadBackground();
  chrome.store.desktopAppLicensed = false;
  await chrome.listeners.downloadCreated(item);
  assert.notEqual(chrome.store.lastInterception?.reason ?? '', 'The desktop app is unlicensed. Activate a license to enable capture.');
  assert.doesNotMatch(chrome.store.lastInterception?.reason ?? '', /licens/i);
});

test('later forwards edits atomically as paused, and failed submission retains the draft', async () => {
  const enqueued = [];
  useApi({ health: async () => true, enqueue: async (body) => { enqueued.push(body); return { ok: false, error: 'offline' }; } });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  const request = { type: 'zoltraak-intercept-submit', id: pending(chrome)[0].id, filename: 'edited.pdf', directory: 'C:\\Chosen', later: true };
  assert.equal((await message(chrome, request)).ok, false);
  assert.equal(pending(chrome).length, 1);
  assert.equal(enqueued[0].paused, true);
  assert.equal(enqueued[0].filename, 'edited.pdf');
  assert.equal(enqueued[0].directory, 'C:\\Chosen');
  globalThis.__api.enqueue = async () => ({ ok: true, id: 'dl-1' });
  assert.equal((await message(chrome, request)).ok, true);
  assert.equal(pending(chrome).length, 0);
});

test('drafts are tab-scoped, reject paths as filenames, and support cancellation', async () => {
  useApi({ health: async () => true });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  const id = pending(chrome)[0].id;
  assert.equal((await message(chrome, { type: 'zoltraak-intercept-cancel', id }, 9)).ok, false);
  assert.equal((await message(chrome, { type: 'zoltraak-intercept-submit', id, filename: '../escape' })).ok, false);
  assert.equal((await message(chrome, { type: 'zoltraak-intercept-cancel', id })).ok, true);
  assert.equal(pending(chrome).length, 0);
});

test('Chromium cancels while filename determination is held, then releases suggest exactly once', async () => {
  useApi({ health: async () => true });
  const chrome = fakeChrome();
  chrome.downloads.onDeterminingFilename = { addListener: (fn) => { chrome.listeners.determine = fn; } };
  globalThis.chrome = chrome;
  await import(`../background.js?case=${Math.random()}`);
  assert.equal(chrome.listeners.downloadCreated, undefined);
  let suggested = 0;
  await new Promise((resolve) => {
    assert.equal(chrome.listeners.determine(item, () => {
      suggested++;
      assert.deepEqual(chrome.calls.cancel, [10]);
      resolve();
    }), true);
  });
  assert.equal(suggested, 1);
});

test('Firefox blocks attachment responses before creating a native download', async () => {
  useApi({ health: async () => true });
  let receive;
  globalThis.browser = { webRequest: { onHeadersReceived: { addListener: (fn) => { receive = fn; } } } };
  try {
    const chrome = await loadBackground();
    const details = { method: 'GET', tabId: 3, url: item.url, responseHeaders: [
      { name: 'Content-Disposition', value: "attachment; filename*=UTF-8''report%20final.pdf" },
    ] };
    assert.deepEqual(await receive(details), { cancel: true });
    assert.equal(pending(chrome)[0].filename, 'report final.pdf');
    assert.deepEqual(chrome.calls.cancel, [], 'no native download should exist yet');
    globalThis.__api.health = async () => false;
    assert.deepEqual(await receive(details), {});
    assert.deepEqual(await receive({ ...details, method: 'POST' }), {});
  } finally { delete globalThis.browser; }
});

test('Download Now submits without pausing and retains captured request headers', async () => {
  let request;
  useApi({ health: async () => true, cookieHeader: async () => 'session=private',
    enqueue: async (body) => { request = body; return { ok: true, id: 'now-1' }; } });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  const listed = await message(chrome, { type: 'zoltraak-intercept-list' });
  assert.equal(listed.drafts[0].headers, undefined);
  assert.equal((await message(chrome, { type: 'zoltraak-intercept-submit', id: pending(chrome)[0].id, filename: 'report.pdf' })).ok, true);
  assert.equal(request.paused, false);
  assert.deepEqual(request.headers, [['Cookie', 'session=private']]);
  assert.equal(request.referrer, item.referrer);
});

test('intercept-list delivers defaultDirectory and cancel-all empties pending drafts', async () => {
  useApi({
    health: async () => true,
    apiFetch: async (path) => {
      if (path === '/categories') return { ok: true, json: async () => [{ id: 1, name: 'Docs', directory: 'C:\\Docs' }] };
      if (path === '/settings') return { ok: true, json: async () => ({ defaultDirectory: 'C:\\Users\\Downloads' }) };
      return { ok: false };
    },
  });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  await chrome.listeners.downloadCreated({ id: 11, url: 'https://example.com/sheet.xlsx', filename: 'sheet.xlsx', tabId: 3 });
  assert.equal(pending(chrome).length, 2);

  const listed = await message(chrome, { type: 'zoltraak-intercept-list' });
  assert.equal(listed.defaultDirectory, 'C:\\Users\\Downloads');
  assert.equal(listed.categories.length, 1);
  assert.equal(listed.drafts.length, 2);

  const cancelResult = await message(chrome, { type: 'zoltraak-intercept-cancel-all' });
  assert.equal(cancelResult.ok, true);
  assert.equal(cancelResult.count, 2);
  assert.equal(pending(chrome).length, 0);
});

test('directory path with quotes is trimmed and forwarded to enqueue', async () => {
  let request;
  useApi({
    health: async () => true,
    enqueue: async (body) => { request = body; return { ok: true, id: 'quoted-1' }; },
  });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  const id = pending(chrome)[0].id;
  const res = await message(chrome, {
    type: 'zoltraak-intercept-submit',
    id,
    filename: 'report.pdf',
    directory: '"C:\\Program Files\\Custom Folder"',
  });
  assert.equal(res.ok, true);
  assert.equal(request.directory, 'C:\\Program Files\\Custom Folder');
});

test('intercept-list prefers resolvedDefaultDirectory', async () => {
  useApi({
    health: async () => true,
    apiFetch: async (path) => {
      if (path === '/settings') return {
        ok: true,
        json: async () => ({
          defaultDirectory: null,
          resolvedDefaultDirectory: 'C:\\Users\\User\\Downloads',
        }),
      };
      return { ok: false };
    },
  });
  const chrome = await loadBackground();
  const listed = await message(chrome, { type: 'zoltraak-intercept-list' });
  assert.equal(listed.defaultDirectory, 'C:\\Users\\User\\Downloads');
});

test('remember flag persists directory to storage and updates desktop settings', async () => {
  let putPayload = null;
  useApi({
    health: async () => true,
    enqueue: async () => ({ ok: true, id: 'rem-1' }),
    apiFetch: async (path, init) => {
      if (path === '/settings' && !init) {
        return { ok: true, json: async () => ({ theme: 'dark', defaultDirectory: 'C:\\Old' }) };
      }
      if (path === '/settings' && init?.method === 'PUT') {
        putPayload = JSON.parse(init.body);
        return { ok: true, json: async () => putPayload };
      }
      return { ok: false };
    },
  });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  const id = pending(chrome)[0].id;
  const res = await message(chrome, {
    type: 'zoltraak-intercept-submit',
    id,
    filename: 'report.pdf',
    directory: 'D:\\SavedDownloads',
    remember: true,
  });
  assert.equal(res.ok, true);
  const { rememberedDirectory } = await chrome.storage.local.get('rememberedDirectory');
  assert.equal(rememberedDirectory, 'D:\\SavedDownloads');
  assert.deepEqual(putPayload, { theme: 'dark', defaultDirectory: 'D:\\SavedDownloads' });
});

test('create-category sends valid POST payload to engine and returns new category', async () => {
  let postPayload = null;
  useApi({
    health: async () => true,
    apiFetch: async (path, init) => {
      if (path === '/categories' && init?.method === 'POST') {
        postPayload = JSON.parse(init.body);
        return {
          ok: true,
          json: async () => ({ id: 42 }),
        };
      }
      return { ok: false };
    },
  });
  const chrome = await loadBackground();
  const res = await message(chrome, {
    type: 'zoltraak-intercept-create-category',
    name: 'Music',
    kind: 'extension',
    pattern: 'mp3, flac, wav',
    directory: '"D:\\Music\\Tracks"',
  });
  assert.equal(res.ok, true);
  assert.equal(res.category.id, 42);
  assert.equal(res.category.name, 'Music');
  assert.equal(res.category.directory, 'D:\\Music\\Tracks');
  assert.deepEqual(postPayload, {
    name: 'Music',
    kind: 'extension',
    pattern: 'mp3, flac, wav',
    directory: 'D:\\Music\\Tracks',
    priority: 0,
  });
});

test('create-category rejects relative directory path', async () => {
  useApi({ health: async () => true });
  const chrome = await loadBackground();
  const res = await message(chrome, {
    type: 'zoltraak-intercept-create-category',
    name: 'Docs',
    kind: 'extension',
    pattern: 'pdf',
    directory: 'relative/path',
  });
  assert.equal(res.ok, false);
  assert.match(res.error, /Enter an absolute destination folder path/);
});

test('choose-folder delegates to /folder/choose and returns selected path', async () => {
  let requestedStart = null;
  useApi({
    health: async () => true,
    apiFetch: async (path, init) => {
      if (path === '/folder/choose' && init?.method === 'POST') {
        requestedStart = JSON.parse(init.body).start;
        return {
          ok: true,
          json: async () => ({ path: 'D:\\Selected\\Directory' }),
        };
      }
      return { ok: false };
    },
  });
  const chrome = await loadBackground();
  const res = await message(chrome, {
    type: 'zoltraak-intercept-choose-folder',
    start: '"C:\\Start\\Folder"',
  });
  assert.equal(res.ok, true);
  assert.equal(res.path, 'D:\\Selected\\Directory');
  assert.equal(requestedStart, 'C:\\Start\\Folder');
});

test('list-folders delegates to /folder/list and returns directory items', async () => {
  useApi({
    health: async () => true,
    apiFetch: async (path, init) => {
      if (path === '/folder/list' && init?.method === 'POST') {
        return {
          ok: true,
          json: async () => ({
            current: 'C:\\Downloads',
            parent: 'C:\\',
            folders: [{ name: 'Work', path: 'C:\\Downloads\\Work' }],
            drives: ['C:\\', 'D:\\'],
          }),
        };
      }
      return { ok: false };
    },
  });
  const chrome = await loadBackground();
  const res = await message(chrome, {
    type: 'zoltraak-intercept-list-folders',
    path: 'C:\\Downloads',
  });
  assert.equal(res.ok, true);
  assert.equal(res.current, 'C:\\Downloads');
  assert.equal(res.parent, 'C:\\');
  assert.deepEqual(res.folders, [{ name: 'Work', path: 'C:\\Downloads\\Work' }]);
  assert.deepEqual(res.drives, ['C:\\', 'D:\\']);
});

// Browser events are exercised against the real background module, with no live engine.
for (const url of ['http://localhost:9000/file.zip', 'http://127.0.0.1:14500/file', 'http://example.com/file', 'https://example.com/file']) {
  test(`intercepts ordinary HTTP(S) download: ${url}`, async () => {
    useApi({ health: async () => true });
    const chrome = await loadBackground();
    await chrome.listeners.downloadCreated({ ...item, url, finalUrl: url });
    assert.deepEqual(chrome.calls.cancel, [10]);
    assert.equal(pending(chrome)[0].url, url);
  });
}

test('engine origin is excluded on original, redirected, link and Firefox attachment paths', async () => {
  useApi({ health: async () => true });
  let receive;
  globalThis.browser = { webRequest: { onHeadersReceived: { addListener: (fn) => { receive = fn; } } } };
  try {
    const chrome = await loadBackground();
    const engine = 'http://127.0.0.1:1450/export';
    await chrome.listeners.downloadCreated({ ...item, url: engine });
    await chrome.listeners.downloadCreated({ ...item, finalUrl: engine });
    assert.equal((await message(chrome, { type: 'zoltraak-intercept-link', url: engine })).ok, false);
    assert.deepEqual(await receive({ method: 'GET', tabId: 3, url: engine, responseHeaders: [{ name: 'Content-Disposition', value: 'attachment' }] }), {});
    assert.equal(pending(chrome).length, 0);
    assert.deepEqual(chrome.calls.cancel, []);
  } finally { delete globalThis.browser; }
});

test('redirect capture uses the final URL and duplicate events cancel only once', async () => {
  useApi({ health: async () => true });
  const chrome = await loadBackground();
  const redirected = { ...item, finalUrl: 'http://localhost:8765/final.zip' };
  await Promise.all([chrome.listeners.downloadCreated(redirected), chrome.listeners.downloadCreated(redirected)]);
  await chrome.listeners.downloadCreated(redirected);
  assert.equal(pending(chrome).length, 1);
  assert.equal(pending(chrome)[0].url, redirected.finalUrl);
  assert.deepEqual(chrome.calls.cancel, [10]);
});

test('fast completion and browser interruption are diagnosed without cancelling', async () => {
  useApi({ health: async () => true });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated({ ...item, state: 'complete' });
  assert.match(chrome.store.lastInterception.reason, /already finished/);
  await chrome.listeners.downloadCreated({ ...item, state: 'interrupted', error: 'NETWORK_FAILED' });
  assert.equal(chrome.store.lastInterception.browserError, 'NETWORK_FAILED');
  assert.deepEqual(chrome.calls.cancel, []);
});

test('browser interruptions before filename determination keep the browser error code', async () => {
  useApi({});
  const chrome = await loadBackground((c) => {
    c.downloads.search = async () => [{ ...item, error: 'NETWORK_FAILED' }];
  });
  chrome.listeners.downloadChanged({ id: item.id, state: { current: 'interrupted' }, error: { current: 'NETWORK_FAILED' } });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(chrome.store.lastInterception.browserError, 'NETWORK_FAILED');
  assert.deepEqual(chrome.calls.cancel, []);
});

test('cancellation failure leaves the browser copy and removes the unowned draft', async () => {
  useApi({ health: async () => true });
  const chrome = await loadBackground((c) => { c.downloads.cancel = async () => { throw new Error('already done'); }; });
  await chrome.listeners.downloadCreated(item);
  assert.equal(pending(chrome).length, 0);
  assert.equal(chrome.store.lastInterception.stage, 'cancel');
});

test('dialog failure after cancellation retains a recoverable draft', async () => {
  useApi({ health: async () => true });
  const chrome = await loadBackground((c) => {
    c.tabs.sendMessage = async () => { throw new Error('no content script'); };
    c.tabs.create = async () => { throw new Error('window closed'); };
  });
  await chrome.listeners.downloadCreated(item);
  assert.equal(pending(chrome).length, 1);
  assert.equal(chrome.store.lastInterception.stage, 'dialog');
});

test('restricted page falls back to extension dialog', async () => {
  useApi({ health: async () => true });
  let opened;
  const chrome = await loadBackground((c) => {
    c.tabs.sendMessage = async () => { throw new Error('restricted'); };
    c.tabs.create = async (options) => { opened = options.url; };
  });
  await chrome.listeners.downloadCreated(item);
  assert.equal(opened, 'chrome-extension://test/intercept.html');
  assert.equal(pending(chrome).length, 1);
});

const scopedMessage = (chrome, body, frameId = 0, documentId = 'doc') => new Promise((resolve) =>
  chrome.listeners.onMessage(body, { tab: { id: 3 }, frameId, documentId, url: 'https://example.com/' }, resolve));
const mediaRequest = (chrome, url, extra = {}) => chrome.listeners.beforeRequest({
  tabId: 3, frameId: 0, documentId: 'doc', documentUrl: 'https://example.com/', url, timeStamp: Date.now(), ...extra,
});
const metadata = (url, duration = 10) => ({ title: 'Fixture video', duration, webpageUrl: url,
  formats: [{ id: 'hls', videoUrl: url, container: 'mp4', hasAudio: true, width: 1280, height: 720 }] });
const mediaApi = (analyzeVideo) => useApi({ analyzeVideo, bestCompleteFormat: (formats) =>
  formats.length ? { format: formats[0], complete: true } : null });

test('frame/document scope excludes adjacent players and old documents', async () => {
  useApi({});
  const chrome = await loadBackground();
  mediaRequest(chrome, 'https://cdn.example/own.m3u8');
  mediaRequest(chrome, 'https://cdn.example/ad.m3u8', { frameId: 7 });
  mediaRequest(chrome, 'https://cdn.example/old.m3u8', { documentId: 'old-doc' });
  const response = await scopedMessage(chrome, { type: 'zoltraak-tab-media' });
  assert.deepEqual(response.media.map((e) => e.url), ['https://cdn.example/own.m3u8']);
});

test('cold worker hydrates persisted captures before a new event and persists repeat timestamps', async () => {
  useApi({});
  const chrome = await loadBackground((c) => {
    c.store['media:3'] = [{ url: 'https://cdn.example/master.m3u8', kind: 'hls', ts: 10, frameId: 0, documentId: 'doc' }];
  });
  mediaRequest(chrome, 'https://cdn.example/movie.mp4');
  await scopedMessage(chrome, { type: 'zoltraak-tab-media' });
  assert.equal(chrome.store['media:3'].length, 2);
  mediaRequest(chrome, 'https://cdn.example/master.m3u8', { timeStamp: 500 });
  await scopedMessage(chrome, { type: 'zoltraak-tab-media' });
  assert.equal(chrome.store['media:3'].find((e) => e.kind === 'hls').ts, 500);
});

test('fragment changes preserve captures but document navigation clears them', async () => {
  useApi({});
  const chrome = await loadBackground();
  mediaRequest(chrome, 'https://cdn.example/early.m3u8');
  chrome.listeners.tabUpdated(3, { url: 'https://example.com/#player' }, { url: 'https://example.com/#player' });
  assert.equal((await scopedMessage(chrome, { type: 'zoltraak-tab-media' })).media.length, 1);
  mediaRequest(chrome, 'https://example.com/next', { type: 'main_frame' });
  assert.equal((await scopedMessage(chrome, { type: 'zoltraak-tab-media' })).media.length, 0);
});

test('HLS fallback recovers an early signed manifest only after empty normal formats', async () => {
  const calls = [];
  const stream = 'https://cdn.example/master.m3u8?sig=a%2Fb+z&token=x';
  mediaApi(async (url) => {
    calls.push(url);
    return { ok: true, metadata: url === stream ? metadata(url) : { formats: [] } };
  });
  const chrome = await loadBackground();
  mediaRequest(chrome, stream, { timeStamp: 100 });
  assert.equal((await scopedMessage(chrome, { type: 'zoltraak-tab-media', since: 6000 })).media.length, 0);
  const response = await scopedMessage(chrome, { type: 'zoltraak-analyze-best', candidates: ['https://example.com/'], documentStartedAt: 50 });
  assert.equal(response.ok, true);
  assert.equal(response.url, stream);
  assert.deepEqual(calls, ['https://example.com/', stream]);
  calls.length = 0;
  const direct = await scopedMessage(chrome, { type: 'zoltraak-analyze-best', candidates: [stream], hlsCandidates: ['https://cdn.example/unrelated.m3u8'] });
  assert.equal(direct.ok, true);
  assert.deepEqual(calls, [stream]);
});

test('extensionless HLS content type is a fallback source', async () => {
  const stream = 'https://cdn.example/playlist?token=abc';
  mediaApi(async (url) => url === stream ? { ok: true, metadata: metadata(url) } : { ok: false, error: 'empty' });
  const chrome = await loadBackground();
  chrome.listeners.headersReceived({ tabId: 3, frameId: 0, documentId: 'doc', url: stream,
    responseHeaders: [{ name: 'Content-Type', value: 'application/vnd.apple.mpegurl' }] });
  assert.equal((await scopedMessage(chrome, { type: 'zoltraak-analyze-best', candidates: ['https://example.com/'] })).url, stream);
});

test('fallback rejects a duration mismatch and reports detection separately from resolution', async () => {
  mediaApi(async (url) => url.endsWith('.m3u8') ? { ok: true, metadata: metadata(url, 90) } : { ok: false, error: 'empty' });
  const chrome = await loadBackground();
  const request = { type: 'zoltraak-analyze-best', candidates: ['https://example.com/'], videoContext: { duration: 10 } };
  assert.match((await scopedMessage(chrome, request)).error, /No stream detected/);
  mediaRequest(chrome, 'https://cdn.example/wrong.m3u8');
  const response = await scopedMessage(chrome, request);
  assert.equal(response.ok, false);
  assert.match(response.error, /Stream detected but could not be resolved/);
});

test('ambiguous fallback returns choices without starting any download', async (t) => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response('#EXTM3U\n#EXTINF:10,\nsegment.ts\n');
  t.after(() => { globalThis.fetch = originalFetch; });
  mediaApi(async (url) => url.endsWith('.m3u8') ? { ok: true, metadata: metadata(url) } : { ok: false, error: 'empty' });
  const chrome = await loadBackground();
  const response = await scopedMessage(chrome, { type: 'zoltraak-analyze-best', candidates: ['https://example.com/'],
    hlsCandidates: ['https://cdn.example/a.m3u8', 'https://cdn.example/b.m3u8'] });
  assert.equal(response.ok, false);
  assert.equal(response.choices.length, 2);
});

test('master playlist suppresses only its own captured renditions', async (t) => {
  useApi({});
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url) => new Response(url.endsWith('master.m3u8')
    ? '#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000\nchild.m3u8\n'
    : '#EXTM3U\n#EXTINF:10,\nsegment.ts\n');
  t.after(() => { globalThis.fetch = originalFetch; });
  const chrome = await loadBackground();
  for (const name of ['master', 'child', 'unrelated']) mediaRequest(chrome, `https://cdn.example/${name}.m3u8`);
  const response = await scopedMessage(chrome, { type: 'zoltraak-tab-media' });
  assert.deepEqual(new Set(response.media.map((e) => e.url)), new Set(['https://cdn.example/master.m3u8', 'https://cdn.example/unrelated.m3u8']));
});

test('equal-duration candidates require a choice and never start a job', async () => {
  const urls = ['https://cdn.example/play?id=A', 'https://cdn.example/play?id=B'];
  mediaApi(async (url) => ({ ok: true, metadata: metadata(url) }));
  const chrome = await loadBackground();
  const response = await scopedMessage(chrome, { type: 'zoltraak-download-best', candidates: urls,
    videoContext: { duration: 10, width: 1280, height: 720, currentSrc: 'blob:player', recentMediaUrls: urls } });
  assert.equal(response.ok, false);
  assert.equal(response.choices.length, 2);
});

test('query content identity wins over an earlier equal-duration neighbor', async () => {
  const urls = ['https://cdn.example/play?id=A', 'https://cdn.example/play?id=B'];
  mediaApi(async (url) => ({ ok: true, metadata: metadata(url) }));
  const chrome = await loadBackground();
  const response = await scopedMessage(chrome, { type: 'zoltraak-analyze-best', candidates: urls,
    videoContext: { duration: 10, currentSrc: urls[1] } });
  assert.equal(response.ok, true);
  assert.equal(response.url, urls[1]);
});

test('a single weak stream with player context still requires confirmation', async () => {
  const url = 'https://cdn.example/weak.m3u8';
  mediaApi(async () => ({ ok: true, metadata: metadata(url) }));
  const chrome = await loadBackground();
  const response = await scopedMessage(chrome, { type: 'zoltraak-analyze-best', candidates: [url],
    videoContext: { duration: 10, currentSrc: 'blob:player' } });
  assert.equal(response.ok, false);
  assert.equal(response.choices.length, 1);
});

test('mixed generic page formats become separate choices rather than one video', async () => {
  const a = metadata('https://cdn.example/a.mp4');
  const b = metadata('https://cdn.example/b.mp4');
  mediaApi(async () => ({ ok: true, metadata: { ...a, formats: [...a.formats, ...b.formats] } }));
  const chrome = await loadBackground();
  const response = await scopedMessage(chrome, { type: 'zoltraak-analyze-best', candidates: ['https://example.com/feed'],
    videoContext: { duration: 10 } });
  assert.equal(response.ok, false);
  assert.equal(response.choices.length, 2);
  assert.ok(response.choices.every((choice) => choice.metadata.formats.length === 1));
});

test('a reused player rejects a stale selected-format download in the original document', async () => {
  useApi({ startVideoDownload: async () => assert.fail('stale video must not start') });
  const chrome = await loadBackground((c) => {
    c.tabs.sendMessage = async (tabId, request, scope) => {
      assert.equal(tabId, 3);
      assert.equal(request.type, 'zoltraak-check-video-identity');
      assert.deepEqual(scope, { documentId: 'doc' });
      return { ok: false };
    };
  });
  const response = await scopedMessage(chrome, { type: 'zoltraak-download', webpageUrl: 'https://example.com/video',
    format: metadata('https://cdn.example/old.mp4').formats[0],
    playerBinding: { playerInstanceId: 'player-1', generation: 1, fingerprint: 'old' } });
  assert.equal(response.ok, false);
  assert.match(response.error, /video changed/);
});

test('automatic selection rechecks player after asynchronous analysis', async () => {
  const url = 'https://cdn.example/own.mp4';
  mediaApi(async () => ({ ok: true, metadata: metadata(url) }));
  const chrome = await loadBackground((c) => { c.tabs.sendMessage = async () => ({ ok: false }); });
  const response = await scopedMessage(chrome, { type: 'zoltraak-download-best', candidates: [url],
    videoContext: { currentSrc: url, playerBinding: { playerInstanceId: 'player-1', generation: 1 } } });
  assert.equal(response.ok, false);
  assert.match(response.error, /video changed/);
});

test('fresh explicit selection starts once with exact signed URL', async () => {
  const url = 'https://cdn.example/own.mp4?sig=a%2Fb+z&id=A';
  let started = 0;
  useApi({ startVideoDownload: async (request) => {
    assert.equal(request.format.videoUrl, url);
    started += 1;
    return { ok: true, id: 'fixture' };
  } });
  const chrome = await loadBackground((c) => {
    c.tabs.sendMessage = async () => ({ ok: true, documentStartedAt: 50 });
  });
  const response = await scopedMessage(chrome, { type: 'zoltraak-download', webpageUrl: 'https://example.com/video',
    format: metadata(url).formats[0], documentStartedAt: 50,
    playerBinding: { playerInstanceId: 'player-1', generation: 1 } });
  assert.equal(response.ok, true);
  assert.equal(started, 1);
});

test('frame reuse after navigation rejects the old document binding', async () => {
  useApi({ startVideoDownload: async () => assert.fail('old document must not start') });
  const chrome = await loadBackground((c) => {
    c.tabs.sendMessage = async () => ({ ok: true, documentStartedAt: 100 });
  });
  const response = await scopedMessage(chrome, { type: 'zoltraak-download', webpageUrl: 'https://example.com/video',
    format: metadata('https://cdn.example/old.mp4').formats[0], documentStartedAt: 50,
    playerBinding: { playerInstanceId: 'player-1', generation: 1 } });
  assert.equal(response.ok, false);
});


test('post fallback opens Images with the exact post, frame and browser container session', async () => {
  const post = 'https://www.instagram.com/reel/ABC123/';
  const calls = [];
  useApi({ cookieHeader: async (url, storeId) => { calls.push([url, storeId]); return 'sessionid=fresh'; },
    prefillImages: async (...args) => { calls.push(args); return { ok: true }; } });
  const chrome = await loadBackground((c) => {
    c.tabs.sendMessage = async (tabId, message, scope) => {
      assert.equal(tabId, 3);
      assert.deepEqual(scope, { documentId: 'doc' });
      return message.type === 'zoltraak-check-video-identity'
        ? { ok: true, documentStartedAt: 50 } : { url: post, documentStartedAt: 50 };
    };
  });
  const response = await new Promise((resolve) => chrome.listeners.onMessage({
    type: 'zoltraak-prefill-post', url: post + '?utm_source=copy', documentStartedAt: 50,
    playerBinding: { playerInstanceId: 'p', generation: 1 },
  }, { tab: { id: 3, cookieStoreId: 'firefox-container-2' }, documentId: 'doc', frameId: 7, url: post }, resolve));
  assert.equal(response.ok, true);
  assert.deepEqual(calls, [[post, 'firefox-container-2'], [post, [['Cookie', 'sessionid=fresh']], post]]);
});

test('post fallback rejects navigation, stale players, unsupported URLs and missing bindings', async () => {
  const post = 'https://www.instagram.com/reel/ABC123/';
  useApi({ prefillImages: async () => assert.fail('wrong post must not open Images') });
  const chrome = await loadBackground((c) => {
    c.tabs.sendMessage = async (_, message) => message.type === 'zoltraak-check-video-identity'
      ? { ok: true, documentStartedAt: 50 } : { url: 'https://www.instagram.com/reel/OTHER/', documentStartedAt: 50 };
  });
  const body = { type: 'zoltraak-prefill-post', url: post, documentStartedAt: 50,
    playerBinding: { playerInstanceId: 'p', generation: 1 } };
  assert.equal((await scopedMessage(chrome, body)).ok, false);
  assert.equal((await scopedMessage(chrome, { ...body, url: 'https://www.instagram.com/' })).ok, false);
  assert.equal((await scopedMessage(chrome, { ...body, playerBinding: null })).ok, false);
  chrome.tabs.sendMessage = async () => ({ ok: false });
  assert.equal((await scopedMessage(chrome, body)).ok, false);
});

test('Instagram carousel videos remain separate choices even with matching post identity', async () => {
  const post = 'https://www.instagram.com/p/ABC123/';
  const clip = (id, duration, width, height) => ({ id: `clip-${id}-mp4`, container: 'mp4',
    videoUrl: `https://cdn.test/${id}.mp4?sig=original`, width, height, hasAudio: true,
    validation: { duration }, downloadOptions: { thumbnail: `https://cdn.test/${id}.jpg` } });
  mediaApi(async () => ({ ok: true, metadata: { webpageUrl: post, title: 'Carousel',
    formats: [clip(1, 20, 1280, 720), clip(2, 10, 720, 1280), clip(3, 10, 720, 1280)] } }));
  const chrome = await loadBackground();
  const response = await scopedMessage(chrome, { type: 'zoltraak-analyze-best', candidates: [post],
    videoContext: { permalink: post, duration: 10, width: 720, height: 1280 } });
  assert.equal(response.ok, false);
  assert.equal(response.choices.length, 2);
  assert.ok(response.choices.every((c) => c.metadata.formats.length === 1 && !c.identity.strong));
  assert.deepEqual(response.choices.map((c) => c.metadata.duration), [10, 10]);
});


test('pending downloads refresh cookies in their original Firefox container', async () => {
  let session = 'session=old';
  let request;
  const stores = [];
  useApi({ health: async () => true,
    cookieHeader: async (url, storeId) => { stores.push(storeId); return session; },
    enqueue: async (body) => { request = body; return { ok: true }; },
  });
  const chrome = await loadBackground((chrome) => {
    chrome.tabs.get = async (id) => ({ id, cookieStoreId: 'firefox-container-2' });
  });
  await chrome.listeners.downloadCreated(item);
  assert.deepEqual(pending(chrome)[0].headers, [['Cookie', 'session=old']]);
  session = 'session=fresh';
  assert.equal((await message(chrome, { type: 'zoltraak-intercept-submit',
    id: pending(chrome)[0].id, filename: 'report.pdf' })).ok, true);
  assert.deepEqual(request.headers, [['Cookie', 'session=fresh']]);
  assert.deepEqual(stores, ['firefox-container-2', 'firefox-container-2']);
});

test('pending downloads stop replaying cookies after access is revoked', async () => {
  let session = 'session=old';
  let request;
  useApi({ health: async () => true, cookieHeader: async () => session,
    enqueue: async (body) => { request = body; return { ok: true }; },
  });
  const chrome = await loadBackground();
  await chrome.listeners.downloadCreated(item);
  session = null;
  assert.equal((await message(chrome, { type: 'zoltraak-intercept-submit',
    id: pending(chrome)[0].id, filename: 'report.pdf' })).ok, true);
  assert.deepEqual(request.headers, []);
});


test('TikTok post actions keep the selected item, container cookie and browser user agent', async () => {
  let prefilled;
  const post = 'https://www.tiktok.com/@creator/video/7671362008486251790';
  useApi({ cookieHeader: async (_url, store) => { assert.equal(store, 'container-2'); return 'session=private'; },
    prefillImages: async (...args) => { prefilled = args; return { ok: true }; } });
  const chrome = await loadBackground((chrome) => {
    chrome.tabs.sendMessage = async () => ({ url: post });
  });
  await chrome.listeners.menuClick({ menuItemId: 'zoltraak-post', frameId: 0, linkUrl: 'https://www.tiktok.com/@creator' },
    { id: 3, url: 'https://www.tiktok.com/foryou', cookieStoreId: 'container-2' });
  assert.equal(prefilled[0], post);
  assert.deepEqual(prefilled[1], [['Cookie', 'session=private'], ['User-Agent', navigator.userAgent]]);
});
