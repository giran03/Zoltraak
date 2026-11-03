import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';
import '../video-identity.js';

const source = readFileSync(new URL('../content.js', import.meta.url), 'utf8');
function fixture() {
  const videos = [];
  const context = vm.createContext({ ZoltraakVideoIdentity: globalThis.ZoltraakVideoIdentity,
    location: { hostname: 'www.instagram.com', href: 'https://www.instagram.com/reel/DdKrjQHNY86/' },
    document: { querySelectorAll: () => videos, baseURI: 'https://www.instagram.com/' },
    window: { innerWidth: 1100, innerHeight: 800 }, Date, URL, Set,
  });
  const names = ['isItemPermalink', 'isYouTubeHost', 'isEligible', 'visibleArea', 'mostProminent',
    'pickVideo', 'nearestPermalink', 'videoPermalink', 'socialPostPermalink', 'setPanelStatus', 'playerFingerprint', 'playerBinding',
    'matchesPlayer', 'directSrc', 'mediaUrl', 'candidateTargets', 'checkAndRefresh'];
  const functions = names.map((name) => {
    const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
    assert.ok(start >= 0, name);
    return source.slice(start, source.indexOf('\n}', start) + 2);
  }).join('\n');
  vm.runInContext(`
    const MIN_WIDTH = 160, MIN_HEIGHT = 90;
    let currentVideo = null, dismissedVideo = null, currentPlayerGeneration = null;
    let currentVideoSince = 0, lastAnalyzedVideo = null, lastAnalyzedMetadata = null;
    const playerTracker = ZoltraakVideoIdentity.createPlayerTracker();
    const overlayPrefs = { showMediaButton: true };
    let renders = 0, captures = 0, statusHideTimer = null;
    function manifestFromAttributes() { return null; }
    function overlaysAllowed() { return true; }
    function isXHost() { return false; }
    function hasSiteExtractor() { return true; }
    async function checkAppHealth() { return true; }
    async function tabCaptures() { captures++; return ['https://cdn.test/unrelated.mp4']; }
    function renderButton() { renders++; }
    function teardownHost() {}
    function updateHostPosition() {}
    ${functions}
  `, context);
  function video(top = 0) {
    const player = { isConnected: true, currentSrc: 'blob:reel', src: '', poster: '',
      duration: 10, currentTime: 8, paused: false, ended: false, readyState: 4,
      closest: () => null, getAttribute: () => null, querySelectorAll: () => [],
      parentElement: null,
      getBoundingClientRect: () => ({ top, bottom: top + 500, left: 0, right: 300, width: 300, height: 500 }),
    };
    videos.push(player);
    return player;
  }
  return { context, video, run: (code) => vm.runInContext(code, context) };
}

test('Instagram audio links are excluded from nearby permalink discovery', () => {
  const f = fixture();
  const player = f.video();
  player.closest = (selector) => selector === 'a[href]'
    ? { href: 'https://www.instagram.com/reels/audio/28686481800976139/' } : null;
  f.context.player = player;
  assert.equal(f.run('nearestPermalink(player)'), null);
  assert.equal(f.run('videoPermalink(player)'), f.context.location.href);
});

test('preloaded reels do not prevent identification of the visible item', async () => {
  const f = fixture();
  f.context.player = f.video();
  f.context.neighbor = f.video(900);
  assert.equal(f.run('videoPermalink(player)'), f.context.location.href);
  assert.equal(f.run('videoPermalink(neighbor)'), null);
  const candidates = await f.run('candidateTargets(player)');
  assert.deepEqual([...candidates], [f.context.location.href]);
  assert.equal(f.run('captures'), 0);
});

test('the same visible reel retains its picker through end, loop, reload and metadata changes', async () => {
  const f = fixture();
  f.context.player = f.video();
  const neighbor = f.video(200);
  await f.run('checkAndRefresh()');
  const binding = f.run('playerBinding(player)');
  f.context.binding = binding;
  f.context.player.ended = true;
  f.context.player.paused = true;
  neighbor.paused = false;
  await f.run('checkAndRefresh()');
  f.context.player.currentTime = 0;
  f.context.player.duration = NaN;
  f.run('playerBinding(player, true)');
  await f.run('checkAndRefresh()');
  assert.equal(f.run('matchesPlayer(player, binding)'), true);
  assert.equal(f.run('renders'), 1);
  f.context.player.currentSrc = 'blob:another-reel';
  await f.run('checkAndRefresh()');
  assert.equal(f.run('renders'), 2);
  assert.equal(f.run('matchesPlayer(player, binding)'), false);
  f.context.player.getBoundingClientRect = () => ({ top: -600, bottom: -100, left: 0, right: 300, width: 300, height: 500 });
  await f.run('checkAndRefresh()');
  assert.equal(f.run('currentVideo'), neighbor);
  assert.equal(f.run('renders'), 3);
});


test('post recovery uses the selected permalink and refuses a changed player', async () => {
  const f = fixture();
  f.context.player = f.video();
  const messages = [];
  const element = () => ({ children: [], style: {}, handlers: {}, classList: { add() {}, remove() {} },
    appendChild(child) { this.children.push(child); },
    setAttribute() {}, addEventListener(type, handler) { this.handlers[type] = handler; } });
  f.context.document.createElement = element;
  f.context.chrome = { runtime: { sendMessage: async (message) => { messages.push(message); return { ok: true }; } } };
  f.context.performance = { timeOrigin: 50 };
  f.context.panel = element();
  f.run("setPanelStatus(panel, 'network error', false, 'bad', { url: 'https://cdn.test/wrong.mp4', video: player, playerBinding: playerBinding(player) })");
  let container = f.context.panel.children[0];
  assert.equal(container.children[0].children[0].children.at(-1).textContent,
    'Open this post in Zoltraak to find its photos and videos.');
  let button = container.children[1].children[0];
  assert.equal(button.textContent, 'Download post media');
  await button.handlers.click();
  assert.equal(messages[0].type, 'zoltraak-prefill-post');
  assert.equal(messages[0].url, f.context.location.href);
  assert.equal(button.textContent, 'Opened in Images');
  f.context.panel = element();
  f.run("setPanelStatus(panel, 'network error', false, 'bad', { url: location.href, video: player, playerBinding: playerBinding(player) })");
  button = f.context.panel.children[0].children[1].children[0];
  f.context.player.currentSrc = 'blob:next';
  await button.handlers.click();
  assert.equal(messages.length, 1);
});

test('feed recovery requires an unambiguous post link and rejects neighboring sites', () => {
  const f = fixture();
  f.context.player = f.video();
  f.context.location.href = 'https://www.instagram.com/';
  assert.equal(f.run('socialPostPermalink(player)'), null);
  f.context.player.closest = (selector) => selector === 'a[href]' ? { href: 'https://www.instagram.com/p/ABC123/' } : null;
  assert.equal(f.run('socialPostPermalink(player)'), 'https://www.instagram.com/p/ABC123/');
  f.context.player.closest = (selector) => selector === 'a[href]' ? { href: 'https://x.com/user/status/123' } : null;
  assert.equal(f.run('socialPostPermalink(player)'), null);
});


test('a multi-player carousel recovers only its own unambiguous article permalink', () => {
  const f = fixture();
  f.context.player = f.video();
  f.context.neighbor = f.video();
  f.context.location.href = 'https://www.instagram.com/';
  const links = [{ href: 'https://www.instagram.com/p/ABC123/' }];
  f.context.player.parentElement = { tagName: 'ARTICLE', getAttribute: () => null,
    matches: () => true, querySelectorAll: (selector) => selector === 'video'
      ? [f.context.player, f.context.neighbor] : links, parentElement: null };
  assert.equal(f.run('socialPostPermalink(player)'), links[0].href);
  links.push({ href: 'https://www.instagram.com/p/OTHER/' });
  assert.equal(f.run('socialPostPermalink(player)'), null);
});
