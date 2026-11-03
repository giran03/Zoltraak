// Service worker. MV3 unloads this between events, so nothing here relies on
// module-level state surviving — settings live in `chrome.storage`, and the
// badge is recomputed from scratch on every alarm tick.
//
// Loaded as an ES module (see manifest.json's background.type) rather than
// via importScripts(): Firefox's MV3 background context is a plain page, not
// a Worker, and importScripts() only exists inside a real Worker — it threw
// ReferenceError there, so nothing behind chrome.runtime.onMessage ever ran.

import {
  ENGINE_ORIGIN,
  ensurePaired,
  health,
  enqueue,
  analyzeVideo,
  startVideoDownload,
  getSnapshot,
  revealTarget,
  openTarget,
  setDownloadState,
  removeDownload,
  cookieHeader,
  bestFormat,
  bestCompleteFormat,
  prefillDownload,
  prefillVideo,
  prefillImages,
  prefillMusic,
  pushSession,
  youtubeSessionCookie,
  getToken,
  verify,
  apiFetch,
  setCookieSourceState,
  canUseBrowserCookies,
  extensionStatusCursor,
  setExtensionStatusCursor,
} from "./api.js";

import "./video-identity.js";
import { registerGenericMedia } from "./media-registry.js";
const { verifyAndScoreCandidate, filterFormatsByVideoContext } = globalThis.ZoltraakVideoIdentity;

const MENU_LINK = "zoltraak-link";
const MENU_MEDIA = "zoltraak-media";
const MENU_POST = "zoltraak-post";
const MENU_MUSIC = "zoltraak-music";
const MENU_MUSIC_LINK = "zoltraak-music-link";
const NON_POST_MENUS = [MENU_LINK, MENU_MEDIA, MENU_MUSIC, MENU_MUSIC_LINK];
const STATUS_ALARM = "zoltraak-status";
const REFRESH_TABS_KEY = "zoltraakRefreshTabsAfterReload";

async function queueInjectedTabsForRefresh() {
  if (!chrome.tabs?.query) return;
  const tabs = await chrome.tabs.query({ url: ["http://*/*", "https://*/*"] });
  const tabIds = [];
  for (const tab of tabs) {
    if (typeof tab.id !== "number") continue;
    try {
      // Old content scripts resolve with `undefined`; a missing content script
      // rejects. Either successful outcome means this tab needs reinjection.
      await chrome.tabs.sendMessage(tab.id, { type: "zoltraak-prepare-refresh" });
      tabIds.push(tab.id);
    } catch {
      // Nothing from this extension is injected in the tab.
    }
  }
  await chrome.storage.local.set({ [REFRESH_TABS_KEY]: tabIds });
}

async function refreshQueuedTabs() {
  if (!chrome.tabs?.reload) return;
  const stored = await chrome.storage.local.get(REFRESH_TABS_KEY);
  const tabIds = Array.isArray(stored[REFRESH_TABS_KEY]) ? stored[REFRESH_TABS_KEY] : [];
  await chrome.storage.local.remove(REFRESH_TABS_KEY);
  await Promise.all(tabIds.map((tabId) => chrome.tabs.reload(tabId).catch(() => {})));
}

// The music services the app's Music view resolves. A right-click on one of
// their pages — or on a link to one from anywhere — sends the link to the
// Music view rather than the Downloads entry, where it would be useless.
const MUSIC_URL_PATTERNS = [
  "*://open.spotify.com/*",
  "*://music.apple.com/*",
  "*://*.deezer.com/*",
  "*://music.youtube.com/*",
  "*://soundcloud.com/*",
  "*://*.bandcamp.com/*",
];

function createMenuItem(createProperties) {
  chrome.contextMenus.create(createProperties, () => {
    if (chrome.runtime.lastError) {
      // Ignored: item might already exist or context menu was closed
    }
  });
}

function setupContextMenus() {
  chrome.contextMenus.removeAll(() => {
    if (chrome.runtime.lastError) {
      // Ignored
    }
    createMenuItem({
      id: MENU_LINK,
      title: "Download with Zoltraak",
      contexts: ["link"],
      targetUrlPatterns: ["http://*/*", "https://*/*"],
    });
    createMenuItem({
      id: MENU_MEDIA,
      title: "Download media with Zoltraak",
      contexts: ["image", "audio"],
    });
    createMenuItem({
      id: MENU_POST,
      title: "Download post with Zoltraak",
      contexts: ["all"],
    });
    // Music services: the page you are on, or a link to one from anywhere.
    createMenuItem({
      id: MENU_MUSIC,
      title: "Download music with Zoltraak",
      contexts: ["page", "selection", "frame"],
      documentUrlPatterns: MUSIC_URL_PATTERNS,
    });
    createMenuItem({
      id: MENU_MUSIC_LINK,
      title: "Download music with Zoltraak",
      contexts: ["link"],
      targetUrlPatterns: MUSIC_URL_PATTERNS,
    });
    void syncContextMenusForActiveTab();
  });
}

function isXUrl(url) {
  return /^https?:\/\/(?:[a-z0-9-]+\.)*(?:x\.com|twitter\.com)\//i.test(url || "");
}

function setMenuVisible(id, visible) {
  return new Promise((resolve) => {
    chrome.contextMenus.update(id, { visible }, () => {
      void chrome.runtime.lastError;
      resolve();
    });
  });
}

async function syncContextMenusForUrl(url) {
  const onlyPost = isXUrl(url);
  await setMenuVisible(MENU_POST, true);
  await Promise.all(NON_POST_MENUS.map((id) => setMenuVisible(id, !onlyPost)));
}

async function syncContextMenusForActiveTab() {
  try {
    const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
    await syncContextMenusForUrl(tab?.url);
  } catch {
    // The next tab activation/navigation will synchronize the menu.
  }
}

chrome.runtime.onInstalled.addListener(() => {
  setupContextMenus();
  void initSessionOnLaunch();
  chrome.storage.local.set({ captureAll: true });
  if (chrome.alarms) {
    chrome.alarms.create(STATUS_ALARM, { periodInMinutes: 0.5 });
  }
  // On the signed Zen/Firefox build there is no token to paste — grab it from
  // the app over native messaging the moment we're installed.
  void ensurePaired().then(() => refreshBadge());
});

chrome.runtime.onStartup.addListener(() => {
  setupContextMenus();
  void initSessionOnLaunch();
  void checkStatus();
  void syncContextMenusForActiveTab();
});

chrome.tabs.onActivated?.addListener(async ({ tabId }) => {
  try {
    await syncContextMenusForUrl((await chrome.tabs.get(tabId)).url);
  } catch {
    // A tab can disappear between activation and lookup.
  }
});

chrome.tabs.onUpdated?.addListener((_tabId, changeInfo, tab) => {
  if (tab.active && (changeInfo.url || changeInfo.status === "loading")) {
    void syncContextMenusForUrl(changeInfo.url || tab.url);
  }
});

chrome.windows?.onFocusChanged?.addListener(() => {
  void syncContextMenusForActiveTab();
});

if (chrome.alarms) {
  chrome.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name === STATUS_ALARM) void checkStatus();
  });
}

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === MENU_POST) {
    await sendPostToApp(info, tab);
    return;
  }
  if (info.menuItemId === MENU_MUSIC || info.menuItemId === MENU_MUSIC_LINK) {
    await sendMusicToApp(info.menuItemId === MENU_MUSIC_LINK ? info.linkUrl : tab?.url, tab);
    return;
  }
  const url =
    info.menuItemId === MENU_LINK
      ? info.linkUrl
      : info.menuItemId === MENU_MEDIA ? info.srcUrl : null;
  if (!url) return;
  await sendDownloadToApp(url, tab);
});

/**
 * The content script's floating button and the popup's "Analyze this page"
 * both need `analyze`/`download`, and both run outside the service worker
 * where the API's origin check would reject them — so they ask this worker
 * to make the call instead of fetching `127.0.0.1` themselves.
 */
/**
 * The session cookie for the page being analyzed/downloaded, as the single
 * header the engine's request builder expects. Without this, a request for
 * a private or login-gated video (Instagram, Facebook, Twitter/X reels, ...)
 * reaches the site as an anonymous visitor and gets a stripped login-wall
 * page with no video embedded — the same page the user's own browser would
 * never see, since it already carries the session.
 */
async function videoRequestHeaders(url, storeId) {
  const headers = [];
  const cookie = await cookieHeader(url, storeId).catch(() => null);
  if (cookie) headers.push(["Cookie", cookie]);
  if (/^https?:\/\/(?:[a-z0-9-]+\.)*tiktok\.com(?:\/|$)/i.test(url || "") && globalThis.navigator?.userAgent) {
    headers.push(["User-Agent", globalThis.navigator.userAgent]);
  }
  return headers;
}

// The browser-harvested poToken + visitorData (see yt-inject.js / content.js),
// as the pseudo-headers the engine's YouTube extractor reads to authorize its
// stream requests. GVS tokens retain their video binding and are never
// replaced by player-context tokens. Drop captures older than six hours.
const YT_POT_MAX_AGE_MS = 6 * 60 * 60 * 1000;
async function youtubeAuthHeaders() {
  if (!(await canUseBrowserCookies())) return [];
  try {
    const { ytPot } = await chrome.storage.local.get("ytPot");
    if (!ytPot || !ytPot.ts || Date.now() - ytPot.ts > YT_POT_MAX_AGE_MS) return [];
    const headers = [];
    const token = ytPot.videoId && ytPot.poToken;
    if (token) headers.push(["X-Youtube-Potoken", token]);
    if (token) headers.push(["X-Youtube-Potoken-Video-Id", ytPot.videoId]);
    if (ytPot.visitorData) headers.push(["X-Youtube-Visitor-Data", ytPot.visitorData]);
    return headers;
  } catch {
    return [];
  }
}

// ─── Proactive session sync ─────────────────────────────────────────────
// The engine's login-gated extractors (YouTube, X, Instagram, Facebook,
// TikTok) only ever saw a cookie when the user explicitly right-clicked
// "Download with Zoltraak" on that exact link. A URL typed or pasted straight
// into the app's Video view never went through the extension at all, so it
// always hit a login wall even though the browser was signed in the whole
// time. Instead, keep the app's session vault fresh just from ordinary
// browsing: whenever a tab finishes loading one of these platforms — or the
// browser's cookies for it change — push the current session, keyed by
// canonical domain (matching `zoltraak_core::video::site::cookie_domain_for`),
// so *any* later analyze for that platform gets it automatically.
//
// For YouTube the session is the allowlisted cookies plus the proof-of-origin
// token yt-inject.js harvested from the player; a browser that has signed
// out pushes an empty session, which makes the app forget the stored one.
const COOKIE_SYNC_DOMAINS = [
  { test: (host) => /(^|\.)(x\.com|twitter\.com)$/i.test(host), domain: "x.com" },
  { test: (host) => /(^|\.)instagram\.com$/i.test(host), domain: "instagram.com" },
  { test: (host) => /(^|\.)(facebook\.com|fb\.watch)$/i.test(host), domain: "facebook.com" },
  { test: (host) => /(^|\.)tiktok\.com$/i.test(host), domain: "tiktok.com" },
  { test: (host) => /(^|\.)(youtube\.com|youtu\.be)$/i.test(host), domain: "youtube.com" },
];
// Per-domain throttle so a busy timeline doesn't push on every SPA
// navigation — the session cookie changes rarely; a couple of minutes'
// staleness costs nothing since a cookie *change* triggers its own sync.
const COOKIE_SYNC_MIN_INTERVAL_MS = 2 * 60 * 1000;
const lastCookieSync = new Map();

/** The session to share for one platform, or null when there is nothing. */
async function sessionFor(domain) {
  if (domain === "youtube.com") {
    const cookie = await youtubeSessionCookie();
    const { ytPot } = await chrome.storage.local.get("ytPot").catch(() => ({}));
    const fresh = ytPot && ytPot.ts && Date.now() - ytPot.ts <= YT_POT_MAX_AGE_MS;
    return {
      cookie,
      poToken: fresh && ytPot.videoId ? ytPot.poToken || null : null,
      poTokenVideoId: fresh ? ytPot.videoId || null : null,
      visitorData: fresh ? ytPot.visitorData || null : null,
    };
  }
  const cookie = await cookieHeader(`https://www.${domain}/`).catch(() => null);
  return { cookie };
}

async function syncSession(domain, { force = false } = {}) {
  const last = lastCookieSync.get(domain) || 0;
  if (!force && Date.now() - last < COOKIE_SYNC_MIN_INTERVAL_MS) {
    return { ok: true, skipped: true, signedIn: false };
  }
  lastCookieSync.set(domain, Date.now());
  const session = await sessionFor(domain).catch(() => ({ cookie: null }));
  // Pushed even when empty: that is how a sign-out reaches the app.
  const res = await pushSession(domain, session).catch(() => ({ ok: false }));
  const ok = Boolean(res?.ok);
  if (ok && domain === "youtube.com") {
    // Remember what the app actually accepted, so a token that never made it
    // can be told apart from one already delivered.
    try {
      await chrome.storage.local.set({ ytPotPushed: ytPushSignature(session) });
    } catch {
      /* the record is an optimisation; a missed write costs one extra push */
    }
  }
  return { ok, skipped: false, signedIn: Boolean(session?.cookie) };
}

/** What a YouTube push carried, reduced to the part worth comparing. */
function ytPushSignature(session) {
  return `${session?.poToken || ""}|${session?.visitorData || ""}|${session?.poTokenVideoId || ""}`;
}

/**
 * Deliver a proof-of-origin token that was harvested while the app was closed.
 *
 * `pushSession` fails silently when nothing is listening on the engine port,
 * and the storage listener above only fires when the token *value* changes —
 * so a token captured during downtime is stranded: starting the app delivers
 * nothing, and the next push waits for the user to happen to play a video that
 * yields a new token. Reconcile on each status check instead: if what the
 * store holds was never accepted, send it now.
 */
async function deliverStrandedYtToken() {
  try {
    const { ytPot, ytPotPushed } = await chrome.storage.local.get(["ytPot", "ytPotPushed"]);
    if (!ytPot?.ts || Date.now() - ytPot.ts > YT_POT_MAX_AGE_MS) return;
    // Built the same way `sessionFor` builds the push, so the two agree.
    const current = ytPushSignature({
      poToken: ytPot.videoId ? ytPot.poToken : null,
      poTokenVideoId: ytPot.videoId,
      visitorData: ytPot.visitorData,
    });
    if (current === ytPotPushed) return;
    await syncSession("youtube.com", { force: true });
  } catch {
    /* nothing to reconcile */
  }
}

/**
 * Re-read this browser's cookies for every supported platform and replace
 * whatever the app holds — the popup's "refresh every shared sign-in".
 *
 * The background sync above keeps the vault fresh as you browse, but it can
 * only react to what it sees: a session that expired server-side, or one
 * pushed before a sign-out on a site you have not opened since, sits in the
 * vault looking valid. A stale cookie is worse than no cookie — a site that
 * would serve an anonymous request happily can refuse, redirect-loop, or send
 * back a login wall when handed a session it has already invalidated — so
 * this is a way to say "take what the browser has now" without hunting for a
 * tab on each site. Domains with nothing to share push an empty session,
 * which is how the app is told to forget one.
 */
async function refreshAllSessions() {
  const domains = [...new Set(COOKIE_SYNC_DOMAINS.map((d) => d.domain))];
  const results = await Promise.all(
    domains.map(async (domain) => ({ domain, ...(await syncSession(domain, { force: true })) })),
  );
  return {
    shared: results.filter((r) => r.ok && r.signedIn).map((r) => r.domain),
    cleared: results.filter((r) => r.ok && !r.signedIn).map((r) => r.domain),
    failed: results.filter((r) => !r.ok).map((r) => r.domain),
  };
}

function syncDomainForHost(host) {
  return COOKIE_SYNC_DOMAINS.find((d) => d.test(host))?.domain ?? null;
}

async function syncSiteCookiesForTab(url) {
  if (!url || !/^https?:\/\//i.test(url)) return;
  let host;
  try {
    host = new URL(url).hostname;
  } catch {
    return;
  }
  const domain = syncDomainForHost(host);
  if (!domain) return;
  await syncSession(domain);
}

// A sign-in or sign-out changes cookies without a page load the tab listener
// would see. Watch the cookie store itself, debounced: a login sets a dozen
// cookies in a burst and one push is enough.
const cookieChangeTimers = new Map();
if (chrome.cookies?.onChanged) {
  chrome.cookies.onChanged.addListener(({ cookie }) => {
    const host = (cookie?.domain || "").replace(/^\./, "");
    const domain = syncDomainForHost(host);
    if (!domain) return;
    clearTimeout(cookieChangeTimers.get(domain));
    cookieChangeTimers.set(
      domain,
      setTimeout(() => {
        cookieChangeTimers.delete(domain);
        void syncSession(domain, { force: true });
      }, 4000),
    );
  });
}

// The proof-of-origin token arrives *after* the page has loaded: yt-inject
// only sees it once the player issues its first stream request, seconds into
// playback. By then the tab listener above has already pushed this domain's
// session — with no token in it — and the two-minute throttle swallows the
// next few pushes, so the token would sit in storage until some unrelated
// event happened to sync YouTube again. A signed-in browser can therefore
// leave the engine without the token needed by a client. Watch the store and push
// the moment a *new* token lands.
//
// This is the fast path only. A service worker can be torn down before the
// debounce below fires, and a push made while the app is closed goes nowhere,
// so delivery does not depend on either surviving: `deliverStrandedYtToken`
// reconciles from the status alarm every 30 seconds regardless.
const YT_POT_KEYS = ["poToken", "playerToken", "visitorData", "videoId"];
let ytPotPushTimer = null;
if (chrome.storage?.onChanged) {
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area !== "local" || !changes.ytPot) return;
    // `ts` is refreshed on every harvest; only a changed token is worth a push.
    const before = changes.ytPot.oldValue || {};
    const after = changes.ytPot.newValue || {};
    if (YT_POT_KEYS.every((k) => before[k] === after[k])) return;
    clearTimeout(ytPotPushTimer);
    ytPotPushTimer = setTimeout(() => {
      ytPotPushTimer = null;
      void syncSession("youtube.com", { force: true });
    }, 1000);
  });
}

// ─── Media sniffer ────────────────────────────────────────────────────────
// The accurate way to capture a *playing* video is to watch what the tab
// actually fetches. Modern players stream via MediaSource from `blob:` URLs
// that a page can never hand back, but the underlying HLS/DASH manifests (and
// progressive files) still travel as ordinary network requests — so we record
// those per tab and offer them to "Download this video" as real sources,
// instead of scraping the page HTML and guessing.
//
// MV3 unloads this worker between events; webRequest events wake it, and each
// capture is mirrored into chrome.storage.session so a query arriving after an
// idle unload still sees what earlier events recorded.

const MEDIA_STORE = chrome.storage.session ?? chrome.storage.local;
const mediaKey = (tabId) => `media:${tabId}`;
const MAX_MEDIA_PER_TAB = 96;

/** tabId -> [{ url, kind, ts }]; authoritative while the worker is alive. */
const tabMedia = new Map();
const mediaWrites = new Map();
const documentUrl = (url) => (url || "").split("#")[0];

// Serialize hydration and writes: a fresh worker must merge the first event
// with persisted captures, not replace them with a single new request.
function editTabMedia(tabId, edit) {
  const next = (mediaWrites.get(tabId) || Promise.resolve()).catch(() => {}).then(async () => {
    if (!tabMedia.has(tabId)) {
      const key = mediaKey(tabId);
      const stored = await MEDIA_STORE.get(key);
      tabMedia.set(tabId, Array.isArray(stored[key]) ? stored[key] : []);
    }
    const entries = edit(tabMedia.get(tabId));
    tabMedia.set(tabId, entries);
    await MEDIA_STORE.set({ [mediaKey(tabId)]: entries });
  });
  mediaWrites.set(tabId, next);
  void next.catch(() => {}).finally(() => {
    if (mediaWrites.get(tabId) === next) mediaWrites.delete(tabId);
  });
  return next;
}

// Individual stream segments and sidecar files are noise — we want the
// manifest or the whole progressive file, never the pieces.
const SEGMENT_RE = /\.(ts|m4s|aac|vtt|srt|key)(\?|$)/i;
const SEGMENTY_FILE_RE = /(seg(?:ment)?|chunk|frag|init)[-_]?\d*\.(?:mp4|m4v|webm)(\?|$)/i;
// Meta's player fetches its DASH tracks as byte ranges of a whole-file URL
// (`…mp4?bytestart=0&byteend=131071`). Each request is one slice of one
// track, and a busy Facebook or Instagram page issues hundreds — but with the
// two range params removed they all collapse to the same whole-track URL,
// which is worth keeping: on a feed page where no permalink can be read off
// the DOM, the tracks the player is fetching *right now* are the only thing
// that identifies the video on screen. The engine tells a picture track from
// a sound track by reading its header, and the resolver pairs them.
function stripByteRange(url) {
  return globalThis.ZoltraakVideoIdentity.completeMediaUrl(url);
}
const MEDIA_EXT_RE = /\.(m3u8|mpd|mp4|webm|mov|mkv|m4v|flv|m4a|mp3|ogg|oga|opus|wav|flac)([?#]|$)/i;
// Extensionless streaming endpoints worth keeping (YouTube's googlevideo, etc.).
const PROGRESSIVE_HINT_RE = /googlevideo\.com\/videoplayback|\/videoplayback\?|manifest\.googlevideo|mime=video/i;

function mediaKind(url) {
  if (/\.m3u8(\?|$)/i.test(url)) return "hls";
  if (/\.mpd(\?|$)/i.test(url)) return "dash";
  return "file";
}

// YouTube's player fetches each itag as many small byte-range requests that
// differ only by volatile params (range/rn/rbuf/cpn). Collapsing them to the
// stable base URL keeps one capture per itag instead of dozens of chunks — and
// the range-less URL serves the *whole* stream, which is exactly what we want
// to hand the engine. Only googlevideo URLs are rewritten; other sites' range
// params can be meaningful, so they're left untouched.
function normalizeGooglevideoUrl(url) {
  if (!/googlevideo\.com\/videoplayback/i.test(url)) return url;
  try {
    const u = new URL(url);
    for (const p of ["range", "rn", "rbuf", "cpn"]) u.searchParams.delete(p);
    return u.toString();
  } catch {
    return url;
  }
}

/** Manifest content types for endpoints that carry no telltale extension. */
function kindFromContentType(ct) {
  const v = ct.toLowerCase();
  if (v.includes("mpegurl")) return "hls"; // application/vnd.apple.mpegurl, application/x-mpegurl
  if (v.includes("dash+xml")) return "dash"; // application/dash+xml
  if (/^(video|audio)\//.test(v) && !/mp2t|aac/.test(v)) return "file";
  return null;
}

// Manifests rank above progressive files (adaptive gives quality choices), and
// within a rank the most recently requested — i.e. what the user just played.
const kindPriority = (kind) => (kind === "hls" || kind === "dash" ? 0 : 1);
const compareMedia = (a, b) => kindPriority(a.kind) - kindPriority(b.kind) || b.ts - a.ts;

function recordMedia(tabId, url, kind, details = {}) {
  if (tabId < 0) return;
  const capture = { url, kind, ts: details.timeStamp || Date.now(), frameId: details.frameId ?? 0,
    requestId: details.requestId, evidence: "frame-network",
    documentId: details.documentId, documentUrl: documentUrl(details.documentUrl || details.originUrl) };
  return editTabMedia(tabId, (entries) => {
    const existing = entries.find((e) => e.url === url && e.frameId === capture.frameId && e.documentId === capture.documentId);
    if (existing) Object.assign(existing, capture);
    else entries.push(capture);
    return entries.sort(compareMedia).slice(0, MAX_MEDIA_PER_TAB);
  });
}

function clearTabMedia(tabId, frameId) {
  return Promise.all([
    editTabMedia(tabId, (entries) => frameId == null ? [] : entries.filter((e) => e.frameId !== frameId)),
    genericMedia.clear(tabId, frameId),
  ]);
}

const genericMedia = registerGenericMedia(chrome, {
  store: MEDIA_STORE, getCaptures: (tabId) => getTabSources(tabId), download: downloadBestFrom,
  prefill: async (url, referrer, tab) => prefillVideo(url, await videoRequestHeaders(url, tab.cookieStoreId), referrer),
});

async function getTabMedia(tabId) {
  if (!tabMedia.has(tabId)) await editTabMedia(tabId, (entries) => entries);
  await mediaWrites.get(tabId);
  return [...(tabMedia.get(tabId) || [])].sort(compareMedia);
}

/**
 * Turn the raw captures into a short, useful source list. A single HLS video
 * shows up as a master playlist *plus* one media (rendition) sub-playlist per
 * quality plus an audio sub-playlist — a dozen near-identical ".m3u8" chips.
 * Only the master is worth offering: the engine expands it into every quality
 * with audio, whereas a lone rendition playlist is video-only. We fetch each
 * captured HLS manifest (the worker can, cross-origin, with host permissions),
 * keep the masters, and drop the sub-playlists whenever a master exists.
 *
 * Fetches are bounded and best-effort: a manifest we cannot read (auth-gated,
 * timed out) is left in place rather than dropped, so we never hide a source we
 * failed to classify.
 */
async function getTabSources(tabId, since = 0, scope, classify = true) {
  let entries = await getTabMedia(tabId);
  if (scope) entries = entries.filter((e) => {
    if (e.frameId !== scope.frameId) return false;
    if (e.documentId && scope.documentId && e.documentId !== scope.documentId) return false;
    if (e.documentUrl && documentUrl(scope.url) !== e.documentUrl) return false;
    return !scope.startedAt || e.ts >= scope.startedAt;
  });
  if (since) entries = entries.filter((e) => e.ts >= since);
  return classify ? preferHlsMasters(entries) : entries;
}

async function preferHlsMasters(entries) {
  const hls = entries.filter((e) => e.kind === "hls").slice(0, 8);
  if (hls.length <= 1) return entries;

  const classified = await Promise.all(
    hls.map(async (e) => {
      try {
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), 2500);
        try {
          const res = await fetch(e.url, { credentials: "include", signal: controller.signal });
          if (!res.ok) return { url: e.url, role: "unknown" };
          const text = await res.text();
          if (/#EXT-X-STREAM-INF/i.test(text)) {
            const children = [];
            const lines = text.split(/\r?\n/);
            for (let i = 0; i < lines.length; i++) {
              let uri = lines[i].match(/^#EXT-X-MEDIA:.*\bURI="([^"]+)"/)?.[1];
              if (/^#EXT-X-STREAM-INF:/.test(lines[i])) uri = lines[i + 1]?.trim();
              if (uri && !uri.startsWith("#")) {
                try { children.push(new URL(uri, res.url || e.url).href); } catch { /* malformed manifest */ }
              }
            }
            return { url: e.url, role: "master", children };
          }
          if (/#EXTINF/i.test(text)) return { url: e.url, role: "media" };
          return { url: e.url, role: "unknown" };
        } finally { clearTimeout(timer); }
      } catch {
        return { url: e.url, role: "unknown" };
      }
    }),
  );

  const roleOf = new Map(classified.map((c) => [c.url, c.role]));
  const children = new Set(classified.flatMap((c) => c.children || []));

  // Keep masters and anything we could not classify; drop confirmed renditions.
  return entries.filter((e) => e.kind !== "hls" || roleOf.get(e.url) !== "media" || !children.has(e.url));
}

// ─── YouTube: reuse the streams the player already fetched ─────────────────
// The engine's InnerTube extractor is increasingly blocked by YouTube's bot
// detection ("Sign in to confirm you're not a bot"). But when the user is
// watching a video, YouTube's own JS has already resolved and fetched the real
// googlevideo playback URLs — fully signed, n-transformed, and poToken-bound.
// We capture those (see the media sniffer above) and hand them straight to the
// engine as a ready-to-download format, so no InnerTube call is made at all.
//
// Minimal itag → quality map for the formats the web player actually streams.
// The googlevideo URL carries no height of its own, so this is used only to
// label and rank captures; an unknown itag still downloads, just unlabeled.
const YT_ITAG = {
  // muxed (video+audio together — downloadable on its own, no merge)
  18: { h: 360, mux: true, c: "mp4" },
  22: { h: 720, mux: true, c: "mp4" },
  // adaptive H.264 (mp4) video-only
  160: { h: 144, c: "mp4" }, 133: { h: 240, c: "mp4" }, 134: { h: 360, c: "mp4" },
  135: { h: 480, c: "mp4" }, 136: { h: 720, c: "mp4" }, 137: { h: 1080, c: "mp4" },
  298: { h: 720, fps: 60, c: "mp4" }, 299: { h: 1080, fps: 60, c: "mp4" },
  // adaptive VP9 (webm) video-only
  278: { h: 144, c: "webm" }, 242: { h: 240, c: "webm" }, 243: { h: 360, c: "webm" },
  244: { h: 480, c: "webm" }, 247: { h: 720, c: "webm" }, 248: { h: 1080, c: "webm" },
  271: { h: 1440, c: "webm" }, 313: { h: 2160, c: "webm" },
  302: { h: 720, fps: 60, c: "webm" }, 303: { h: 1080, fps: 60, c: "webm" },
  308: { h: 1440, fps: 60, c: "webm" }, 315: { h: 2160, fps: 60, c: "webm" },
  // audio-only
  139: { audio: true, abr: 48, c: "mp4" }, 140: { audio: true, abr: 128, c: "mp4" },
  141: { audio: true, abr: 256, c: "mp4" },
  249: { audio: true, abr: 50, c: "webm" }, 250: { audio: true, abr: 70, c: "webm" },
  251: { audio: true, abr: 160, c: "webm" },
};

/** Classify a captured googlevideo URL by its `itag`/`mime` params, or null if
 * it isn't a usable standalone stream (not googlevideo, or a SABR request whose
 * media only comes back through the player's protobuf POST body). */
function parseYtStream(url) {
  if (!/googlevideo\.com\/videoplayback/i.test(url)) return null;
  let itag, mime;
  try {
    const u = new URL(url);
    if (u.searchParams.get("sabr") === "1") return null;
    itag = Number(u.searchParams.get("itag"));
    mime = decodeURIComponent(u.searchParams.get("mime") || "");
  } catch {
    return null;
  }
  const info = YT_ITAG[itag] || {};
  const isAudio = Boolean(info.audio) || /^audio\//i.test(mime);
  const isVideo = !isAudio && (info.h != null || /^video\//i.test(mime));
  if (!isAudio && !isVideo) return null;
  return {
    url,
    itag,
    isAudio,
    isVideo,
    height: info.h ?? null,
    fps: info.fps ?? null,
    abr: info.abr ?? null,
    container: info.c || (/webm/i.test(mime) ? "webm" : "mp4"),
    mux: Boolean(info.mux),
  };
}

/**
 * Build a ready-to-download VideoFormat from this tab's captured YouTube
 * streams, or null when nothing usable was captured (SABR playback, or the
 * video hasn't buffered yet). Prefers the best captured video+audio pair as a
 * merge job; falls back to a muxed itag (18/22) that already carries audio.
 * The `since` window matches the capture to the video currently on screen, the
 * same way gatherSources() does, so a feed that played several clips doesn't
 * hand back a stale one.
 */
async function youtubeCaptureFormat(tabId, since) {
  let entries = await getTabMedia(tabId);
  if (since) entries = entries.filter((e) => e.ts >= since - 5000);
  const streams = entries.map((e) => parseYtStream(e.url)).filter(Boolean);
  if (!streams.length) return null;

  const videos = streams
    .filter((s) => s.isVideo)
    .sort((a, b) => (b.height ?? 0) - (a.height ?? 0) || (b.fps ?? 0) - (a.fps ?? 0));
  const audios = streams
    .filter((s) => s.isAudio)
    .sort((a, b) => (b.abr ?? 0) - (a.abr ?? 0));

  const bestVideo = videos.find((s) => !s.mux) || null;
  const bestAudio = audios[0] || null;

  // A real adaptive video+audio pair — highest quality, merged by the engine.
  if (bestVideo && bestAudio) {
    return {
      id: `yt-cap-${bestVideo.itag}+${bestAudio.itag}`,
      container: bestVideo.container === "webm" ? "webm" : "mp4",
      videoCodec: null,
      audioCodec: null,
      width: null,
      height: bestVideo.height,
      fps: bestVideo.fps,
      bitrate: null,
      filesize: null,
      videoUrl: bestVideo.url,
      audioUrl: bestAudio.url,
      requiresMerge: true,
    };
  }

  // Otherwise a muxed stream that already has audio (or, last resort, a
  // video-only stream with no captured audio to pair it with).
  const single = videos.find((s) => s.mux) || videos[0];
  if (single) {
    return {
      id: `yt-cap-${single.itag}`,
      container: single.container === "webm" ? "webm" : "mp4",
      videoCodec: null,
      audioCodec: null,
      width: null,
      height: single.height,
      fps: single.fps,
      bitrate: null,
      filesize: null,
      videoUrl: single.url,
      audioUrl: null,
      requiresMerge: false,
    };
  }
  return null;
}

if (chrome.webRequest) {
  chrome.webRequest.onBeforeRequest.addListener(
    (details) => {
      const { url, tabId } = details;
      if (tabId < 0 || !/^https?:/i.test(url)) return;
      if (details.type === "main_frame" || details.type === "sub_frame") {
        void clearTabMedia(tabId, details.type === "main_frame" ? undefined : details.frameId).catch(() => {});
        return;
      }
      if (SEGMENT_RE.test(url) || SEGMENTY_FILE_RE.test(url)) return;
      if (MEDIA_EXT_RE.test(url) || PROGRESSIVE_HINT_RE.test(url)) {
        const normalized = stripByteRange(normalizeGooglevideoUrl(url));
        void recordMedia(tabId, normalized, mediaKind(normalized), details).catch(() => {});
      }
    },
    { urls: ["http://*/*", "https://*/*"] },
  );

  // Catch manifests served from extensionless URLs by their Content-Type.
  chrome.webRequest.onHeadersReceived.addListener(
    (details) => {
      const { url, tabId, responseHeaders } = details;
      if (tabId < 0 || !/^https?:/i.test(url) || SEGMENT_RE.test(url) || SEGMENTY_FILE_RE.test(url)) return;
      const ct = responseHeaders?.find((h) => h.name.toLowerCase() === "content-type")?.value || "";
      const kind = kindFromContentType(ct);
      if (kind) void recordMedia(tabId, stripByteRange(normalizeGooglevideoUrl(url)), kind, details).catch(() => {});
    },
    { urls: ["http://*/*", "https://*/*"] },
    ["responseHeaders"],
  );
}

// Actual document requests clear captures above. Fragment changes do not.
if (chrome.tabs?.onUpdated) {
  chrome.tabs.onUpdated.addListener((tabId, info, tab) => {
    if (info.status === "complete" && tab?.url) {
      void syncSiteCookiesForTab(tab.url);
    }
  });
}
if (chrome.tabs?.onRemoved) {
  chrome.tabs.onRemoved.addListener((tabId) => {
    void clearTabMedia(tabId).then(async () => {
      tabMedia.delete(tabId);
      await MEDIA_STORE.remove(mediaKey(tabId));
    }).catch(() => {});
  });
}

// The keyboard shortcut opens or folds the Dispatch Board pill in the focused tab.
chrome.commands?.onCommand?.addListener(async (command) => {
  if (command !== "toggle-pill") return;
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (tab?.id != null) await chrome.tabs.sendMessage(tab.id, { type: "zoltraak-toggle-pill" }, { frameId: 0 });
  } catch {
    // The page has no content script (a browser page, say); nothing to toggle.
  }
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message?.type === "zoltraak-theme") {
    syncExtensionTheme().then(sendResponse).catch(() => sendResponse({ ok: false }));
    return true;
  }
  if (message?.type === "zoltraak-prepare-reload") {
    queueInjectedTabsForRefresh()
      .then(() => sendResponse({ ok: true }))
      .catch(() => sendResponse({ ok: false }));
    return true;
  }
  if (message?.type === "zoltraak-open-placement") {
    const url = chrome.runtime.getURL("placement.html");
    chrome.tabs.create({ url });
    sendResponse({ ok: true });
    return true;
  }
  if (message?.type?.startsWith("zoltraak-intercept-")) {
    handleInterception(message, sender)
      .then(sendResponse)
      .catch((error) => sendResponse({ ok: false, error: error.message }));
    return true;
  }
  if (message?.type === "zoltraak-health") {
    health()
      .then((up) => sendResponse({ ok: true, up }))
      .catch(() => sendResponse({ ok: false, up: false }));
    return true;
  }
  if (message?.type === "zoltraak-tab-media") {
    const tabId = sender?.tab?.id;
    if (typeof tabId !== "number") {
      sendResponse({ ok: false, media: [] });
      return true;
    }
    getTabSources(tabId, message.since || 0, { frameId: sender.frameId ?? 0,
      documentId: sender.documentId, url: sender.url, startedAt: message.documentStartedAt })
      .then((media) => sendResponse({ ok: true, media }))
      .catch(() => sendResponse({ ok: false, media: [] }));
    return true;
  }
  if (message?.type === "zoltraak-youtube-capture") {
    const tabId = sender?.tab?.id;
    const webpageUrl = message.webpageUrl;
    (async () => {
      // 1. Best path: reuse the real playback URLs YouTube's own player already
      //    fetched. googlevideo URLs are self-authenticating via their embedded
      //    signature (and carry their own pot) — they need no cookies, and a
      //    large YouTube cookie can even draw a 413 from the CDN, so the
      //    download uses an empty header set.
      if (typeof tabId === "number" && !message.videoContext?.playerBinding) {
        const format = await youtubeCaptureFormat(tabId, message.since || 0);
        if (format) {
          const started = await startVideoDownload({
            title: message.title || "YouTube video",
            webpageUrl,
            format,
            headers: [],
          });
          if (started.ok && started.id) await addSessionEntry(started.id, "video");
          if (started.ok) {
            return { ...started, label: describeFormat(format), complete: playsWithAudioFmt(format), url: webpageUrl };
          }
        }
      }

      // 2. Fallback: the engine's InnerTube extractor, armed by the resolver
      //    with the browser's poToken + visitorData (+ session cookie) so it
      //    clears the "confirm you're not a bot" check.
      return downloadBestFrom({ ...message, candidates: [webpageUrl] }, sender);
    })()
      .then((res) => sendResponse(res))
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-analyze-best") {
    resolveWithHlsFallback(message, sender)
      .then((res) => sendResponse(res.ok ? { ok: true, url: res.url, metadata: res.metadata }
        : { ...res, choices: res.choices?.map(({ url, metadata, identity }) => ({ url, metadata, identity })) }))
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-download-best") {
    (async () => {
      let source = sender;
      if (!sender.tab && sender.url?.startsWith(chrome.runtime.getURL("")) && Number.isInteger(message.tabId)) {
        const tab = await chrome.tabs.get(message.tabId);
        source = { tab, frameId: 0, url: tab.url };
      }
      return downloadBestFrom(message, source);
    })()
      .then((res) => sendResponse(res))
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-prefill-download") {
    videoRequestHeaders(message.url)
      .then((headers) => prefillDownload(message.url, headers, message.referrer))
      .then((res) => sendResponse(res))
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-sync-session") {
    // The popup's "share session now" — an explicit push, throttle or not.
    const domain = syncDomainForHost(message.host || "");
    if (!domain) {
      sendResponse({ ok: false, error: "This site has no login-gated extractor." });
      return true;
    }
    syncSession(domain, { force: true })
      .then((res) =>
        sendResponse(
          res.ok
            ? { ok: true, domain }
            : { ok: false, error: "The app did not accept it — check pairing below." },
        ),
      )
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-refresh-sessions") {
    // The popup's "refresh every shared sign-in". Every domain failing at once
    // means the app is not reachable or not paired, not that the cookies are
    // bad — say which, rather than reporting a per-site failure list.
    refreshAllSessions()
      .then((res) =>
        sendResponse(
          res.failed.length && !res.shared.length && !res.cleared.length
            ? { ok: false, error: "Could not reach the Zoltraak app. Is it running and paired?" }
            : { ok: true, ...res },
        ),
      )
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-prefill-post") {
    (async () => {
      const url = globalThis.ZoltraakVideoIdentity.socialPostUrl(message.url);
      if (!url || !message.playerBinding || !(sender?.tab?.id >= 0)) {
        throw new Error("Find the current post again before opening Images.");
      }
      await assertCurrentPlayer(message.playerBinding, sender, message.documentStartedAt);
      const scope = sender.documentId ? { documentId: sender.documentId } : { frameId: sender.frameId ?? 0 };
      const current = await chrome.tabs.sendMessage(sender.tab.id,
        { type: "zoltraak-post-permalink", playerBinding: message.playerBinding }, scope);
      if (globalThis.ZoltraakVideoIdentity.socialPostUrl(current?.url) !== url
          || current?.documentStartedAt !== message.documentStartedAt) {
        throw new Error("The post changed. Find its video again before opening Images.");
      }
      const headers = await videoRequestHeaders(url, sender.tab.cookieStoreId);
      const browserDocuments = await facebookAttachments(url, sender.tab.id, scope);
      return prefillImages(url, headers, sender.url || sender.tab.url, ...(browserDocuments ? [browserDocuments] : []));
    })().then(sendResponse).catch((error) => sendResponse({ ok: false, error: String(error?.message || error) }));
    return true;
  }
  if (message?.type === "zoltraak-prefill-video") {
    // For YouTube, always normalise the URL to the canonical `watch?v=<id>`
    // form and pull cookies from youtube.com (not from whatever embed/shorts
    // URL the overlay happened to be on). This lets the engine's InnerTube
    // extractor receive the user's session for age-gated content and ensures
    // the right video is fetched regardless of the URL shape on screen.
    let prefillUrl = message.url;
    const ytIdMatch = prefillUrl?.match(
      /(?:\/shorts\/|\/watch\?v=|\/v\/|\/embed\/|\/live\/|youtu\.be\/)([a-zA-Z0-9_-]{11})/
    ) || prefillUrl?.match(/[?&]v=([a-zA-Z0-9_-]{11})/);
    if (ytIdMatch?.[1]) {
      prefillUrl = `https://www.youtube.com/watch?v=${ytIdMatch[1]}`;
    }
    const isYouTube = /^https?:\/\/(?:[a-z0-9-]+\.)*(youtube\.com|youtu\.be)\//i.test(prefillUrl);
    const headerSource = isYouTube ? "https://www.youtube.com/watch" : prefillUrl;
    (async () => {
      let headers = await videoRequestHeaders(headerSource, sender?.tab?.cookieStoreId);
      // Ride the browser's harvested poToken along so the engine's InnerTube
      // call is authorized when the Video view resolves this YouTube URL.
      if (isYouTube) headers = headers.concat(await youtubeAuthHeaders());
      return prefillVideo(prefillUrl, headers, message.referrer, message.autoDownload, message.title);
    })()
      .then((res) => sendResponse(res))
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-analyze") {
    videoRequestHeaders(message.url, sender?.tab?.cookieStoreId)
      .then((headers) => analyzeVideo(message.url, message.referrer, headers))
      .then((res) => sendResponse(res))
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-download") {
    videoRequestHeaders(message.webpageUrl, sender?.tab?.cookieStoreId)
      .then(async (headers) => {
        await assertCurrentPlayer(message.playerBinding, sender, message.documentStartedAt);
        return headers;
      })
      .then((headers) => startVideoDownload({ ...message, headers: downloadHeadersFor(message.webpageUrl, headers) }))
      .then(async (res) => {
        if (res.ok && res.id) await addSessionEntry(res.id, "video");
        return res;
      })
      .then((res) => sendResponse(res))
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  if (message?.type === "zoltraak-get-snapshot") {
    getSnapshot()
      .then((res) => sendResponse(res))
      .catch(() => sendResponse({ ok: false, items: [], videoJobs: [] }));
    return true;
  }
  if (message?.type === "zoltraak-get-session") {
    getSessionSnapshot()
      .then((res) => sendResponse(res))
      .catch(() => sendResponse({ ok: false, items: [], paired: false }));
    return true;
  }
  if (message?.type === "zoltraak-reveal") {
    revealTarget(message.id, message.path)
      .then((res) => sendResponse(res))
      .catch(() => sendResponse({ ok: false }));
    return true;
  }
  if (message?.type === "zoltraak-open") {
    openTarget(message.id, message.path)
      .then((res) => sendResponse(res))
      .catch(() => sendResponse({ ok: false }));
    return true;
  }
  if (message?.type === "zoltraak-set-state") {
    setDownloadState(message.id, message.action)
      .then((res) => sendResponse(res))
      .catch(() => sendResponse({ ok: false }));
    return true;
  }
  if (message?.type === "zoltraak-remove") {
    removeDownload(message.id)
      .then(async (res) => {
        if (res.ok) await removeSessionEntry(message.id);
        return res;
      })
      .then((res) => sendResponse(res))
      .catch(() => sendResponse({ ok: false }));
    return true;
  }
  if (message?.type === "zoltraak-clear-session") {
    clearFinishedSessionEntries(message.ids || [])
      .then(() => sendResponse({ ok: true }))
      .catch(() => sendResponse({ ok: false }));
    return true;
  }
  if (message?.type === "zoltraak-retry") {
    (async () => {
      await capture(message.url, undefined, message.filename);
      await removeSessionEntry(message.oldId);
    })()
      .then(() => sendResponse({ ok: true }))
      .catch((err) => sendResponse({ ok: false, error: String(err?.message || err) }));
    return true;
  }
  return false;
});

function formatXUrl(url) {
  if (!url || typeof url !== "string") return url;
  if (/https?:\/\/(?:[a-z0-9-]+\.)*(?:x\.com|twitter\.com)\//i.test(url)) {
    return url.split("?")[0];
  }
  return url;
}

/** Resolve the post under the context-menu click and open it in Images. */
async function facebookAttachments(url, tabId, scope) {
  if (!/^https:\/\/(?:[a-z0-9-]+\.)*facebook\.com\//i.test(url) || !await canUseBrowserCookies()) return undefined;
  try {
    const result = await chrome.tabs.sendMessage(tabId, { type: "zoltraak-facebook-attachments", url }, scope);
    return Array.isArray(result?.browserDocuments) && result.browserDocuments.length ? result.browserDocuments : undefined;
  } catch { return undefined; }
}

async function sendPostToApp(info, tab) {
  let url = null;
  if (typeof tab?.id === "number") {
    try {
      const res = await chrome.tabs.sendMessage(
        tab.id,
        { type: "zoltraak-post-permalink" },
        { frameId: info.frameId ?? 0 },
      );
      url = res?.url || null;
    } catch {
      // The content script may be unavailable on browser-internal pages.
    }
  }
  // The browser's clicked-link URL is the most exact context we have. On
  // Instagram it can include the `/c/<comment-id>/` route that the extractor
  // needs, while the DOM fallback may resolve only the surrounding `/p/` URL.
  url = formatXUrl(globalThis.ZoltraakVideoIdentity.contextPostTarget(
    info.linkUrl, url, info.frameUrl || info.pageUrl || tab?.url));
  if (!url) {
    const tiktok = /^https?:\/\/(?:[a-z0-9-]+\.)*tiktok\.com(?:\/|$)/i.test(tab?.url || "");
    showToastNotification("Post link not found", tiktok
      ? "Open this TikTok post, or use Share > Copy link and paste it in Zoltraak."
      : "Right-click the post's timestamp or open the post, then try again.");
    flashBadge(false);
    return;
  }

  const headers = await videoRequestHeaders(url, tab?.cookieStoreId);
  const browserDocuments = await facebookAttachments(url, tab?.id, { frameId: info.frameId ?? 0 });
  const result = await prefillImages(url, headers, tab?.url, ...(browserDocuments ? [browserDocuments] : []));
  showToastNotification(
    result.ok ? "↗ Sent to Zoltraak (Images)" : "❌ Could not reach Zoltraak",
    result.ok ? url : result.error || url,
  );
  flashBadge(result.ok);
}

/**
 * Hand a URL coming from the browser to the desktop app's downloads text input.
 * The app brings its window forward, opens the downloads composer, and pastes the URL in.
 */
/**
 * Hand a music link to the app's Music view. YouTube Music links ride with
 * the YouTube session so a private playlist resolves; the other services need
 * nothing, and get nothing.
 */
async function sendMusicToApp(url, tab) {
  if (!url) return;
  let headers = [];
  if (/^https?:\/\/(?:[a-z0-9-]+\.)*(youtube\.com|youtu\.be)\//i.test(url)) {
    headers = await videoRequestHeaders("https://www.youtube.com/watch");
    headers = headers.concat(await youtubeAuthHeaders());
  }
  const result = await prefillMusic(url, headers, tab?.url);
  showToastNotification(
    result.ok ? "↗ Sent to Zoltraak (Music)" : "❌ Could not reach Zoltraak",
    result.ok ? url : result.error || url,
  );
  flashBadge(result.ok);
}

async function sendDownloadToApp(url, tab) {
  url = formatXUrl(url);
  if (!url) return;
  const result = await offerDownload({ url, referrer: tab?.url }, tab?.id);
  showToastNotification(
    result.ok ? "↗ Sent to Zoltraak" : "❌ Could not reach Zoltraak",
    result.ok ? url : result.error || url,
  );
  flashBadge(result.ok);
}

// Resolve the selected clip before choosing quality. Weak matches are offered
// for explicit inspection; duration, dimensions and tab traffic cannot prove
// that a stream belongs to the player the user selected.

const MAX_CANDIDATES = 5;

const playsWithAudioFmt = (f) => Boolean(f.requiresMerge) || f.hasAudio !== false;

/** A human label for what was started: "1080p", "720p · 60fps", "audio". */
function describeFormat(format) {
  if (globalThis.ZoltraakVideoIdentity.isAudioOnly(format)) return "audio";
  const parts = [];
  if (format.height) parts.push(`${format.height}p`);
  else if (format.bitrate) parts.push(`${Math.round(format.bitrate / 1000)}kbps`);
  if (format.fps && format.fps >= 48) parts.push(`${Math.round(format.fps)}fps`);
  return parts.join(" · ") || format.id || "best quality";
}

/** Analyze bounded candidates without changing signed URLs or mixing clips. */
async function resolveCandidates({ candidates, referrer, title, videoContext, sourceKinds }, sender) {
  if (videoContext?.protected) return { ok: false, error: "This media is protected." };
  if (videoContext?.live) return { ok: false, error: "Live media requires a recording duration. Open the source in the app." };
  sourceKinds ||= videoContext?.sourceKinds;
  const seen = new Set();
  const seenItems = new Set();
  const rawList = candidates || [];
  const urls = rawList
    .map((url) => globalThis.ZoltraakVideoIdentity.completeMediaUrl(url))
    .filter((u) => typeof u === "string" && /^https?:\/\//i.test(u) && !seen.has(u) && seen.add(u))
    .filter((url) => {
      const item = globalThis.ZoltraakVideoIdentity.itemIdentity(url);
      if (!item?.startsWith("instagram:")) return true;
      if (seenItems.has(item)) return false;
      seenItems.add(item);
      return true;
    })
    .slice(0, MAX_CANDIDATES);
  if (urls.length === 0) return { ok: false, error: "Nothing to download here." };

  const results = [];
  let lastError = null;
  for (const url of urls) {
    const isYouTube = isYouTubeUrl(url);
    let headers = await videoRequestHeaders(isYouTube ? "https://www.youtube.com/watch" : url, sender?.tab?.cookieStoreId);
    if (isYouTube) headers = headers.concat(await youtubeAuthHeaders());
    let analyzed = await analyzeVideo(url, referrer, headers, sourceKinds?.[url], videoContext?.mediaType);
    // Instagram's shared resolver already tries its supported stages. Empty
    // request headers still inherit the vault session, so retrying here repeats
    // the same failure and can worsen a rate limit.
    if (!analyzed.ok && headers.length > 0 && !isYouTube
        && !globalThis.ZoltraakVideoIdentity.itemIdentity(url)?.startsWith("instagram:")
        && !/^https?:\/\/(?:[a-z0-9-]+\.)*tiktok\.com(?:\/|$)/i.test(url)) {
      const retry = await analyzeVideo(url, referrer, [], sourceKinds?.[url], videoContext?.mediaType);
      if (retry.ok) { analyzed = retry; headers = []; }
    }
    if (!analyzed.ok) { lastError = analyzed.error; continue; }
    const carousel = analyzed.metadata.formats?.some((f) => /^clip-\d+-/.test(f.id || ""));
    const verification = verifyAndScoreCandidate(analyzed.metadata, url, carousel && videoContext
      ? { ...videoContext, duration: null, width: null, height: null } : videoContext);
    if (!verification.valid) { lastError = verification.reason; continue; }
    const itemMatch = verification.evidence.includes("item-id");
    let formats = rankFormats(itemMatch && !carousel ? analyzed.metadata.formats
      : filterFormatsByVideoContext(analyzed.metadata.formats, videoContext));
    if (videoContext?.mediaType === "audio") {
      const audioFormats = formats.filter((f) => globalThis.ZoltraakVideoIdentity.isAudioOnly(f));
      formats = audioFormats.length ? audioFormats : formats.filter((f) => f.hasAudio !== false);
    }
    else if (videoContext) formats = formats.filter((f) => !globalThis.ZoltraakVideoIdentity.isAudioOnly(f));
    if (!formats.some((f) => f.videoUrl)) continue;
    const metadata = { ...analyzed.metadata, formats };
    if (!metadata.title || metadata.title === "video") metadata.title = title || "Video";
    if (!metadata.webpageUrl) metadata.webpageUrl = url;
    const result = { ok: true, url, metadata, headers: downloadHeadersFor(url, headers),
      identity: verification, playerBinding: videoContext?.playerBinding };
    const clips = new Map();
    for (const format of formats) {
      const clip = format.id?.match(/^clip-(\d+)-/)?.[1];
      if (clip) clips.set(clip, [...(clips.get(clip) || []), format]);
    }
    if (videoContext && clips.size) {
      for (const [clip, formats] of clips) {
        const one = { ...metadata, formats, title: `${metadata.title} · video ${clip}`,
          thumbnail: formats[0].downloadOptions?.thumbnail,
          duration: formats[0].validation?.duration };
        const identity = verifyAndScoreCandidate(one, formats[0].videoUrl, videoContext);
        // A post ID identifies the carousel, not which of its clips is playing.
        if (identity.valid) results.push({ ...result, metadata: one,
          identity: { ...identity, strong: identity.evidence.includes("player-source") } });
      }
      continue;
    }
    // A page scraper may return unrelated files. Only a known item extractor
    // or a player-bound manifest can establish that its renditions form one clip.
    const manifestMatch = videoContext?.elementManifest &&
      globalThis.ZoltraakVideoIdentity.sameMedia(url, videoContext.elementManifest);
    // The selected item's extractor already returned its full quality list.
    // Do not wait for unrelated captures or analyze the same post URL again.
    if (verification.strong && (itemMatch || manifestMatch)) return result;
    if (formats.length > 1 && !itemMatch && !manifestMatch && !/\.(m3u8|mpd)(?:[?#]|$)/i.test(url)) {
      for (const format of formats) {
        const one = { ...metadata, formats: [format] };
        const identity = verifyAndScoreCandidate(one, format.videoUrl, videoContext);
        if (identity.valid) results.push({ ...result, metadata: one, identity });
      }
    } else results.push(result);
  }
  const byStreams = new Map();
  for (const result of results) {
    const key = JSON.stringify(result.metadata.formats.map((f) => [
      globalThis.ZoltraakVideoIdentity.mediaIdentity(f.videoUrl),
      globalThis.ZoltraakVideoIdentity.mediaIdentity(f.audioUrl)]));
    const previous = byStreams.get(key);
    if (!previous || result.identity.score > previous.identity.score) byStreams.set(key, result);
  }
  const unique = [...byStreams.values()];
  const strong = unique.filter((r) => r.identity.strong);
  if (strong.length === 1) return strong[0];
  // An explicit URL with no selected player remains a valid user choice. With
  // player context, even one weak result needs confirmation.
  if (!videoContext && unique.length === 1) return unique[0];
  if (unique.length) return { ok: false, choices: unique.sort((a, b) => b.identity.score - a.identity.score),
    error: "Choose the video to inspect before downloading." };
  return { ok: false, error: lastError || "No downloadable video found." };
}

function isYouTubeUrl(url) {
  return /^https?:\/\/(?:[a-z0-9-]+\.)*(youtube\.com|youtu\.be)\//i.test(url || "");
}

/** googlevideo URLs are self-signed and a large YouTube cookie can draw a 413
 * from the CDN, so a YouTube download goes out with no headers at all. */
function downloadHeadersFor(url, headers) {
  return isYouTubeUrl(url) ? [] : headers;
}

/** The same order the app's picker shows: sound first, then resolution,
 * bitrate, size — so the overlay and the Video view agree on "best". */
function rankFormats(formats) {
  return [...(formats || [])].sort(
    (a, b) =>
      (b.requiresMerge || b.hasAudio === true ? 2 : b.hasAudio == null ? 1 : 0) -
      (a.requiresMerge || a.hasAudio === true ? 2 : a.hasAudio == null ? 1 : 0) ||
      (b.height ?? 0) - (a.height ?? 0) ||
      (b.bitrate ?? 0) - (a.bitrate ?? 0) ||
      (b.filesize ?? 0) - (a.filesize ?? 0),
  );
}

async function resolveWithHlsFallback(options, sender) {
  const primary = await resolveCandidates(options, sender);
  if (options.genericGuard || options.videoContext?.protected || options.videoContext?.live) return primary;
  if (primary.choices?.length || (primary.ok && primary.metadata?.formats?.length)) return primary;
  if (!(sender?.tab?.id >= 0)) return primary;

  const captured = await getTabSources(sender.tab.id, 0, { frameId: sender.frameId ?? 0,
    documentId: sender.documentId, url: sender.url, startedAt: options.documentStartedAt }, false);
  const rawUrls = [...new Set([
    ...(options.hlsCandidates || []).filter((url) => typeof url === "string" && /^https?:\/\//i.test(url) && /\.m3u8(?:[?#]|$)/i.test(url)),
    ...captured.filter((entry) => entry.kind === "hls").map((entry) => entry.url),
  ])].slice(0, 8);
  const urls = (await preferHlsMasters(rawUrls.map((url) => ({ url, kind: "hls" })))).map((entry) => entry.url);
  if (!urls.length) return { ok: false, error: `No stream detected. Play the video, then retry. ${primary.error || ""}`.trim() };

  const choices = [];
  let lastError = primary.error;
  for (const url of urls) {
    const result = await resolveCandidates({ ...options, candidates: [url] }, sender);
    if (result.ok && result.metadata.formats.length) {
      choices.push({ ...result, metadata: { ...result.metadata, webpageUrl: result.url } });
    } else if (result.choices?.length) choices.push(...result.choices);
    else lastError = result.error;
  }
  if (choices.length === 1 && (!options.videoContext || choices[0].identity?.strong)) return choices[0];
  if (choices.length > 0) return { ok: false, choices, error: "Choose a stream in the video picker before downloading." };
  return { ok: false, error: `Stream detected but could not be resolved. Resume playback and retry. ${lastError || ""}`.trim() };
}

async function assertCurrentPlayer(binding, sender, documentStartedAt) {
  if (!binding) return;
  if (!(sender?.tab?.id >= 0)) throw new Error("The selected video is no longer available. Find it again.");
  const scope = sender.documentId ? { documentId: sender.documentId } : { frameId: sender.frameId ?? 0 };
  const response = await chrome.tabs.sendMessage(sender.tab.id,
    { type: "zoltraak-check-video-identity", playerBinding: binding }, scope);
  if (!response?.ok || (documentStartedAt && response.documentStartedAt !== documentStartedAt)) {
    throw new Error("The video changed. Find this video again before downloading.");
  }
}

async function downloadBestFrom(options, sender) {
  const { referrer } = options;
  const resolved = await resolveWithHlsFallback(options, sender);
  if (!resolved.ok) {
    if (resolved.choices?.length && options.videoContext?.playerBinding && sender?.tab) {
      const shown = await chrome.tabs.sendMessage(sender.tab.id, { type: "zoltraak-show-video-choices",
        choices: resolved.choices.map(({ url, metadata }) => ({ url, metadata })),
        playerBinding: options.videoContext.playerBinding, documentStartedAt: options.documentStartedAt },
        sender.documentId ? { documentId: sender.documentId } : { frameId: sender.frameId ?? 0 }).catch(() => null);
      if (shown?.ok) return { ...resolved, error: "Choose a video in the page's video picker." };
    }
    return resolved;
  }
  const audioOnly = options.videoContext?.mediaType === "audio";
  const pick = audioOnly ? { format: rankFormats(resolved.metadata.formats)[0], complete: true } : bestCompleteFormat(resolved.metadata.formats);
  if (!pick) return { ok: false, error: "No downloadable format found." };
  await assertCurrentPlayer(options.videoContext?.playerBinding, sender, options.documentStartedAt);
  if (options.genericGuard) await options.genericGuard();
  const started = await startVideoDownload({
    title: resolved.metadata.title || "video",
    webpageUrl: resolved.metadata.webpageUrl,
    format: pick.format,
    referrer,
    headers: resolved.headers,
  });
  if (!started.ok) return { ok: false, error: started.error };
  if (started.id) await addSessionEntry(started.id, "video");
  return { ok: true, id: started.id, label: describeFormat(pick.format), complete: pick.complete, audioStatus: pick.format.validation?.status ||
    (pick.format.requiresMerge || pick.format.hasAudio != null ? "inferred" : "unknown"), url: resolved.url };
}

/**
 * The right-click menu and captured manifests: same resolver, badge feedback.
 * Reports the resolver's verdict so a caller that cancelled nothing yet — the
 * capture-all handler — can tell whether the engine actually took the job.
 */
async function captureVideo(url, referrer, sender) {
  if (!url) return { ok: false, error: "No URL to capture." };
  const result = await downloadBestFrom({ candidates: [url, referrer].filter(Boolean), referrer }, sender);
  flashBadge(result.ok);
  if (result.ok && !result.complete) {
    showToastNotification("⚠ Downloading without audio", `${result.label} — no audio track was available for ${url}`);
  }
  return result;
}

function flashBadge(ok) {
  chrome.action.setBadgeText({ text: ok ? "✓" : "!" });
  chrome.action.setBadgeBackgroundColor({ color: ok ? "#35c46f" : "#f1595e" });
  setTimeout(() => void refreshBadge(), 2500);
}

// Chromium holds filename determination until suggest() is called. Cancel
// while it is held, before the browser can display its native Save As dialog.
// Firefox lacks this event, so onCreated is the fallback there.
const interceptingDownloads = new Set();
const interceptedDownloads = new Set();

function isEngineUrl(value) {
  try { return new URL(value).origin === new URL(ENGINE_ORIGIN).origin; }
  catch { return false; }
}

async function interceptionDiagnostic(stage, reason, browserError) {
  await chrome.storage.local.set({ lastInterception: { stage, reason, browserError, at: Date.now() } }).catch(() => {});
}

async function interceptionProblem() {
  const { captureAll = true } = await chrome.storage.local.get("captureAll");
  if (!captureAll) return "Automatic capture is disabled in the extension.";
  if (!(await health())) return "The desktop app is unavailable. Start it and retry.";
  if (!(await ensurePaired())) return "Browser pairing failed. Reconnect the extension and retry.";
  return null;
}

async function interceptBrowserDownload(item) {
  if (!/^https?:\/\//i.test(item.finalUrl || item.url) || isEngineUrl(item.url) || isEngineUrl(item.finalUrl)) return;
  if (interceptingDownloads.has(item.id) || interceptedDownloads.has(item.id)) return;
  interceptingDownloads.add(item.id);
  try {
    if (item.state === "complete" || item.state === "interrupted") {
      await interceptionDiagnostic("browser", item.state === "complete" ? "The browser already finished this download." : "The browser interrupted this download. Check its Downloads page.", item.error);
      return;
    }
    const problem = await interceptionProblem();
    if (problem) { await interceptionDiagnostic("skipped", problem); return; }
    const draft = await saveDraft({
      url: item.finalUrl || item.url, referrer: item.referrer,
      filename: item.filename, size: item.fileSize || item.totalBytes,
    }, item.tabId);
    // Persist before cancelling; only remove the draft if the browser keeps it.
    try {
      await chrome.downloads.cancel(item.id);
    } catch {
      await chrome.storage.local.remove(draftKey(draft.id));
      const current = await chrome.downloads.search?.({ id: item.id }).catch(() => []);
      await interceptionDiagnostic("cancel", "Could not take over the download. Check the browser's Downloads page.", current?.[0]?.error);
      return;
    }
    interceptedDownloads.add(item.id);
    if (interceptedDownloads.size > 200) interceptedDownloads.delete(interceptedDownloads.values().next().value);
    await chrome.downloads.erase({ id: item.id }).catch(() => {});
    try {
      await presentDraft(draft);
      await interceptionDiagnostic("captured", "Download captured. Choose Download now or Later.");
    } catch {
      await interceptionDiagnostic("dialog", "Download saved as a pending draft, but its dialog could not open. Reopen the extension to recover it.");
    }
  } catch {
    await interceptionDiagnostic("failed", "Capture could not be prepared. The browser download was left unchanged.");
  } finally { interceptingDownloads.delete(item.id); }
}
const downloadsApi = chrome.downloads;
const hasDeterminingFilename = Boolean(downloadsApi && downloadsApi["onDeterminingFilename"]);
if (hasDeterminingFilename) {
  downloadsApi["onDeterminingFilename"].addListener((item, suggest) => {
    interceptBrowserDownload(item).catch(console.error).finally(() => suggest());
    return true;
  });
} else {
  chrome.downloads.onCreated.addListener((item) => interceptBrowserDownload(item).catch(console.error));
}

// Browser security/network failures can precede filename determination. Keep
// their actual interruption reason instead of calling every HTTP failure a
// disconnected engine. Our own successful cancellation is not a failure.
chrome.downloads.onChanged?.addListener((delta) => {
  if (delta.state?.current !== "interrupted" || delta.error?.current === "USER_CANCELED"
      || interceptingDownloads.has(delta.id) || interceptedDownloads.has(delta.id)) return;
  void (async () => {
    const [item] = await chrome.downloads.search({ id: delta.id });
    if (item && /^https?:/i.test(item.finalUrl || item.url) && !isEngineUrl(item.url) && !isEngineUrl(item.finalUrl)) {
      await interceptionDiagnostic("browser", "The browser interrupted this download. Check its Downloads page.", delta.error?.current || item.error);
    }
  })().catch(() => {});
});

const draftKey = (id) => `intercept:${id}`;
const submittingDrafts = new Set();
// Firefox supports blocking response headers in MV3. Stop attachment GETs
// before its download helper (and native picker) is created. Chromium uses
// onDeterminingFilename above and must not register a blocking listener.
if (typeof browser !== "undefined" && browser.webRequest && !hasDeterminingFilename) {
  browser.webRequest.onHeadersReceived.addListener(async (details) => {
    if (isEngineUrl(details.url)) return {};
    if (details.method !== "GET" || details.tabId < 0 || !details.responseHeaders?.some(
      (header) => header.name.toLowerCase() === "content-disposition" && /^attachment\b/i.test(header.value || ""),
    )) return {};
    try {
      const problem = await interceptionProblem();
      if (problem) { await interceptionDiagnostic("skipped", problem); return {}; }
      const disposition = details.responseHeaders.find((h) => h.name.toLowerCase() === "content-disposition").value;
      let filename = disposition.match(/filename\s*=\s*"([^"]+)"/i)?.[1]
        || disposition.match(/filename\s*=\s*([^;\s]+)/i)?.[1];
      const encoded = disposition.match(/filename\*\s*=\s*UTF-8''([^;\s]+)/i)?.[1];
      if (encoded) { try { filename = decodeURIComponent(encoded); } catch { /* retain plain name */ } }
      const draft = await saveDraft({ url: details.url, referrer: details.originUrl, filename }, details.tabId);
      try { await presentDraft(draft); }
      catch {
        await chrome.storage.local.remove(draftKey(draft.id));
        await interceptionDiagnostic("dialog", "Could not open the download dialog. The browser will handle this download.");
        return {};
      }
      await interceptionDiagnostic("captured", "Download captured. Choose Download now or Later.");
      return { cancel: true };
    } catch { return {}; }
  }, { urls: ["http://*/*", "https://*/*"], types: ["main_frame", "sub_frame"] }, ["blocking", "responseHeaders"]);
}
async function interceptionReady() {
  const { captureAll = true } = await chrome.storage.local.get("captureAll");
  return captureAll && await health() && await ensurePaired();
}

function dispositionFilename(disposition) {
  const encoded = disposition.match(/filename\*\s*=\s*UTF-8''([^;\s]+)/i)?.[1];
  if (encoded) { try { return decodeURIComponent(encoded); } catch { /* fall through */ } }
  return disposition.match(/filename\s*=\s*"([^"]+)"/i)?.[1]
    || disposition.match(/filename\s*=\s*([^;\s]+)/i)?.[1];
}

async function serverFilename(url, headers) {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 4000);
  try {
    const sent = {};
    for (const [name, value] of headers || []) {
      if (name.toLowerCase() !== "cookie") sent[name] = value;
    }
    const res = await fetch(url, { headers: { ...sent, Range: "bytes=0-0" }, credentials: "include", signal: ctl.signal });
    const name = dispositionFilename(res.headers.get("content-disposition") || "");
    return name ? name.split(/[\\/]/).pop().trim() : undefined;
  } catch { return undefined; } finally { clearTimeout(timer); ctl.abort(); }
}

async function saveDraft(input, tabId) {
  if (!/^https?:\/\//i.test(input.url || "")) throw new Error("Only HTTP and HTTPS downloads are supported.");
  let filename = (input.filename || "").split(/[\\/]/).pop();
  const needsProbe = !filename;
  if (!filename) {
    try { filename = decodeURIComponent(new URL(input.url).pathname.split("/").pop()); } catch { /* use fallback */ }
  }
  // DownloadItem has no tabId in Chromium; locate the referring page before
  // reading cookies so Firefox container sessions stay with their own tab.
  const tab = tabId >= 0 ? await chrome.tabs.get(tabId).catch(() => null)
    : (await chrome.tabs.query({})).find((tab) => input.referrer && tab.url === input.referrer);
  const cookieStoreId = tab?.cookieStoreId;
  const headers = await videoRequestHeaders(input.url, cookieStoreId);
  // The URL's last segment is often a route name ("download"), not the file.
  // Ask the server what it would call the file so the dialog shows that name.
  if (needsProbe) filename = await serverFilename(input.url, headers) || filename;
  const draft = {
    id: crypto.randomUUID(), url: input.url, referrer: input.referrer,
    filename: filename || "download", size: input.size, tabId, createdAt: Date.now(),
    cookieStoreId,
    headers,
  };
  await chrome.storage.local.set({ [draftKey(draft.id)]: draft });
  return draft;
}

async function presentDraft(draft) {
  let tabId = draft.tabId;
  if (!(tabId >= 0)) {
    const tabs = await chrome.tabs.query({});
    // Chromium's DownloadItem has no tabId. Prefer its referring page over
    // whichever tab the user happened to switch to while the response arrived.
    tabId = tabs.find((tab) => draft.referrer && tab.url === draft.referrer)?.id
      ?? tabs.find((tab) => tab.active && /^https?:/i.test(tab.url || ""))?.id;
  }
  if (tabId >= 0) {
    draft.tabId = tabId;
    await chrome.storage.local.set({ [draftKey(draft.id)]: draft });
    try {
      const shown = await chrome.tabs.sendMessage(tabId, { type: "zoltraak-show-interception" }, { frameId: 0 });
      if (shown?.ok) return;
    } catch { /* restricted or not-yet-loaded page: use extension-owned dialog */ }
  }
  await chrome.tabs.create({ url: chrome.runtime.getURL("intercept.html") });
}

async function offerDownload(input, tabId, automatic = false) {
  if (isEngineUrl(input.url)) return { ok: false, error: "The desktop app handles its own downloads." };
  const problem = automatic ? await interceptionProblem() : !(await health() && await ensurePaired()) ? "Reconnect the extension to the desktop app and retry." : null;
  if (problem) {
    await interceptionDiagnostic("skipped", problem);
    return { ok: false, error: problem };
  }
  const draft = await saveDraft(input, tabId);
  try { await presentDraft(draft); }
  catch {
    await chrome.storage.local.remove(draftKey(draft.id));
    const error = "Could not open the download dialog. The browser will handle this download.";
    await interceptionDiagnostic("dialog", error);
    return { ok: false, error };
  }
  await interceptionDiagnostic("captured", "Download captured. Choose Download now or Later.");
  return { ok: true };
}

async function handleInterception(message, sender) {
  const ownPage = sender.url?.startsWith(chrome.runtime.getURL("intercept.html"));
  const tabId = sender.tab?.id;
  if (!ownPage && !(tabId >= 0)) throw new Error("A browser tab is required.");
  if (message.type === "zoltraak-intercept-status") return { ok: true, ready: await interceptionReady() };
  if (message.type === "zoltraak-intercept-link") {
    return offerDownload({ url: message.url, filename: message.filename, referrer: sender.url }, tabId, true);
  }
  if (message.type === "zoltraak-intercept-list") {
    const all = await chrome.storage.local.get(null);
    const drafts = Object.entries(all).filter(([key, d]) => key.startsWith("intercept:") && (ownPage || d.tabId === tabId))
      .map(([, { headers, ...d }]) => d).sort((a, b) => a.createdAt - b.createdAt);
    const [categoriesRes, settingsRes] = await Promise.all([
      apiFetch("/categories").catch(() => null),
      apiFetch("/settings").catch(() => null),
    ]);
    const categoriesData = categoriesRes?.ok ? await categoriesRes.json().catch(() => []) : [];
    const categories = Array.isArray(categoriesData) ? categoriesData : [];
    let defaultDirectory = "";
    if (settingsRes?.ok) {
      const settings = await settingsRes.json().catch(() => null);
      if (settings && typeof settings === "object" && !Array.isArray(settings)) {
        defaultDirectory = settings.resolvedDefaultDirectory || settings.defaultDirectory || settings.default_directory || "";
      }
    }
    if (!defaultDirectory) {
      const { rememberedDirectory } = await chrome.storage.local.get("rememberedDirectory");
      if (rememberedDirectory) defaultDirectory = rememberedDirectory;
    }
    return { ok: true, drafts, categories, defaultDirectory };
  }
  if (message.type === "zoltraak-intercept-cancel-all") {
    const all = await chrome.storage.local.get(null);
    const keys = Object.entries(all).filter(([key, d]) => key.startsWith("intercept:") && (ownPage || d.tabId === tabId))
      .map(([key]) => key);
    for (const k of keys) {
      await chrome.storage.local.remove(k);
    }
    return { ok: true, count: keys.length };
  }
  if (message.type === "zoltraak-intercept-create-category") {
    const name = String(message.name || "").trim();
    const kind = String(message.kind || "extension").trim();
    const pattern = String(message.pattern || "").trim();
    let directory = String(message.directory || "").trim();
    if ((directory.startsWith('"') && directory.endsWith('"')) || (directory.startsWith("'") && directory.endsWith("'"))) {
      directory = directory.slice(1, -1).trim();
    }
    if (!name) throw new Error("Category name is required.");
    if (!pattern) throw new Error("Match pattern is required.");
    if (!directory) throw new Error("Destination folder path is required.");
    if (!/^(?:[a-z]:[\\/]|\\\\[^\\]+\\[^\\]+|\/)/i.test(directory)) {
      throw new Error("Enter an absolute destination folder path (e.g. C:\\Downloads or /home/...).");
    }
    const res = await apiFetch("/categories", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        name,
        kind,
        pattern,
        directory,
        priority: 0,
      }),
    });
    if (!res || !res.ok) {
      let errText = "Failed to create category in Zoltraak.";
      try {
        const data = await res.json();
        if (data?.error) errText = data.error;
      } catch {}
      throw new Error(errText);
    }
    const created = await res.json().catch(() => ({}));
    return {
      ok: true,
      category: {
        id: created.id,
        name,
        kind,
        pattern,
        directory,
        priority: 0,
        enabled: true,
      },
    };
  }
  if (message.type === "zoltraak-intercept-choose-folder") {
    let start = message.start ? String(message.start).trim() : "";
    if ((start.startsWith('"') && start.endsWith('"')) || (start.startsWith("'") && start.endsWith("'"))) {
      start = start.slice(1, -1).trim();
    }
    const res = await apiFetch("/folder/choose", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ start: start || undefined }),
    });
    if (!res || !res.ok) {
      let errText = "Folder picker is not available or closed.";
      try {
        const data = await res.json();
        if (data?.error) errText = data.error;
      } catch {}
      return { ok: false, error: errText };
    }
    const data = await res.json().catch(() => ({}));
    return { ok: true, path: data?.path || null };
  }
  if (message.type === "zoltraak-intercept-list-folders") {
    let path = message.path ? String(message.path).trim() : "";
    if ((path.startsWith('"') && path.endsWith('"')) || (path.startsWith("'") && path.endsWith("'"))) {
      path = path.slice(1, -1).trim();
    }
    const res = await apiFetch("/folder/list", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ path: path || undefined }),
    });
    if (!res || !res.ok) {
      return { ok: false, error: "Could not read folders from disk." };
    }
    const data = await res.json().catch(() => ({}));
    return { ok: true, ...data };
  }
  const key = draftKey(message.id);
  const { [key]: draft } = await chrome.storage.local.get(key);
  if (!draft || (!ownPage && draft.tabId !== tabId)) throw new Error("Download is no longer available.");
  if (submittingDrafts.has(key)) throw new Error("This download is already being submitted.");
  if (message.type === "zoltraak-intercept-cancel") {
    await chrome.storage.local.remove(key);
    return { ok: true };
  }
  if (message.type !== "zoltraak-intercept-submit") throw new Error("Unknown download action.");
  const filename = String(message.filename || "").trim();
  if (!filename || /[<>:"/\\|?*\x00-\x1f]/.test(filename) || /[. ]$/.test(filename)) {
    throw new Error("Enter a valid filename without path separators.");
  }
  let directory = String(message.directory || "").trim();
  if ((directory.startsWith('"') && directory.endsWith('"')) || (directory.startsWith("'") && directory.endsWith("'"))) {
    directory = directory.slice(1, -1).trim();
  }
  if (directory && !/^(?:[a-z]:[\\/]|\\\\[^\\]+\\[^\\]+|\/)/i.test(directory)) {
    throw new Error("Enter an absolute folder path, or leave it blank for automatic routing.");
  }
  const remember = message.remember === true;
  submittingDrafts.add(key);
  try {
    // A draft may have waited while the user logged in, logged out, or revoked
    // cookie access. Re-read rather than replaying the stored session cookie.
    const fresh = await videoRequestHeaders(draft.url, draft.cookieStoreId);
    const headers = (draft.headers || []).filter(([name]) => name.toLowerCase() !== "cookie"
      && !fresh.some(([current]) => current.toLowerCase() === name.toLowerCase()));
    headers.push(...fresh);
    const result = await enqueue({
      url: draft.url, referrer: draft.referrer, headers,
      filename, directory: directory || undefined, paused: message.later === true,
    });
    if (result.ok) {
      await chrome.storage.local.remove(key);
      if (result.id) await addSessionEntry(result.id, "download").catch(() => {});
      if (directory && remember) {
        await chrome.storage.local.set({ rememberedDirectory: directory });
        try {
          const currentSettingsRes = await apiFetch("/settings");
          if (currentSettingsRes?.ok) {
            const currentSettings = await currentSettingsRes.json();
            if (currentSettings && typeof currentSettings === "object" && !Array.isArray(currentSettings)) {
              await apiFetch("/settings", {
                method: "PUT",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ ...currentSettings, defaultDirectory: directory }),
              });
            }
          }
        } catch { /* ignore setting update errors */ }
      }
    }
    return result;
  } finally { submittingDrafts.delete(key); }
}

function showToastNotification(title, message) {
  if (chrome.notifications) {
    chrome.notifications.create({
      type: "basic",
      iconUrl: chrome.runtime.getURL("icons/128.png"),
      title: title || "Zoltraak Download",
      message: message || "Download enqueued in Zoltraak",
    });
  }
}

// An HLS/DASH manifest is not a file to save — it's the recipe for one. It has
// to go through the video pipeline, which fetches the segments, muxes video and
// audio into a single .mp4, and deletes the intermediates. Sending it to the
// plain file downloader instead saves the raw playlist and (with capture-all
// on) litters the folder with one .ts per segment.
const MANIFEST_URL_RE = /\.(m3u8|mpd)(\?|$)/i;

/**
 * Hand one URL to the engine, as a video job or a plain download.
 *
 * @returns {Promise<{ok: boolean, error?: string}>} whether the engine took
 * it. A caller holding a live browser download decides from this whether that
 * download is now redundant — an unreported failure here used to mean the
 * download was cancelled on the browser's side and never started on ours.
 */
async function capture(url, referrer, filename) {
  if (MANIFEST_URL_RE.test(url)) {
    return await captureVideo(url, referrer);
  }

  const headers = [];
  const cookie = await cookieHeader(url).catch(() => null);
  if (cookie) headers.push(["Cookie", cookie]);

  const result = await enqueue({ url, referrer, filename, headers });
  if (result.ok && result.id) await addSessionEntry(result.id, "download");

  await chrome.storage.local.set({
    lastCapture: { url, ok: result.ok, error: result.ok ? null : result.error, at: Date.now() },
  });

  const label = filename || url.split('/').pop() || url;
  showToastNotification(result.ok ? "⚡ Zoltraak Download Queued" : "❌ Zoltraak Download Failed", label);

  chrome.action.setBadgeText({ text: result.ok ? "✓" : "!" });
  chrome.action.setBadgeBackgroundColor({ color: result.ok ? "#35c46f" : "#f1595e" });
  setTimeout(() => void refreshBadge(), 2500);

  return result;
}

/* --- Session tracking: what this browser session has sent to Zoltraak ---
 * MV3's service worker is unloaded between events, so this lives in
 * chrome.storage (not module state) and is joined against the engine's live
 * snapshot on read — the stored list only remembers *which* ids are "ours"
 * and in what order, never their status. */
const SESSION_KEY = "sessionEntries";
const SESSION_START_KEY = "sessionStartTime";
const DISMISSED_KEY = "dismissedSessionEntries";
const SESSION_LIMIT = 40;

async function initSessionOnLaunch() {
  const now = Date.now();
  await chrome.storage.local.set({
    [SESSION_KEY]: [],
    [SESSION_START_KEY]: now,
    [DISMISSED_KEY]: [],
  });
}

async function getSessionStartTime() {
  const { [SESSION_START_KEY]: startTime } = await chrome.storage.local.get(SESSION_START_KEY);
  if (startTime) return startTime;
  const now = Date.now();
  await chrome.storage.local.set({ [SESSION_START_KEY]: now });
  return now;
}

async function addSessionEntry(id, kind) {
  const { [SESSION_KEY]: entries = [] } = await chrome.storage.local.get(SESSION_KEY);
  const next = [{ id, kind, addedAt: Date.now() }, ...entries.filter((e) => e.id !== id)].slice(0, SESSION_LIMIT);
  await chrome.storage.local.set({ [SESSION_KEY]: next });
}

async function removeSessionEntry(id) {
  const { [SESSION_KEY]: entries = [], [DISMISSED_KEY]: dismissed = [] } =
    await chrome.storage.local.get([SESSION_KEY, DISMISSED_KEY]);

  await chrome.storage.local.set({
    [SESSION_KEY]: entries.filter((e) => e.id !== id),
    [DISMISSED_KEY]: Array.from(new Set([...dismissed, id])),
  });
}

async function clearFinishedSessionEntries(ids) {
  if (!ids.length) return;
  const { [SESSION_KEY]: entries = [], [DISMISSED_KEY]: dismissed = [] } =
    await chrome.storage.local.get([SESSION_KEY, DISMISSED_KEY]);

  const drop = new Set(ids);
  const nextEntries = entries.filter((e) => !drop.has(e.id));
  const nextDismissed = Array.from(new Set([...dismissed, ...ids]));

  await chrome.storage.local.set({
    [SESSION_KEY]: nextEntries,
    [DISMISSED_KEY]: nextDismissed,
  });
}

/**
 * This session's tracked ids, joined against the engine's live state. An id
 * the engine no longer knows about (removed elsewhere, e.g. the desktop app)
 * is dropped from storage rather than shown as a ghost row.
 */
async function getSessionSnapshot() {
  const up = await health();
  if (!up) {
    return { ok: true, items: [], paired: false };
  }

  const sessionStartTime = await getSessionStartTime();
  const {
    [SESSION_KEY]: entries = [],
    [DISMISSED_KEY]: dismissed = [],
  } = await chrome.storage.local.get([SESSION_KEY, DISMISSED_KEY]);

  const snapshot = await getSnapshot();
  const paired = snapshot.ok;
  const dismissedSet = new Set(dismissed);

  const byId = new Map();
  for (const it of snapshot.items || []) byId.set(it.id, { ...it, kind: "download" });
  for (const j of snapshot.videoJobs || []) byId.set(j.id, { ...j, kind: "video" });

  const items = [];
  const stale = [];
  const seenIds = new Set();

  for (const entry of entries) {
    if (dismissedSet.has(entry.id)) continue;
    const live = byId.get(entry.id);
    if (live) {
      items.push({ ...live, addedAt: entry.addedAt });
      seenIds.add(entry.id);
    } else if (paired) {
      // Engine is reachable and does not know this id anymore — it was
      // removed elsewhere. Drop it instead of showing a dead row forever.
      stale.push(entry.id);
    } else {
      // Engine unreachable right now; keep the entry, show it as unknown.
      items.push({ id: entry.id, kind: entry.kind, state: "unknown", addedAt: entry.addedAt });
      seenIds.add(entry.id);
    }
  }

  // Include downloads/video jobs from the engine that were created since the browser was opened in this session
  if (paired) {
    for (const [id, live] of byId.entries()) {
      if (seenIds.has(id) || dismissedSet.has(id)) continue;

      let createdAtMs = 0;
      const dateStr = live.createdAt || live.created_at;
      if (dateStr) {
        createdAtMs = new Date(dateStr).getTime();
      }

      if (createdAtMs >= sessionStartTime || live.state === "downloading" || live.state === "merging" || live.state === "waiting") {
        items.push({ ...live, addedAt: dateStr || new Date(sessionStartTime).toISOString() });
        seenIds.add(id);
      }
    }
  }

  if (stale.length) await clearFinishedSessionEntries(stale);

  return { ok: true, items, paired };
}

let reloadWatcherActive = false;
let extensionStatusRequest = null;
let lastReloadTriggeredAt = 0;

async function checkReloadOnce() {
  if (extensionStatusRequest) return extensionStatusRequest;
  extensionStatusRequest = (async () => {
    try {
      const cursor = await extensionStatusCursor();
      const res = await apiFetch(
        `/extension/status?reloadVersion=${cursor.reloadVersion}&sourceRevision=${cursor.sourceRevision}`,
        {
          headers: { "Cache-Control": "no-cache" },
          signal: AbortSignal.timeout(28000),
        },
      );
      if (res?.ok) {
        const data = await res.json();
        if (data && typeof data.reloadVersion === "number") {
          const sourceBecameActive = await setCookieSourceState(
            data.activeCookieSource,
            data.sourceRevision,
          );
          if (sourceBecameActive && data.activeCookieSource) {
            void refreshAllSessions();
          }
          // A service worker loses every global on runtime.reload(). Acknowledge
          // the server revision first so the replacement worker cannot receive
          // the same command forever.
          await setExtensionStatusCursor(data.reloadVersion, data.sourceRevision);
          const isGecko = typeof navigator !== "undefined" && (/Firefox\//i.test(navigator.userAgent) || /Gecko\//i.test(navigator.userAgent));
          if (data.reload && !isGecko) {
            const now = Date.now();
            if (now - lastReloadTriggeredAt > 5000) {
              lastReloadTriggeredAt = now;
              console.log(`[Zoltraak] Reload signal received from desktop app (version ${data.reloadVersion}). Reloading extension...`);
              if (chrome.runtime?.reload) {
                await queueInjectedTabsForRefresh();
                chrome.runtime.reload();
                return true;
              }
            }
          }
        }
      }
    } catch {
      // Engine offline or request timed out — normal behavior.
    }
    return false;
  })();
  try {
    return await extensionStatusRequest;
  } finally {
    extensionStatusRequest = null;
  }
}

async function startReloadWatcher() {
  if (reloadWatcherActive) return;
  reloadWatcherActive = true;

  while (reloadWatcherActive) {
    try {
      const reloaded = await checkReloadOnce();
      if (reloaded) break;
      await new Promise((r) => setTimeout(r, 1000));
    } catch {
      await new Promise((r) => setTimeout(r, 5000));
    }
  }
}

async function checkStatus() {
  const up = await health();
  if (up) {
    await checkReloadOnce();
    // If the engine is up, ensure we are properly paired.
    // If the token was changed/mismatched, ensurePaired will self-heal it!
    await ensurePaired();
    // After pairing, not before: an unpaired push is refused, and this is the
    // path that carries a token stranded by the app having been closed.
    await deliverStrandedYtToken();
  }
  await refreshBadge();
}

let themeRequest = null;
let themeFetchedAt = 0;
async function syncExtensionTheme() {
  if (themeRequest) return themeRequest;
  if (Date.now() - themeFetchedAt < 1500) return { ok: true };
  themeRequest = (async () => {
    const response = await apiFetch("/extension/theme", { signal: AbortSignal.timeout(2500) });
    if (!response?.ok) return { ok: false };
    const theme = await response.json();
    const { extensionTheme } = await chrome.storage.local.get("extensionTheme");
    if (JSON.stringify(theme) !== JSON.stringify(extensionTheme)) {
      await chrome.storage.local.set({ extensionTheme: theme });
    }
    return { ok: true };
  })();
  try { return await themeRequest; }
  finally { themeFetchedAt = Date.now(); themeRequest = null; }
}

async function refreshBadge() {
  const up = await health();
  if (!up) {
    chrome.action.setBadgeText({ text: "!" });
    chrome.action.setBadgeBackgroundColor({ color: "#f1595e" });
    chrome.action.setTitle({
      title: "Zoltraak — app not running",
    });
    return;
  }
  const token = await getToken();
  const paired = token ? await verify(token) : false;
  if (!paired) {
    chrome.action.setBadgeText({ text: "!" });
    chrome.action.setBadgeBackgroundColor({ color: "#f59e0b" });
    chrome.action.setTitle({
      title: "Zoltraak — token mismatch (click to pair)",
    });
    return;
  }
  chrome.action.setBadgeText({ text: "" });
  chrome.action.setTitle({
    title: "Zoltraak — connected",
  });
}

void startReloadWatcher();
void refreshQueuedTabs();
