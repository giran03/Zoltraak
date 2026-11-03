// Detects on-page playing <video> elements and anchors an IDM-style floating
// "▶ Download this video" button to the top-right corner of the active video player.

const MIN_WIDTH = 160;
const MIN_HEIGHT = 90;
const OVERLAY_HOST_IDS = ["zoltraak-extension-host", "zoltraak-session-host"];
const EXTENSION_OWNER = chrome.runtime?.id || "zoltraak";

// An older unpacked OpenDM/Zoltraak copy can still inject into the same page.
// Clear anything that predates this content-script generation, then keep hosts
// from other extension IDs out so only the current build can own the overlay.
for (const id of OVERLAY_HOST_IDS) document.getElementById(id)?.remove();
const overlayOwnerObserver = new MutationObserver(() => {
  for (const id of OVERLAY_HOST_IDS) {
    document.querySelectorAll(`[id="${id}"]`).forEach((node) => {
      if (node.dataset.zoltraakOwner !== EXTENSION_OWNER) node.remove();
    });
  }
});
overlayOwnerObserver.observe(document.documentElement, { childList: true, subtree: true });

/**
 * Dispatch Board, injected.
 *
 * Shared by both shadow roots so the floating button, the pill, the session
 * panel and the format picker cannot drift apart. Values come from DESIGN.md.
 *
 * These dark defaults apply when desktop theme sync is disabled. theme.js
 * overrides tokens only on our shadow hosts when syncing is enabled.
 */
const OVERLAY_TOKENS = `
  @font-face {
    font-family: "Archivo Variable";
    font-style: normal;
    font-weight: 100 900;
    font-stretch: 62% 125%;
    font-display: block;
    src: url("${chrome.runtime.getURL("fonts/archivo-subset.woff2")}") format("woff2-variations");
  }
  @font-face {
    font-family: "Martian Mono Variable";
    font-style: normal;
    font-weight: 100 800;
    font-stretch: 75% 112.5%;
    font-display: block;
    src: url("${chrome.runtime.getURL("fonts/martian-mono-subset.woff2")}") format("woff2-variations");
  }

  :host {
    all: initial;

    --panel: #161a17;
    --face: #1e2320;
    --recess: #101312;
    --edge: #333b35;
    --edge-strong: #4c564e;

    --ink: #e9ede7;
    --ink-dim: #a6aea4;
    --ink-faint: #868e84;

    --clear: #35c46f;
    --caution: #e8a317;
    --danger: #f1595e;
    --route: #56a2f0;
    --brass: #b09a63;
    --lamp-off: #2b332d;
    --on-signal: #0b0e0d;

    /* Deeper than the desktop's plate shadow: this one has to separate from an
       unknown ground, including a bright page. */
    --shadow-plate: 0 1px 2px rgb(0 0 0 / 0.4), 0 6px 20px -4px rgb(0 0 0 / 0.55);
    --shadow-lift: 0 2px 4px rgb(0 0 0 / 0.45), 0 18px 40px -10px rgb(0 0 0 / 0.7);
  }

  *, *::before, *::after {
    box-sizing: border-box;
    font-family: "Archivo Variable", ui-sans-serif, system-ui, sans-serif;
  }

  /* Engraved legend: names a control, cut adjacent to it. Never an eyebrow. */
  .legend {
    font-stretch: 78%;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.11em;
    font-size: 11px;
    font-feature-settings: "tnum" 1;
  }

  /* Describer: measured values, URLs, identifiers. Never a sentence. */
  .describer {
    font-family: "Martian Mono Variable", ui-monospace, monospace;
    font-stretch: 75%;
    font-size: 11px;
    letter-spacing: -0.01em;
    font-variant-numeric: tabular-nums;
  }

  /* A lamp switches; it never fades. An unlit housing is still a housing. */
  .lamp {
    width: 8px;
    height: 8px;
    border-radius: var(--hw-r-sm, 1px);
    flex-shrink: 0;
    display: inline-block;
    background: var(--lamp-off);
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.25);
  }
  .lamp.clear { background: var(--clear); }
  .lamp.caution { background: var(--caution); }
  .lamp.danger { background: var(--danger); }
  .lamp.route { background: var(--route); }

  /* Occupancy: rectangular cells, because a cell is a byte range and a range
     has edges. Never a rounded bar, never a ring. */
  .occupancy {
    display: flex;
    gap: 2px;
    height: 4px;
    width: 100%;
  }
  .occupancy span {
    flex: 1;
    min-width: 0;
    border-radius: var(--hw-r-sm, 1px);
    background: var(--lamp-off);
  }
  .occupancy span.lit { background: var(--clear); }
  .occupancy span.held { background: var(--danger); }
  .occupancy span.merge { background: var(--route); }

  button { font: inherit; cursor: pointer; }

  :focus-visible {
    outline: 2px solid var(--route);
    outline-offset: -2px;
    border-radius: var(--hw-r, 2px);
  }

  @media (prefers-reduced-motion: reduce) {
    * { animation-duration: 0.01ms !important; transition-duration: 0.01ms !important; }
  }
`;

/**
 * Whether this frame may render the floating UI at all. Rendered at most once
 * per tab: only in the top frame (never inside a sub-frame/iframe, which is
 * what kept several overlays from stacking on pages full of embeds) and only
 * while this tab is the active, visible one — a background tab shows nothing.
 */
function overlaysAllowed() {
  return window.self === window.top && document.visibilityState === "visible";
}

/**
 * The two overlays the user can switch off independently from the popup: the
 * download button that rides on a video, and the Dispatch Board pill.
 *
 * They are separate switches because they answer to different annoyances — one
 * sits on top of the page's own player controls, the other is a permanent
 * fixture in a corner — and someone who wants rid of one usually still wants
 * the other. Both default on; `undefined` (nothing stored yet) reads as on.
 *
 * Held as a plain object rather than read per-call: this is consulted on every
 * refresh tick and every poll, and `chrome.storage` is asynchronous.
 */
const PillPlacement = globalThis.ZoltraakPillPlacement;

const overlayPrefs = {
  showMediaButton: true,
  showSessionPill: true,
  position: { preset: "top-right", offsetX: 0, offsetY: 0, scale: 100 },
  // Where and how the Dispatch Board pill is drawn; see pill-placement.js.
  pill: PillPlacement.sanitize(undefined),
  pillSites: {},
};
// The pill is not drawn until its saved placement has been read, so it never
// flashes in the default corner before jumping to the user's chosen spot.
let pillPrefsLoaded = false;

function loadPillPrefs(stored) {
  overlayPrefs.pill = PillPlacement.sanitize(stored.pillPlacement, stored.pillCorner);
  overlayPrefs.pillSites = PillPlacement.sanitizeSites(stored.pillSites);
  pillPrefsLoaded = true;
}

function sanitizeOverlayScale(value) {
  const scale = Number(value);
  return Number.isFinite(scale) ? Math.max(50, Math.min(200, scale)) : 100;
}

if (chrome.runtime?.id) {
  // The pill's poll loop belongs to the top frame only — the same rule the
  // bottom-of-file bootstrap follows. Waking it from here in every iframe
  // would put a timer in each one just to tear a host down that was never
  // built.
  const repollIfTopFrame = () => {
    if (window.self === window.top) void pollSession();
  };

  chrome.storage.local.get(
    {
      showMediaButton: true,
      showSessionPill: true,
      overlayPosition: { preset: "top-right", offsetX: 0, offsetY: 0, scale: 100 },
      pillPlacement: null,
      pillSites: {},
      pillCorner: null,
    },
    (res) => {
      overlayPrefs.showMediaButton = res.showMediaButton !== false;
      overlayPrefs.showSessionPill = res.showSessionPill !== false;
      loadPillPrefs(res);
      if (res.overlayPosition) {
        overlayPrefs.position = {
          preset: res.overlayPosition.preset || "top-right",
          offsetX: Number(res.overlayPosition.offsetX) || 0,
          offsetY: Number(res.overlayPosition.offsetY) || 0,
          scale: sanitizeOverlayScale(res.overlayPosition.scale),
        };
      }
      refresh();
      repollIfTopFrame();
      if (currentVideo) updateHostPosition();
    },
  );

  // Flipping a switch in the popup applies to every open tab at once, rather
  // than only to pages loaded afterwards.
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area !== "local") return;
    if (changes.showMediaButton) {
      overlayPrefs.showMediaButton = changes.showMediaButton.newValue !== false;
      refresh();
    }
    if (changes.showSessionPill) {
      overlayPrefs.showSessionPill = changes.showSessionPill.newValue !== false;
      repollIfTopFrame();
    }
    if (changes.pillPlacement || changes.pillSites || changes.pillCorner) {
      chrome.storage.local.get({ pillPlacement: null, pillSites: {}, pillCorner: null }, (stored) => {
        loadPillPrefs(stored);
        lastRenderSignature = "";
        repollIfTopFrame();
      });
    }
    if (changes.overlayPosition) {
      const val = changes.overlayPosition.newValue || {};
      overlayPrefs.position = {
        preset: val.preset || "top-right",
        offsetX: Number(val.offsetX) || 0,
        offsetY: Number(val.offsetY) || 0,
        scale: sanitizeOverlayScale(val.scale),
      };
      if (currentVideo) updateHostPosition();
    }
  });
}

/** X.com/Twitter host check used for three-dot dropdown menu injection and permalink formatting. */
function isXHost() {
  return /(^|\.)(x\.com|twitter\.com)$/i.test(location.hostname);
}

/**
 * Hosts the engine has a purpose-built extractor for.
 *
 * On these, the only correct thing to send is the *item's own address*. The
 * extractor reads the site's manifest and pairs picture with sound; a URL
 * sniffed off the wire cannot be — Facebook and Instagram fetch a video's
 * picture and its audio as two separate, identically-named `.mp4` files, and
 * X's timeline has a dozen clips in flight at once. Handing over one of those
 * captures is how "Download this video" produced a silent clip, an audio-only
 * file, or somebody else's tweet.
 */
function hasSiteExtractor() {
  return /(^|\.)(facebook\.com|fb\.watch|instagram\.com|x\.com|twitter\.com|tiktok\.com|reddit\.com|redd\.it|vimeo\.com|youtube\.com|youtu\.be)$/i.test(
    location.hostname,
  );
}

/** Whether a URL already names one specific post/video rather than a feed,
 * profile, or search page — on such a page the video on screen *is* the page. */
function isItemPermalink(url) {
  if (!url || typeof url !== "string") return false;
  try {
    if (/(^|\.)instagram\.com$/i.test(new URL(url).hostname)) {
      return Boolean(globalThis.ZoltraakVideoIdentity.itemIdentity(url));
    }
  } catch { return false; }
  const cleanPath = url.split("?")[0].split("#")[0];
  const isPathMatch = /\/(status\/\d+|reels?\/[\w.-]+|p\/[\w.-]+|tv\/[\w.-]+|videos?\/[\w.-]*\d|watch\/[\w.-]+|share\/[prv]\/[\w.-]+|shorts\/[\w.-]+|video\/\d+|comments\/[\w.-]+|posts\/[\w.-]+|permalink\/\d+)/i.test(cleanPath);
  if (isPathMatch) return true;
  return /[?&](?:v|fbid|story_fbid)=[\w.-]+/i.test(url);
}

/** YouTube host check — used to route the overlay to the canonical watch URL. */
function isYouTubeHost() {
  return /(^|\.)(youtube\.com|youtu\.be)$/i.test(location.hostname);
}

/**
 * Derive the canonical `https://www.youtube.com/watch?v=<id>` URL from
 * whatever YouTube page shape is currently loaded (watch, shorts, live, embed).
 * Falls back to `location.href` unchanged if no video ID can be found.
 */
function youTubeWatchUrl() {
  const href = location.href;
  const m = href.match(
    /(?:\/shorts\/|\/watch\?.*?v=|\/v\/|\/embed\/|\/live\/|youtu\.be\/)([a-zA-Z0-9_-]{11})/
  ) || href.match(/[?&]v=([a-zA-Z0-9_-]{11})/);
  if (m?.[1]) return `https://www.youtube.com/watch?v=${m[1]}`;
  return href;
}

let host = null;
let shadow = null;
let currentVideo = null;
let dismissedVideo = null;
// When `currentVideo` last changed — lets gatherSources() ask the background
// worker for only the network captures made since *this* video became
// current, instead of every stream the tab has ever fetched. Without this, a
// feed/timeline page where several videos play one after another (or an
// autoplaying ad) can hand back a previous clip's manifest as if it were the
// one behind the overlay.
let currentVideoSince = 0;
const playerTracker = globalThis.ZoltraakVideoIdentity.createPlayerTracker();
function videoPermalink(video) {
  if (!video) return null;
  if (video.closest(".ad-showing")) return null;
  const nearby = nearestPermalink(video);
  if (nearby) return nearby;
  if (isYouTubeHost() && video.closest("#movie_player")) return youTubeWatchUrl();
  // Instagram preloads neighboring reels in the DOM, even on an item page.
  // Only the prominent visible player may inherit that page's post identity.
  if (/(^|\.)(instagram\.com|tiktok\.com)$/i.test(location.hostname) && isItemPermalink(location.href)) {
    const visible = Array.from(document.querySelectorAll("video")).filter(isEligible);
    if (mostProminent(visible) === video) return location.href;
  }
  if (document.querySelectorAll("video").length === 1 && isItemPermalink(location.href)) return location.href;
  return null;
}
function socialPostPermalink(video) {
  const url = globalThis.ZoltraakVideoIdentity.socialPostUrl(videoPermalink(video));
  if (!url) return null;
  const site = (host) => host.replace(/^(www|m|mobile)\./, "").replace(/^twitter\.com$/, "x.com")
    .replace(/^(old|new)\.reddit\.com$/, "reddit.com");
  return site(new URL(url).hostname) === site(location.hostname) ? url : null;
}
function playerFingerprint(video) {
  const permalink = videoPermalink(video);
  const itemId = globalThis.ZoltraakVideoIdentity.itemIdentity(permalink);
  return JSON.stringify([itemId || permalink, video.currentSrc || video.src || "",
    manifestFromAttributes(video), video.poster || "",
    itemId ? null : Number.isFinite(video.duration) ? video.duration : null,
    itemId ? null : globalThis.ZoltraakGenericMedia?.recordFor(video)?.generation]);
}
function playerBinding(video, reset = false) {
  if (!video) return null;
  const itemId = globalThis.ZoltraakVideoIdentity.itemIdentity(videoPermalink(video));
  return { ...playerTracker.observe(video, playerFingerprint(video), reset, itemId),
    siteItemId: itemId,
    sourceUrl: video.currentSrc || video.src || null,
    elementManifest: manifestFromAttributes(video) };
}
function matchesPlayer(video, binding) {
  return Boolean(video?.isConnected && playerTracker.matches(video, playerFingerprint(video), binding,
    globalThis.ZoltraakVideoIdentity.itemIdentity(videoPermalink(video))));
}
let currentPlayerGeneration = null;


function isEligible(video) {
  if (video === dismissedVideo) return false;
  const rect = video.getBoundingClientRect();
  const visible =
    rect.width >= MIN_WIDTH &&
    rect.height >= MIN_HEIGHT &&
    rect.bottom > 0 &&
    rect.right > 0 &&
    rect.top < window.innerHeight &&
    rect.left < window.innerWidth;

  return visible;
}

/** On-screen area clipped to the viewport — used to pick the most prominent
 * candidate among several videos that are simultaneously playing (e.g. the
 * article's real video next to an autoplaying ad or a sidebar recommendation). */
function visibleArea(video) {
  const r = video.getBoundingClientRect();
  const w = Math.max(0, Math.min(r.right, window.innerWidth) - Math.max(r.left, 0));
  const h = Math.max(0, Math.min(r.bottom, window.innerHeight) - Math.max(r.top, 0));
  return w * h;
}

function mostProminent(videos) {
  return [...videos].sort((a, b) => visibleArea(b) - visibleArea(a))[0];
}

function pickVideo() {
  // Playback ending or a neighboring player starting must not close the picker.
  // Rebind only when this player leaves view or its content identity changes.
  if (currentVideo?.isConnected && isEligible(currentVideo)) return currentVideo;
  const videos = Array.from(document.querySelectorAll("video")).filter(isEligible);
  if (videos.length === 0) return null;

  // 1. Currently playing wins; if several are playing at once, the one most
  //    visible on screen (not just the first in DOM order) is almost always
  //    the one the user means.
  const playing = videos.filter((v) => !v.paused && !v.ended && v.readyState >= 2);
  if (playing.length > 0) return mostProminent(playing);

  // 2. Fallback to a video with active playback time > 0.
  const active = videos.filter((v) => v.currentTime > 0 && !v.ended);
  if (active.length > 0) return mostProminent(active);

  // 3. Fallback to the largest visible video.
  return mostProminent(videos);
}

/** A directly fetchable URL for this exact element — never blob:/mediasource:,
 * which only resolve inside this page's memory and are useless to a
 * server-side fetch. */
function directSrc(video) {
  if (!video) return null;
  const candidates = [
    video.currentSrc,
    video.src,
    video.getAttribute("src"),
  ];
  const sources = video.querySelectorAll("source");
  for (const s of sources) {
    candidates.push(s.src || s.getAttribute("src"));
  }
  for (const src of candidates) {
    const resolved = mediaUrl(src);
    if (resolved) return resolved;
  }
  return null;
}

const MANIFEST_ATTR_PATTERN = /(hls|dash|manifest|stream|video)[-_]?(src|url)$/i;
const MANIFEST_VALUE_PATTERN = /\.(m3u8|mpd|mp4|webm)(\?|$)/i;

function mediaUrl(value) {
  if (!value) return null;
  try {
    const url = new URL(value, document.baseURI);
    return /^https?:$/.test(url.protocol) ? globalThis.ZoltraakVideoIdentity.completeMediaUrl(url.href) : null;
  } catch { return null; }
}

/** Only consulted after normal extraction fails; never scan unrelated players. */
function hlsFallbackSources(video) {
  const urls = new Set();
  const add = (value) => {
    const url = mediaUrl(value);
    if (url && /\.m3u8(?:[?#]|$)/i.test(url)) urls.add(url);
  };
  add(directSrc(video));
  let el = video;
  for (let depth = 0; el && depth < 5; depth++, el = el.parentElement) {
    if (el !== video && el.querySelectorAll("video").length > 1) break;
    for (const value of Object.values(el.dataset || {})) add(value);
  }
  for (const entry of performance.getEntriesByType("resource")) add(entry.name);
  return [...urls];
}

/** Custom players frequently keep the real stream/manifest URL in a data-*
 * attribute even while the <video> itself plays from a MediaSource `blob:`
 * URL — the attribute is sometimes on a wrapping player div rather than the
 * <video> element, so this walks a few ancestors looking for it. */
function manifestFromAttributes(video) {
  let el = video;
  for (let depth = 0; el && depth < 5; depth += 1, el = el.parentElement) {
    if (el !== video && el.querySelectorAll("video").length > 1) break;
    for (const [key, value] of Object.entries(el.dataset || {})) {
      const resolved = mediaUrl(value);
      if (!resolved) continue;
      if (MANIFEST_ATTR_PATTERN.test(key) || MANIFEST_VALUE_PATTERN.test(value)) {
        return resolved;
      }
    }
  }
  return null;
}

/** The specific item's permalink (a single tweet, post, reel, or card) rather than
 * the surrounding feed/page URL — on a page with several videos, this is
 * what lets the engine analyze just the one the user meant instead of
 * scanning every media URL on the whole page. */
function nearestPermalink(video) {
  if (!video) return null;
  if (/(^|\.)tiktok\.com$/i.test(location.hostname)) return tiktokPermalinkFromNode(video);
  if (/(^|\.)facebook\.com$/i.test(location.hostname)) {
    return facebookPermalinkFromNode(video, video.tagName === "VIDEO");
  }

  // 1. Direct enclosing link
  const directAnchor = video.closest("a[href]");
  if (directAnchor?.href && /^https?:\/\//i.test(directAnchor.href) && isItemPermalink(directAnchor.href)) {
    return directAnchor.href;
  }

  // 2. Data attributes on the video or its immediate player wrappers
  let attrEl = video;
  for (let d = 0; attrEl && d < 6; d++, attrEl = attrEl.parentElement) {
    if (attrEl.querySelectorAll("video").length > 1) break;
    const videoId = attrEl.getAttribute("data-video-id") || attrEl.getAttribute("data-id");
    if (videoId && /^\d{10,25}$/.test(videoId)) {
      if (/(^|\.)facebook\.com$/i.test(location.hostname)) {
        return `https://www.facebook.com/reel/${videoId}`;
      }
      if (/(^|\.)tiktok\.com$/i.test(location.hostname)) {
        return `https://www.tiktok.com/video/${videoId}`;
      }
    }
  }

  const selectorList = [
    "a[href*='/status/']",
    "a[href*='/videos/']",
    "a[href*='/video/']",
    "a[href*='/watch/']",
    "a[href*='/watch?']",
    "a[href*='/reel/']",
    "a[href*='/reels/']",
    "a[href*='/share/v/']",
    "a[href*='/share/r/']",
    "a[href*='/share/p/']",
    "a[href*='/p/']",
    "a[href*='/tv/']",
    "a[href*='/v/']",
    "a[href*='/post/']",
    "a[href*='/posts/']",
    "a[href*='/shorts/']",
    "a[href*='/comments/']",
    "a[href*='story_fbid=']",
    "a[href*='fbid=']",
  ].join(", ");

  // 3. Isolated ancestor walk: stop at any container that contains multiple <video> tags
  // to guarantee we never leak into the outer feed and pick up another post's link!
  let el = video;
  for (let depth = 0; el && depth < 35; depth += 1, el = el.parentElement) {
    if (el.tagName === "BODY" || el.tagName === "HTML") break;
    if (el.querySelectorAll("article, [role='article']").length > 1) break;

    const otherVideos = el.querySelectorAll("video");
    const isMultiVideo = otherVideos.length > 1;

    if (isMultiVideo) {
      // A carousel can preload several players inside one post article.
      // Recover the post only when every supported link agrees on its identity.
      if (el.matches?.("article, [role='article']")) {
        const posts = Array.from(el.querySelectorAll(selectorList))
          .map((a) => globalThis.ZoltraakVideoIdentity.socialPostUrl(a.href)).filter(Boolean);
        const unique = new Set(posts);
        if (unique.size === 1) return posts[0];
      }
      break;
    }
    const candidates = Array.from(el.querySelectorAll(selectorList))
      .filter((a) => a.href && /^https?:\/\//i.test(a.href) && isItemPermalink(a.href));
    const identities = new Set(candidates.map((a) => globalThis.ZoltraakVideoIdentity.itemIdentity(a.href) || a.href));
    if (identities.size === 1) return candidates[0].href;
    if (identities.size > 1) return null;
    if (el.matches?.("article, [role='article']")) break;

  }

  return null;
}

/** TikTok controls are siblings of its blob-backed player. Search one card
 * at a time; neither a feed container nor a preloaded neighbor proves ownership. */
function tiktokPermalinkFromNode(node) {
  const normalize = (value) => {
    const url = globalThis.ZoltraakVideoIdentity.socialPostUrl(value);
    return url && /(^|\.)tiktok\.com$/i.test(new URL(url).hostname) ? url : null;
  };
  const direct = normalize(node.closest?.("a[href]")?.href);
  if (direct) return direct;
  const boundary = "article, [role='article'], [data-e2e='recommend-list-item-container'], [data-e2e='feed-item'], [data-e2e='browse-video']";
  for (let el = node, depth = 0; el && depth < 24; el = el.parentElement, depth++) {
    if (["BODY", "HTML"].includes(el.tagName)) break;
    if (el.querySelectorAll(boundary).length > 1) break;
    const videos = el.querySelectorAll("video");
    if (videos.length > 1) break;
    const posts = [...el.querySelectorAll("a[href], input, [data-e2e='video-share-link']")]
      .map((el) => normalize(el.href || el.value || el.textContent?.trim())).filter(Boolean);
    const identified = posts.filter((url) => globalThis.ZoltraakVideoIdentity.itemIdentity(url));
    const candidates = identified.length ? identified : posts;
    const identities = new Set(candidates.map((url) => globalThis.ZoltraakVideoIdentity.itemIdentity(url) || url));
    if (identities.size === 1) return candidates[0];
    if (identities.size > 1) return null;
    const id = ["data-video-id", "data-item-id", "data-aweme-id", "data-id"]
      .map((key) => el.getAttribute(key)).find((id) => /^\d{10,25}$/.test(id || ""))
      || el.id?.match(/^(?:video|item)[-_](\d{10,25})$/)?.[1];
    if (id && (videos.length === 1 || el.tagName === "VIDEO")) {
      const users = new Set([...el.querySelectorAll("a[href]")].map((a) => {
        try { const u = new URL(a.href); return /(^|\.)tiktok\.com$/.test(u.hostname) ? u.pathname.match(/^\/@([^/]+)\/?$/)?.[1] : null; }
        catch { return null; }
      }).filter(Boolean));
      return `https://www.tiktok.com/@${users.size === 1 ? [...users][0] : '_'}/video/${id}`;
    }
    if (el.matches(boundary)) break;
  }
  return null;
}

/** Facebook timestamps, media and controls occupy sibling wrappers. Search
 * the clicked wrapper itself, then its post; never scan into the outer feed. */
function facebookPermalinkFromNode(node, preferVideo = false) {
  const article = node.closest?.("article, [role='article']");
  const normalize = (value) => {
    const url = globalThis.ZoltraakVideoIdentity.socialPostUrl(value);
    return url && /(^|\.)facebook\.com$/.test(new URL(url).hostname) ? url : null;
  };
  const direct = normalize(node.closest?.("a[href]")?.href);
  let attachment = direct;
  // A photo link describes one attachment; the post action wants the post.
  if (direct && !/\/photo(?:\.php)?(?:[/?]|$)/.test(new URL(direct).pathname)) return direct;
  for (let el = node, depth = 0; el && depth < 35; el = el.parentElement, depth++) {
    if (["BODY", "HTML"].includes(el.tagName)) break;
    if (el.querySelectorAll("article, [role='article']").length > 1) break;
    const videoId = el.getAttribute("data-video-id");
    if (preferVideo && /^\d{3,25}$/.test(videoId || "") && el.querySelectorAll("video").length <= 1) {
      return `https://www.facebook.com/watch/?v=${videoId}`;
    }
    const links = [...el.querySelectorAll("a[href]")]
      .filter((a) => !a.closest?.("[role='comment']"))
      .filter((a) => !article || a.closest?.("article, [role='article']") === article)
      .map((a) => normalize(a.href)).filter(Boolean);
    const videoLinks = links.filter((url) => /\/(?:reel|videos|share\/[rv])\//.test(new URL(url).pathname)
      || new URL(url).searchParams.has("v"));
    const postLinks = links.filter((url) => /\/(?:posts|permalink|share\/p)\//.test(new URL(url).pathname)
      || new URL(url).searchParams.has("story_fbid"));
    const preferred = preferVideo && videoLinks.length ? videoLinks : postLinks;
    if (!preferred.length && !attachment) {
      const uniqueLinks = new Set(links);
      if (uniqueLinks.size === 1) attachment = links[0];
    }
    const canonical = preferred.filter((url) => !new URL(url).pathname.startsWith("/share/"));
    const candidates = canonical.length ? canonical : preferred;
    const unique = new Map(candidates.map((url) => [globalThis.ZoltraakVideoIdentity.itemIdentity(url) || url, url]));
    if (unique.size === 1) return unique.values().next().value;
    if (unique.size > 1) return null;
    // Older/mobile Facebook markup carries the Story ID on the post wrapper.
    try {
      const data = JSON.parse(el.getAttribute("data-ft") || "null");
      const id = data?.top_level_post_id;
      const owner = data?.content_owner_id_new;
      if (/^\d+$/.test(id || "") && /^\d+$/.test(owner || "")) {
        return `https://www.facebook.com/permalink.php?story_fbid=${id}&id=${owner}`;
      }
    } catch { /* Ignore unrelated/malformed page metadata. */ }
    if (el.matches?.("article, [role='article']")) break;
  }
  // Inside Facebook's gallery there may be no timestamp nearby. Its pcb.
  // collection identifies the parent post, rather than just this photo.
  try {
    const set = attachment && new URL(attachment).searchParams.get("set");
    if (/^pcb\.\d+$/.test(set || "")) return `https://www.facebook.com/permalink.php?story_fbid=${set.slice(4)}`;
  } catch { /* Keep the original attachment when collection data is absent. */ }
  return attachment;
}

/**
 * Every plausible analysis target for this exact video, best-first. The
 * element's own direct file and any manifest in its markup come first (they
 * are tied to *this* element), then the streams the tab actually fetched over
 * the network (the accurate path for MSE/`blob:` playback, where the element
 * exposes no real URL at all). The bare page URL is only ever a last resort,
 * since the engine's generic scraper returns every media URL on the page.
 */
async function gatherSources(video) {
  const seen = new Set();
  const sources = [];
  const add = (url) => {
    if (!url || seen.has(url) || /^blob:/i.test(url)) return;
    seen.add(url);
    sources.push(url);
  };

  add(directSrc(video));
  add(manifestFromAttributes(video));
  for (const source of globalThis.ZoltraakGenericMedia?.sourcesFor(video) || []) add(source.url);

  // The clicked item's own permalink identifies *this* video unambiguously.
  // The tab's captured network streams (added next) are tab-wide, so on a feed
  // or timeline with several videos in the DOM they can belong to a different
  // clip than the one whose overlay was opened — that mismatch is what made
  // "Download this video" grab the wrong video. Every supported platform
  // (X, Facebook, Instagram, YouTube, Reddit, TikTok, Vimeo) resolves a
  // per-item permalink to exactly that item, so it ranks above tab captures —
  // but still below a URL taken straight off the element, which is more direct.
  add(nearestPermalink(video));

  try {
    // A few seconds of slack before `currentVideoSince`: a player usually
    // fetches its manifest right as playback starts, sometimes a beat before
    // the "playing" event fires, so a hard cutoff at that exact instant would
    // drop the video's own manifest along with the stale ones.
    for (const url of await tabCaptures()) add(url);
  } catch {
    // Extension context gone; fall through to whatever we found from the DOM.
  }

  // The whole-page URL is the last resort: the engine's generic scraper returns
  // every media URL on the page, so it must never outrank a per-item source.
  add(location.href);

  return sources;
}

function ensureHost() {
  if (host) return;
  host = document.createElement("div");
  host.id = "zoltraak-extension-host";
  host.dataset.zoltraakOwner = EXTENSION_OWNER;
  host.style.cssText =
    "position:fixed;z-index:2147483647;pointer-events:none;transition:top 0.1s ease, right 0.1s ease, bottom 0.1s ease, left 0.1s ease;";
  shadow = host.attachShadow({ mode: "open" });
  globalThis.ZoltraakTheme?.register(host);
  (document.body || document.documentElement).appendChild(host);
}

function updateHostPosition() {
  if (!host || !currentVideo) return;
  const rect = currentVideo.getBoundingClientRect();

  const pos = overlayPrefs.position || { preset: "top-right", offsetX: 0, offsetY: 0, scale: 100 };
  const preset = pos.preset || "top-right";
  const offsetX = Number(pos.offsetX) || 0;
  const offsetY = Number(pos.offsetY) || 0;
  const scale = sanitizeOverlayScale(pos.scale) / 100;
  const isOutside = preset.startsWith("outside-");
  const isBottom = preset.includes("bottom");
  const anchorBottom = preset.startsWith("outside-top") || (!isOutside && isBottom);

  // Clear previous anchor styles
  host.style.top = "";
  host.style.bottom = "";
  host.style.left = "";
  host.style.right = "";
  host.style.transform = "";

  const container = shadow?.querySelector(".container");
  if (container) {
    container.classList.toggle("align-left", preset.includes("left"));
    container.classList.toggle("align-center", preset.includes("center"));
    container.classList.toggle("align-right", preset.includes("right") || (!preset.includes("left") && !preset.includes("center")));
    container.classList.toggle("flow-up", preset.startsWith("bottom"));
    container.style.scale = String(scale);
    container.style.transformOrigin = `${preset.includes("left") ? "left" : preset.includes("center") ? "center" : "right"} ${anchorBottom ? "bottom" : "top"}`;
  }

  // Vertical anchors match the preview exactly. Bottom-origin presets use
  // `bottom` so scaling preserves the selected corner instead of shifting it.
  if (preset.startsWith("outside-top")) {
    host.style.bottom = `${window.innerHeight - rect.top + 8 - offsetY}px`;
  } else if (preset.startsWith("outside-bottom")) {
    host.style.top = `${rect.bottom + 8 + offsetY}px`;
  } else if (preset.startsWith("bottom")) {
    host.style.bottom = `${window.innerHeight - rect.bottom + 48 - offsetY}px`;
  } else {
    host.style.top = `${rect.top + 12 + offsetY}px`;
  }

  // Horizontal anchor calculation
  if (preset.includes("left")) {
    const targetLeft = rect.left + 12 + offsetX;
    const left = Math.max(8, Math.min(window.innerWidth - 120, targetLeft));
    host.style.left = `${left}px`;
  } else if (preset.includes("center")) {
    const centerX = (rect.left + rect.width / 2) + offsetX;
    const left = Math.max(60, Math.min(window.innerWidth - 60, centerX));
    host.style.left = `${left}px`;
    host.style.transform = "translateX(-50%)";
  } else {
    // right
    const targetRight = (window.innerWidth - rect.right) + 12 - offsetX;
    const maxRight = Math.max(8, window.innerWidth - 500);
    const right = Math.max(8, Math.min(maxRight, targetRight));
    host.style.right = `${right}px`;
  }
}

function teardownHost() {
  if (mainBtnResetTimer) {
    clearTimeout(mainBtnResetTimer);
    mainBtnResetTimer = null;
  }
  if (host) {
    host.remove();
    host = null;
    shadow = null;
  }
}

let statusHideTimer = null;
let lastAnalyzedVideo = null;
let lastAnalyzedMetadata = null;
let lastAnalyzedBinding = null;
let isAnalyzingVideo = false;
let mainBtnResetTimer = null;

function createFilmIcon() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "15");
  svg.setAttribute("height", "15");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.5");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const rect = document.createElementNS("http://www.w3.org/2000/svg", "rect");
  rect.setAttribute("x", "3");
  rect.setAttribute("y", "5");
  rect.setAttribute("width", "18");
  rect.setAttribute("height", "14");
  rect.setAttribute("rx", "1.5");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", "M8 5v14M16 5v14M3 12h18M3 8.5h5M3 15.5h5M16 8.5h5M16 15.5h5");
  svg.append(rect, path);
  return svg;
}

function createCheckIcon() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "15");
  svg.setAttribute("height", "15");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2.5");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", "M20 6L9 17l-5-5");
  svg.appendChild(path);
  return svg;
}

function createWarnIcon() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "15");
  svg.setAttribute("height", "15");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.5");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const circle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
  circle.setAttribute("cx", "12");
  circle.setAttribute("cy", "12");
  circle.setAttribute("r", "9");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", "M12 7.5v5M12 16h.01");
  svg.append(circle, path);
  return svg;
}

function createDiscordIcon() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "14");
  svg.setAttribute("height", "14");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "currentColor");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", "M20.317 4.37a19.791 19.791 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994.021-.041.001-.09-.041-.106a13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128 10.2 10.2 0 0 0 .372-.292.074.074 0 0 1 .077-.01c3.929 1.793 8.18 1.793 12.061 0a.074.074 0 0 1 .078.01c.12.098.246.198.373.292a.077.077 0 0 1-.006.127 12.299 12.299 0 0 1-1.873.894.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.028zM8.02 15.33c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.956-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.956 2.418-2.157 2.418zm7.975 0c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.946 2.418-2.157 2.418z");
  svg.appendChild(path);
  return svg;
}

function createSlidersIcon() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "13");
  svg.setAttribute("height", "13");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.75");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const p1 = document.createElementNS("http://www.w3.org/2000/svg", "path");
  p1.setAttribute("d", "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6");
  svg.appendChild(p1);
  return svg;
}

function updateMainButton(state, text = null) {
  if (mainBtnResetTimer) {
    clearTimeout(mainBtnResetTimer);
    mainBtnResetTimer = null;
  }
  const btn = shadow?.getElementById("zoltraak-btn");
  if (!btn) return;

  btn.classList.remove("loading", "success", "failed");

  const labelSpan = document.createElement("span");
  if (state === "loading") {
    btn.classList.add("loading");
    const spinner = document.createElement("span");
    spinner.className = "spinner";
    labelSpan.textContent = text || "Finding video…";
    btn.replaceChildren(spinner, labelSpan);
  } else if (state === "success") {
    btn.classList.add("success");
    labelSpan.textContent = text || "Video ready";
    btn.replaceChildren(createCheckIcon(), labelSpan);
    mainBtnResetTimer = setTimeout(() => {
      updateMainButton("idle");
    }, 2800);
  } else if (state === "failed") {
    btn.classList.add("failed");
    labelSpan.textContent = text || "No video found";
    btn.replaceChildren(createWarnIcon(), labelSpan);
    mainBtnResetTimer = setTimeout(() => {
      updateMainButton("idle");
    }, 3200);
  } else {
    labelSpan.textContent = text || "Download this video";
    btn.replaceChildren(createFilmIcon(), labelSpan);
  }
}

function setPanelStatus(panel, text, autoHide = false, tone = "", options = null) {
  const postUrl = tone === "bad" && options?.video
    ? socialPostPermalink(options.video) : null;
  if (postUrl) {
    text = "Open this post in Zoltraak to find its photos and videos.";
    tone = "warn";
    autoHide = false;
  }
  if (statusHideTimer) {
    clearTimeout(statusHideTimer);
    statusHideTimer = null;
  }
  panel.textContent = "";

  const container = document.createElement("div");
  container.className = "status-container";

  const topRow = document.createElement("div");
  topRow.className = "status-top-row";

  const row = document.createElement("div");
  row.className = tone ? `status-row ${tone}` : "status-row";
  row.setAttribute("role", "status");

  if (tone === "loading") {
    const spinner = document.createElement("span");
    spinner.className = "spinner";
    row.appendChild(spinner);
  } else if (tone === "bad") {
    const icon = document.createElement("span");
    icon.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7.5v5M12 16h.01"/></svg>`;
    row.appendChild(icon);
  } else if (tone === "warn") {
    const icon = document.createElement("span");
    icon.innerHTML = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M10.9 3.9 2 18.4A1.8 1.8 0 0 0 3.6 21h16.8a1.8 1.8 0 0 0 1.6-2.6L13.1 3.9a1.8 1.8 0 0 0-3.1 0Z"/><path d="M12 9.5v4M12 17h.01"/></svg>`;
    row.appendChild(icon);
  }

  const label = document.createElement("span");
  label.textContent = text;
  label.style.flex = "1";
  row.appendChild(label);
  topRow.appendChild(row);

  const refreshBtn = document.createElement("button");
  refreshBtn.type = "button";
  refreshBtn.className = tone === "loading" ? "refresh-btn rotating" : "refresh-btn";
  refreshBtn.title = "Refresh and find video again";
  refreshBtn.setAttribute("aria-label", "Refresh and find video again");
  refreshBtn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>`;
  refreshBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    refreshBtn.classList.add("rotating");
    void triggerFindVideo(panel, true);
  });
  topRow.appendChild(refreshBtn);

  container.appendChild(topRow);

  if (options?.url || postUrl) {
    const actions = document.createElement("div");
    actions.className = "status-actions";

    const openAppBtn = document.createElement("button");
    openAppBtn.type = "button";
    openAppBtn.className = "action-btn primary";
    openAppBtn.textContent = postUrl ? "Download post media" : "Open in Zoltraak App";
    openAppBtn.title = postUrl ? "Open this post in Images to find its photos and videos" : "Open in Zoltraak";
    openAppBtn.addEventListener("click", async () => {
      openAppBtn.disabled = true;
      try {
        if (postUrl && (!matchesPlayer(options.video, options.playerBinding)
            || socialPostPermalink(options.video) !== postUrl)) {
          label.textContent = "The post changed. Find the current video again.";
          return;
        }
        const res = await chrome.runtime.sendMessage({
          type: postUrl ? "zoltraak-prefill-post" : "zoltraak-prefill-video",
          url: postUrl || options.url,
          title: options.title,
          referrer: location.href,
          ...(postUrl ? { playerBinding: options.playerBinding, documentStartedAt: performance.timeOrigin } : {}),
        });
        if (res?.ok) {
          openAppBtn.textContent = postUrl ? "Opened in Images" : "Sent to Zoltraak";
        } else {
          label.textContent = res?.error || "Could not open Zoltraak. Check that the app is running and paired.";
          openAppBtn.disabled = false;
        }
      } catch {
        label.textContent = "Could not open Zoltraak. Check that the app is running and paired.";
        openAppBtn.disabled = false;
      }
    });
    actions.appendChild(openAppBtn);

    if (options?.onRetry) {
      const retryBtn = document.createElement("button");
      retryBtn.className = "action-btn";
      retryBtn.textContent = "Retry";
      retryBtn.addEventListener("click", () => options.onRetry());
      actions.appendChild(retryBtn);
    }

    const dismissBtn = document.createElement("button");
    dismissBtn.className = "action-dismiss";
    dismissBtn.textContent = "Dismiss";
    dismissBtn.addEventListener("click", () => {
      panel.classList.remove("open");
      panel.textContent = "";
    });
    actions.appendChild(dismissBtn);

    container.appendChild(actions);
  }

  panel.appendChild(container);

  if (autoHide) {
    statusHideTimer = setTimeout(() => {
      panel.classList.remove("open");
      panel.textContent = "";
    }, 3200);
  }
}

function renderButton() {
  // The single choke point for the video overlay — the hover handler renders
  // straight through here without passing checkAndRefresh().
  if (!overlayPrefs.showMediaButton) {
    teardownHost();
    return;
  }
  ensureHost();
  while (shadow.firstChild) {
    shadow.removeChild(shadow.firstChild);
  }

  const styleEl = document.createElement("style");
  styleEl.textContent = OVERLAY_TOKENS + `
    .container {
      pointer-events: auto;
      display: flex;
      flex-direction: column;
      align-items: flex-end;
    }
    .container.align-left {
      align-items: flex-start;
    }
    .container.align-center {
      align-items: center;
    }
    .container.align-right {
      align-items: flex-end;
    }
    .container.flow-up {
      flex-direction: column-reverse;
    }
    .container.flow-up .panel {
      margin-top: 0;
      margin-bottom: 8px;
    }

    /* The lineside plate: a machined bar set down on the page, not a glass
       lozenge floating in it. Square corners, hairline edge, real cast shadow. */
    .bar {
      display: inline-flex;
      align-items: center;
      gap: 10px;
      background: var(--face);
      border: 1px solid var(--edge-strong);
      border-radius: var(--hw-r-lg, 3px);
      padding: calc(7px * var(--density, 1)) 10px;
      box-shadow: var(--shadow-plate);
      user-select: none;
    }
    .btn {
      background: transparent;
      border: none;
      color: var(--ink);
      cursor: pointer;
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 0 0;
      margin: 0;
      font-stretch: 78%;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.11em;
      font-size: 11px;
      transition: color 120ms;
    }
    .btn svg { display: block; color: var(--clear); }
    .btn:hover { color: var(--clear); }
    .btn.loading { color: var(--clear); }
    .btn.loading .spinner { margin-right: 2px; }
    .btn.success { color: var(--clear); }
    .btn.success svg { color: var(--clear); }
    .btn.failed { color: var(--danger); }
    .btn.failed svg { color: var(--danger); }

    .close-btn {
      background: transparent;
      border: 1px solid transparent;
      border-radius: var(--hw-r, 2px);
      color: var(--ink-faint);
      cursor: pointer;
      padding: calc(2px * var(--density, 1)) 2px;
      margin: 0;
      display: flex;
      align-items: center;
      transition: color 120ms, border-color 120ms;
    }
    .close-btn svg { display: block; }
    .close-btn:hover { color: var(--ink); border-color: var(--edge); }

    .tune-btn {
      background: transparent;
      border: 1px solid transparent;
      border-radius: var(--hw-r, 2px);
      color: var(--ink-faint);
      cursor: pointer;
      padding: calc(2px * var(--density, 1)) 2px;
      margin: 0;
      display: flex;
      align-items: center;
      transition: color 120ms, border-color 120ms;
    }
    .tune-btn svg { display: block; }
    .tune-btn:hover { color: var(--route); border-color: var(--edge); }

    /* The format picker: one plate, rows divided by hairline, no nested cards. */
    .panel {
      margin-top: 8px;
      width: 480px;
      max-height: 480px;
      max-width: calc(100vw - 24px);
      overflow-y: auto;
      overflow-x: hidden;
      border-radius: var(--hw-r-lg, 3px);
      background: var(--face);
      border: 1px solid var(--edge-strong);
      color: var(--ink);
      font-size: 13px;
      box-shadow: var(--shadow-lift);
      display: none;
    }
    .panel.open { display: block; }
    .panel:empty { display: none !important; }
    .panel::-webkit-scrollbar { width: 10px; }
    .panel::-webkit-scrollbar-track { background: var(--recess); }
    .panel::-webkit-scrollbar-thumb {
      background: var(--edge);
      border: 3px solid var(--recess);
      border-radius: var(--hw-r, 2px);
    }
    .panel::-webkit-scrollbar-thumb:hover { background: var(--edge-strong); }

    .panel-header {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: calc(11px * var(--density, 1)) 14px;
      border-bottom: 1px solid var(--edge);
      background: var(--recess);
    }
    .poster-thumb {
      width: 76px;
      height: 44px;
      border-radius: var(--hw-r, 2px);
      object-fit: cover;
      flex-shrink: 0;
      border: 1px solid var(--edge);
      background: var(--recess);
    }
    .poster-fallback {
      width: 76px;
      height: 44px;
      border-radius: var(--hw-r, 2px);
      background: var(--recess);
      border: 1px solid var(--edge);
      display: flex;
      align-items: center;
      justify-content: center;
      flex-shrink: 0;
      color: var(--ink-faint);
    }
    .header-details { min-width: 0; flex: 1; }
    .header-title {
      font-size: 13px;
      color: var(--ink);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .header-url {
      margin-top: 3px;
      font-family: "Martian Mono Variable", ui-monospace, monospace;
      font-stretch: 75%;
      font-size: 11px;
      color: var(--ink-dim);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
      user-select: text;
    }
    .header-actions {
      display: flex;
      flex-direction: column;
      align-items: flex-end;
      gap: 6px;
      flex-shrink: 0;
    }
    .format-count {
      font-stretch: 78%;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.11em;
      font-size: 11px;
      color: var(--ink-faint);
      white-space: nowrap;
    }
    .open-app-btn {
      display: inline-flex;
      align-items: center;
      gap: 5px;
      background: transparent;
      border: 1px solid var(--edge-strong);
      border-radius: var(--hw-r, 2px);
      padding: calc(4px * var(--density, 1)) 9px;
      font-stretch: 78%;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.11em;
      font-size: 11px;
      color: var(--ink-dim);
      cursor: pointer;
      transition: background 120ms, color 120ms;
      white-space: nowrap;
    }
    .open-app-btn:hover { background: var(--panel); color: var(--ink); }

    .row {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: calc(11px * var(--density, 1)) 14px;
      border-bottom: 1px solid var(--edge);
      transition: background 120ms;
    }
    .row:last-child { border-bottom: none; }
    .row:hover { background: var(--panel); }
    .row-thumb {
      width: 60px;
      height: 36px;
      border-radius: var(--hw-r, 2px);
      object-fit: cover;
      flex-shrink: 0;
      border: 1px solid var(--edge);
      background: var(--recess);
    }
    .row-thumb-fallback {
      width: 60px;
      height: 36px;
      border-radius: var(--hw-r, 2px);
      background: var(--recess);
      border: 1px solid var(--edge);
      display: flex;
      align-items: center;
      justify-content: center;
      flex-shrink: 0;
      color: var(--ink-faint);
    }
    .fmt-info { flex: 1; min-width: 0; }
    .fmt-head { display: flex; align-items: baseline; gap: 9px; }
    .fmt-label { font-size: 13px; color: var(--ink); }
    .fmt-container {
      font-stretch: 78%;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.11em;
      font-size: 11px;
      color: var(--ink-faint);
    }
    .fmt-badge {
      font-stretch: 78%;
      font-weight: 600;
      letter-spacing: 0.11em;
      text-transform: uppercase;
      font-size: 11px;
      border: 1px solid var(--edge-strong);
      border-radius: var(--hw-r, 2px);
      padding: calc(1px * var(--density, 1)) 6px;
      color: var(--ink-faint);
      white-space: nowrap;
    }
    /* A source the engine could not pair with audio is still offered — it may
       be all there is — but never unlabelled. */
    .fmt-badge.warn {
      border-color: var(--caution);
      color: var(--caution);
    }
    .fmt-meta {
      margin-top: 4px;
      font-family: "Martian Mono Variable", ui-monospace, monospace;
      font-stretch: 75%;
      font-size: 11px;
      font-variant-numeric: tabular-nums;
      color: var(--ink-dim);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }

    .dl-btn {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      padding: calc(7px * var(--density, 1)) 12px;
      border-radius: var(--hw-r, 2px);
      border: 1px solid var(--route);
      background: var(--route);
      color: var(--on-signal);
      font-stretch: 78%;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.11em;
      font-size: 11px;
      cursor: pointer;
      flex-shrink: 0;
      transition: filter 120ms;
    }
    .dl-btn:hover { filter: brightness(1.1); }
    .dl-btn:disabled { opacity: 0.45; cursor: not-allowed; filter: none; }
    .dl-btn.success {
      border-color: var(--clear);
      background: var(--clear);
      color: var(--on-signal);
      cursor: default;
    }
    .dl-btn svg { display: block; }
    /* The old pill wrapped its glyph in a circular well. Nothing on this panel
       is circular. */
    .dl-icon-circle { display: inline-flex; align-items: center; }

    .dl-btn-group {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      flex-shrink: 0;
    }
    .dl-discord-btn {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 29px;
      height: 29px;
      padding: 0 0;
      border-radius: var(--hw-r, 2px);
      border: 1px solid var(--edge-strong);
      background: transparent;
      color: var(--ink-dim);
      cursor: pointer;
      flex-shrink: 0;
      transition: all 120ms;
    }
    .dl-discord-btn:hover {
      border-color: #5865F2;
      background: rgba(88, 101, 242, 0.12);
      color: #5865F2;
    }
    .dl-discord-btn:disabled {
      opacity: 0.45;
      cursor: not-allowed;
    }
    .dl-discord-btn.success {
      border-color: #5865F2;
      background: #5865F2;
      color: #ffffff;
      cursor: default;
    }
    .dl-discord-btn svg { display: block; }

    .header-actions-top {
      display: flex;
      align-items: center;
      gap: 8px;
    }
    .stream-choice {
      display:flex;align-items:center;gap:10px;width:100%;min-height:44px;
      padding: calc(10px * var(--density, 1)) 10px;margin-top:8px;text-align:left;white-space:normal;
      overflow-wrap:anywhere;background:var(--face);
      color:var(--ink);border:1px solid var(--edge-strong);border-radius:6px;
      font:inherit;cursor:pointer;
    }
    .stream-choice img {width:64px;height:42px;object-fit:cover;border-radius: var(--hw-r-lg, 3px);}
    .stream-choice:hover {border-color:currentColor;}
    .stream-choice:focus-visible {outline:2px solid var(--clear);outline-offset:2px;}
    .panel.choosing {padding: calc(12px * var(--density, 1)) 12px;box-sizing:border-box;}
    .refresh-btn {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 22px;
      height: 22px;
      padding: 0 0;
      border-radius: var(--hw-r, 2px);
      border: 1px solid var(--edge-strong);
      background: transparent;
      color: var(--ink-dim);
      cursor: pointer;
      flex-shrink: 0;
      transition: background 120ms, color 120ms, border-color 120ms;
    }
    .refresh-btn:hover {
      background: var(--panel);
      color: var(--ink);
      border-color: var(--edge-strong);
    }
    .refresh-btn:disabled {
      opacity: 0.45;
      cursor: not-allowed;
    }
    .refresh-btn.rotating svg {
      animation: spin 0.8s linear infinite;
    }
    @keyframes spin {
      from { transform: rotate(0deg); }
      to { transform: rotate(360deg); }
    }

    .status-container {
      padding: calc(14px * var(--density, 1)) 16px;
      display: flex;
      flex-direction: column;
      gap: 10px;
    }
    .status-top-row {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 10px;
    }
    .status-row {
      display: flex;
      align-items: center;
      gap: 10px;
      font-size: 13px;
      color: var(--ink);
      flex: 1;
      min-width: 0;
    }
    .status-row.bad { color: var(--danger); }
    .status-row.warn { color: var(--caution); }
    .status-actions {
      display: flex;
      align-items: center;
      gap: 8px;
      margin-top: 2px;
      flex-wrap: wrap;
    }
    .action-btn {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      padding: calc(6px * var(--density, 1)) 12px;
      border-radius: var(--hw-r, 2px);
      font-stretch: 78%;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.11em;
      font-size: 11px;
      border: 1px solid var(--edge-strong);
      background: transparent;
      color: var(--ink-dim);
      cursor: pointer;
      transition: background 120ms, color 120ms;
    }
    .action-btn:hover { background: var(--panel); color: var(--ink); }
    .action-btn.primary {
      background: var(--route);
      border-color: var(--route);
      color: var(--on-signal);
    }
    .action-btn.primary:hover { filter: brightness(1.1); background: var(--route); }
    .action-dismiss {
      background: transparent;
      border: none;
      color: var(--ink-faint);
      font-stretch: 78%;
      font-weight: 600;
      text-transform: uppercase;
      letter-spacing: 0.11em;
      font-size: 11px;
      cursor: pointer;
      padding: calc(4px * var(--density, 1)) 6px;
    }
    .action-dismiss:hover { color: var(--ink); }

    /* Work in progress is a lit lamp on a hairline track, not a spinning ring:
       nothing in this world is circular and nothing rotates. */
    .spinner {
      width: 22px;
      height: 4px;
      border: none;
      border-radius: var(--hw-r-sm, 1px);
      background: linear-gradient(
        to right,
        var(--clear) 0 33%,
        var(--lamp-off) 33% 100%
      );
      background-size: 300% 100%;
      animation: sweep 1.1s steps(3, end) infinite;
      flex-shrink: 0;
    }
    @keyframes sweep {
      from { background-position: 100% 0; }
      to { background-position: 0% 0; }
    }
  `;
  shadow.appendChild(styleEl);

  const container = document.createElement("div");
  container.className = "container";

  const bar = document.createElement("div");
  bar.className = "bar";

  const button = document.createElement("button");
  button.id = "zoltraak-btn";
  button.className = "btn";

  if (isAnalyzingVideo) {
    button.classList.add("loading");
    const spinner = document.createElement("span");
    spinner.className = "spinner";
    const label = document.createElement("span");
    label.textContent = "Finding video…";
    button.replaceChildren(spinner, label);
  } else {
    const label = document.createElement("span");
    label.textContent = "Download this video";
    button.replaceChildren(createFilmIcon(), label);
  }

  const closeBtn = document.createElement("button");
  closeBtn.id = "zoltraak-close";
  closeBtn.className = "close-btn";
  closeBtn.title = "Hide for this video";
  closeBtn.setAttribute("aria-label", "Hide for this video");
  closeBtn.innerHTML =
    `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" ` +
    `stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">` +
    `<path d="M6 6l12 12M18 6 6 18"/></svg>`;

  const tuneBtn = document.createElement("button");
  tuneBtn.id = "zoltraak-tune";
  tuneBtn.className = "tune-btn";
  tuneBtn.title = "Configure overlay placement & scale";
  tuneBtn.setAttribute("aria-label", "Configure overlay placement & scale");
  tuneBtn.appendChild(createSlidersIcon());
  tuneBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    chrome.runtime.sendMessage({ type: "zoltraak-open-placement" });
  });

  bar.appendChild(button);
  bar.appendChild(tuneBtn);
  bar.appendChild(closeBtn);

  const panel = document.createElement("div");
  panel.id = "panel";
  panel.className = "panel";

  container.appendChild(bar);
  container.appendChild(panel);
  shadow.appendChild(container);

  button.addEventListener("click", () => void onButtonClick(panel));
  closeBtn.addEventListener("click", () => {
    dismissedVideo = currentVideo;
    teardownHost();
    lastAnalyzedVideo = null;
    lastAnalyzedMetadata = null;
  });

  updateHostPosition();
}

/**
 * The floating button opens the same picker the app's Video view shows for a
 * URL — every rendition, best first, each with its own Download — for the
 * video the overlay is sitting on.
 */
/** The streams this tab fetched since the current video became current. */
async function tabCaptures() {
  try {
    const since = currentVideoSince ? currentVideoSince - 5000 : 0;
    const res = await chrome.runtime.sendMessage({ type: "zoltraak-tab-media", since, documentStartedAt: performance.timeOrigin });
    const media = res?.ok && Array.isArray(res.media) ? res.media.map((m) => m.url) : [];
    return media;
  } catch {
    return [];
  }
}

function findActiveVideoUnderButton() {
  if (currentVideo && currentVideo.isConnected) {
    const r = currentVideo.getBoundingClientRect();
    if (r.width > 0 && r.height > 0) {
      return currentVideo;
    }
  }

  if (host) {
    const hostRect = host.getBoundingClientRect();
    const hostCenterX = hostRect.left + hostRect.width / 2;
    const hostCenterY = hostRect.top + hostRect.height / 2;

    const videos = Array.from(document.querySelectorAll("video")).filter(isEligible);
    let bestVideo = null;
    let minDistance = Infinity;

    for (const v of videos) {
      const vr = v.getBoundingClientRect();
      if (
        hostCenterX >= vr.left &&
        hostCenterX <= vr.right &&
        hostCenterY >= vr.top &&
        hostCenterY <= vr.bottom
      ) {
        return v;
      }
      const vCenterX = vr.left + vr.width / 2;
      const vCenterY = vr.top + vr.height / 2;
      const dist = Math.hypot(hostCenterX - vCenterX, hostCenterY - vCenterY);
      if (dist < minDistance) {
        minDistance = dist;
        bestVideo = v;
      }
    }
    if (bestVideo) return bestVideo;
  }

  return pickVideo();
}

function getVideoContext(video) {
  if (!video) return null;
  const generic = globalThis.ZoltraakGenericMedia?.recordFor(video);
  const duration = Number.isFinite(video.duration) && video.duration > 0 ? video.duration : null;
  const width = video.videoWidth || null;
  const height = video.videoHeight || null;
  const aspectRatio = width && height ? width / height : null;
  const currentSrc = generic?.sources.find((s) => s.evidence === "mse-buffer")?.url || video.currentSrc || video.src || null;
  const permalink = videoPermalink(video);
  const currentTime = Number.isFinite(video.currentTime) ? video.currentTime : 0;

  let recentMediaUrls = [];
  try {
    const entries = performance.getEntriesByType("resource");
    const mediaEntries = entries.slice(-60).filter((e) => {
      const u = e.name || "";
      return (
        /\.(mp4|webm|m3u8|mpd|m4s|ts)(\?|$)/i.test(u) ||
        /(bytestart|range=|\/videoplayback|\/seg-|\.fbcdn\.net\/v\/|\.cdninstagram\.com)/i.test(u)
      );
    });
    recentMediaUrls = mediaEntries.map((e) => e.name);
  } catch {
    // ignore
  }

  return {
    duration,
    currentTime,
    width,
    height,
    aspectRatio,
    currentSrc,
    permalink,
    recentMediaUrls,
    elementManifest: manifestFromAttributes(video) || generic?.sources.find((s) => s.kind === "hls" || s.kind === "dash")?.url,
    sourceKinds: Object.fromEntries((generic?.sources || []).map((s) => [s.url, s.kind])),
    protected: generic?.protected || Boolean(video.mediaKeys),
    live: generic?.live || video.duration === Infinity,
    playerBinding: playerBinding(video),
    documentStartedAt: performance.timeOrigin,
    // Resource timings remain diagnostics, not proof of player ownership.
  };
}

async function candidateTargets(targetVideo) {
  const vid = targetVideo || currentVideo;
  if (!vid) return null;
  if (isXHost()) return [xPermalinkFromNode(vid), location.href].filter(Boolean);
  if (isYouTubeHost()) return [youTubeWatchUrl(), location.href].filter(Boolean);
  if (hasSiteExtractor()) {
    // A site the engine understands: the item's own permalink first — the
    // extractor reads the real manifest and pairs picture with sound. Then
    // the page URL (if it's an item permalink), the element's direct source /
    // manifest, and then captured tracks.
    const permalink = videoPermalink(vid);
    const own = document.querySelectorAll("video").length === 1 && isItemPermalink(location.href) ? location.href : null;
    const direct = directSrc(vid);
    const manifest = manifestFromAttributes(vid);
    if (/(^|\.)facebook\.com$/i.test(location.hostname) && permalink) {
      // CDN MP4 requests can be silent DASH tracks, not complete videos.
      // Resolve the post's paired formats or its own manifest instead.
      return Array.from(new Set([permalink, manifest].filter(Boolean)));
    }
    if (/(^|\.)instagram\.com$/i.test(location.hostname) && permalink) {
      return Array.from(new Set([permalink, direct, manifest].filter(Boolean)));
    }
    if (/(^|\.)tiktok\.com$/i.test(location.hostname)) {
      const bound = (globalThis.ZoltraakGenericMedia?.sourcesFor(vid) || [])
        .filter((s) => ["element", "element-manifest", "mse-buffer", "mse-manifest"].includes(s.evidence))
        .map((s) => s.url);
      return Array.from(new Set([permalink, own, direct, manifest, ...bound].filter(Boolean)));
    }
    const captures = await tabCaptures();
    return Array.from(new Set([permalink, own, direct, manifest, ...captures, location.href].filter(Boolean)));
  }
  return null; // generic: gatherSources() decides
}

async function onButtonClick(panel) {
  if (panel.classList.contains("open")) {
    panel.classList.remove("open");
    if (statusHideTimer) {
      clearTimeout(statusHideTimer);
      statusHideTimer = null;
    }
    return;
  }

  // Re-verify the active video under the button
  const video = findActiveVideoUnderButton();
  if (video) {
    currentVideo = video;
  }
  if (!currentVideo) {
    setPanelStatus(panel, "No active video found under button.", false, "bad");
    panel.classList.add("open");
    return;
  }

  // If the dropdown was already opened initially and we already have results for this video,
  // ensure the panel has content and show it without re-finding.
  if (lastAnalyzedVideo === currentVideo && lastAnalyzedMetadata && matchesPlayer(currentVideo, lastAnalyzedBinding)) {
    if (panel.children.length === 0) {
      renderFormats(panel, lastAnalyzedMetadata, lastAnalyzedBinding);
    }
    panel.classList.add("open");
    return;
  }

  // If already analyzing, ensure loading status is visible and open panel
  if (isAnalyzingVideo) {
    if (panel.children.length === 0) {
      setPanelStatus(panel, "Finding the video…", false, "loading");
    }
    panel.classList.add("open");
    updateMainButton("loading", "Finding video…");
    return;
  }

  await triggerFindVideo(panel, false);
}

async function triggerFindVideo(panel, force = false) {
  const observed = playerBinding(currentVideo);
  if (observed?.generation !== currentPlayerGeneration) {
    currentPlayerGeneration = observed?.generation;
    currentVideoSince = observed?.playbackStartedAt || 0;
    lastAnalyzedVideo = null;
    lastAnalyzedMetadata = null;
  }
  if (!force && lastAnalyzedVideo === currentVideo && lastAnalyzedMetadata && matchesPlayer(currentVideo, lastAnalyzedBinding)) {
    renderFormats(panel, lastAnalyzedMetadata, lastAnalyzedBinding);
    panel.classList.add("open");
    return;
  }

  if (isAnalyzingVideo) {
    if (panel.children.length === 0) {
      setPanelStatus(panel, "Finding the video…", false, "loading");
    }
    panel.classList.add("open");
    updateMainButton("loading", "Finding video…");
    return;
  }

  if (!chrome.runtime?.id) {
    setPanelStatus(panel, "Extension was updated/reloaded. Please refresh this tab (F5).", false, "bad");
    panel.classList.add("open");
    updateMainButton("failed", "Reload tab");
    return;
  }

  isAnalyzingVideo = true;
  updateMainButton("loading", "Finding video…");
  setPanelStatus(panel, "Finding the video…", false, "loading");
  panel.classList.add("open");
  const targetVideo = currentVideo;
  const title = (document.title || "").replace(/\s*-\s*YouTube\s*$/i, "") || undefined;
  const videoContext = getVideoContext(targetVideo);

  try {
    let candidates = await candidateTargets(targetVideo);
    if (!candidates || candidates.length === 0) {
      try {
        candidates = await gatherSources(targetVideo);
      } catch {
        candidates = [nearestPermalink(targetVideo), location.href].filter(Boolean);
      }
    }
    if (!candidates || candidates.length === 0) {
      candidates = [location.href];
    }

    const res = await chrome.runtime.sendMessage({
      type: "zoltraak-analyze-best",
      candidates: candidates.filter(Boolean),
      referrer: location.href,
      title,
      videoContext,
      hlsCandidates: hlsFallbackSources(targetVideo),
      documentStartedAt: performance.timeOrigin,
    });
    if (!panel.classList.contains("open") && !force) return;
    if (targetVideo !== currentVideo || !matchesPlayer(targetVideo, videoContext?.playerBinding)) return;

    if (res?.choices?.length) {
      renderVideoChoices(panel, res.choices, targetVideo, videoContext.playerBinding);
      return;
    }

    if (res?.ok && res.metadata?.formats?.length) {
      // Client-side verification guard: if both client and metadata duration are available,
      // ensure we don't display a candidate with a significant duration mismatch (> 3s and > 10% diff)
      if (
        typeof videoContext?.duration === "number" &&
        videoContext.duration > 1 &&
        typeof res.metadata.duration === "number" &&
        res.metadata.duration > 1
      ) {
        const diff = Math.abs(res.metadata.duration - videoContext.duration);
        const relDiff = diff / videoContext.duration;
        if (diff > 3.0 && relDiff > 0.1) {
          setPanelStatus(
            panel,
            `Found video duration (${res.metadata.duration.toFixed(1)}s) did not match the player on screen (${videoContext.duration.toFixed(1)}s).`,
            false,
            "bad",
            {
              url: candidates[0] || location.href,
              video: targetVideo,
              playerBinding: videoContext.playerBinding,
              title,
              onRetry: () => void triggerFindVideo(panel, true),
            }
          );
          updateMainButton("failed", "Mismatch");
          return;
        }
      }

      lastAnalyzedVideo = targetVideo;
      lastAnalyzedMetadata = res.metadata;
      lastAnalyzedBinding = videoContext.playerBinding;
      renderFormats(panel, res.metadata, videoContext.playerBinding);
      updateMainButton("success", "Video ready");
      return;
    }

    // YouTube: when the extractor is blocked (bot check, a gate the session
    // did not clear), the player on this page has already fetched the real
    // streams with the user's session. Start the best captured pair directly
    // rather than showing an empty list.
    if (isYouTubeHost()) {
      setPanelStatus(panel, "Using the player's own streams…", false, "loading");
      const cap = await chrome.runtime.sendMessage({
        type: "zoltraak-youtube-capture",
        webpageUrl: youTubeWatchUrl(),
        title,
        since: currentVideoSince,
        videoContext,
        documentStartedAt: performance.timeOrigin,
      });
      if (!panel.classList.contains("open") && !force) return;
      if (!matchesPlayer(targetVideo, videoContext.playerBinding)) return;
      reportStarted(panel, cap);
      updateMainButton(cap?.ok ? "success" : "failed", cap?.ok ? "Video ready" : "Failed");
      return;
    }

    const targetUrl = candidates[0] || location.href;
    setPanelStatus(
      panel,
      res?.error || "No downloadable video found.",
      false,
      "bad",
      {
        url: targetUrl,
        video: targetVideo,
        playerBinding: videoContext.playerBinding,
        title,
        onRetry: () => void triggerFindVideo(panel, true),
      }
    );
    updateMainButton("failed", "No video found");
  } catch (e) {
    if (!matchesPlayer(targetVideo, videoContext?.playerBinding)) return;
    const msg = String(e?.message || e);
    const isConnErr = msg.includes("Could not establish connection") || msg.includes("context invalidated");
    setPanelStatus(
      panel,
      isConnErr
        ? "Extension was reloaded — refresh this tab (F5)."
        : "Could not reach the Zoltraak desktop app.",
      false,
      "bad",
      {
        url: location.href,
        video: targetVideo,
        playerBinding: videoContext.playerBinding,
        title,
        onRetry: () => void triggerFindVideo(panel, true),
      }
    );
    updateMainButton("failed", "Search failed");
  } finally {
    isAnalyzingVideo = false;
    if (!matchesPlayer(targetVideo, videoContext?.playerBinding)) updateMainButton("idle");
  }
}

function renderVideoChoices(panel, choices, video, binding) {
  panel.replaceChildren();
  panel.classList.add("choosing");
  const heading = document.createElement("p");
  heading.className = "header-title";
  heading.textContent = "Which video did you mean?";
  panel.appendChild(heading);
  const note = document.createElement("p");
  note.className = "header-url";
  note.textContent = "Check the video details before choosing a download format.";
  panel.appendChild(note);
  for (const [index, choice] of choices.entries()) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "stream-choice";
    if (choice.metadata.thumbnail) {
      const image = document.createElement("img");
      image.src = choice.metadata.thumbnail;
      image.alt = "";
      image.referrerPolicy = "no-referrer";
      image.addEventListener("error", () => image.remove());
      button.appendChild(image);
    }
    const text = document.createElement("span");
    const duration = choice.metadata.duration;
    const quality = choice.metadata.formats?.[0]?.height;
    text.textContent = [choice.metadata.title || `Video ${index + 1}`,
      duration ? `${Math.round(duration)} seconds` : null,
      quality ? `${quality}p` : null, new URL(choice.url).hostname].filter(Boolean).join(" · ");
    button.appendChild(text);
    button.addEventListener("click", () => {
      if (!matchesPlayer(video, binding)) { void triggerFindVideo(panel, true); return; }
      lastAnalyzedVideo = video;
      lastAnalyzedMetadata = choice.metadata;
      lastAnalyzedBinding = binding;
      renderFormats(panel, choice.metadata, binding);
      updateMainButton("success", "Video ready");
    });
    panel.appendChild(button);
  }
  panel.classList.add("open");
  updateMainButton("success", "Choose video");
  panel.querySelector("button")?.focus();
}

function formatLabel(format) {
  if (format.height) return `${format.height}p`;
  if (format.bitrate) return `${Math.round(format.bitrate / 1000)}kbps`;
  return format.id;
}

/** What the row has to admit about its sound — mirrors the app's audioNote. */
function audioNote(format) {
  if (format.requiresMerge) return { text: "merges audio", warn: false };
  if (globalThis.ZoltraakVideoIdentity.isAudioOnly(format)) return { text: "audio only", warn: true };
  if (format.hasAudio === false) return { text: "no audio", warn: true };
  if (format.hasAudio == null) return { text: "audio unverified", warn: true };
  if (format.validation?.status !== "verified") return { text: "audio inferred", warn: false };
  return null;
}

function renderFormats(panel, metadata, binding = playerBinding(currentVideo)) {
  const selectedVideo = currentVideo;
  panel.classList.remove("choosing");
  if (statusHideTimer) {
    clearTimeout(statusHideTimer);
    statusHideTimer = null;
  }
  panel.textContent = "";

  // 1. Panel Header (matching desktop app's VideoView header)
  const header = document.createElement("div");
  header.className = "panel-header";

  if (metadata.thumbnail) {
    const poster = document.createElement("img");
    poster.className = "poster-thumb";
    poster.src = metadata.thumbnail;
    poster.alt = "";
    poster.onerror = () => {
      poster.style.display = "none";
      if (fallback) fallback.style.display = "flex";
    };
    header.appendChild(poster);
  }
  const fallback = document.createElement("div");
  fallback.className = "poster-fallback";
  fallback.style.display = metadata.thumbnail ? "none" : "flex";
  fallback.innerHTML = `<svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>`;
  header.appendChild(fallback);

  const details = document.createElement("div");
  details.className = "header-details";

  const titleEl = document.createElement("div");
  titleEl.className = "header-title";
  titleEl.textContent = metadata.title || "Video";
  titleEl.title = metadata.title || "";
  details.appendChild(titleEl);

  const urlEl = document.createElement("div");
  urlEl.className = "header-url";
  urlEl.textContent = metadata.webpageUrl || location.href;
  urlEl.title = metadata.webpageUrl || location.href;
  details.appendChild(urlEl);

  header.appendChild(details);

  const actions = document.createElement("div");
  actions.className = "header-actions";

  const actionsTop = document.createElement("div");
  actionsTop.className = "header-actions-top";

  const countBadge = document.createElement("div");
  countBadge.className = "format-count";
  const count = metadata.formats.length;
  countBadge.textContent = `${count} ${count === 1 ? "format" : "formats"}`;
  actionsTop.appendChild(countBadge);

  const refreshBtn = document.createElement("button");
  refreshBtn.type = "button";
  refreshBtn.className = "refresh-btn";
  refreshBtn.title = "Refresh and find video again";
  refreshBtn.setAttribute("aria-label", "Refresh and find video again");
  refreshBtn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>`;
  refreshBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    refreshBtn.classList.add("rotating");
    void triggerFindVideo(panel, true);
  });
  actionsTop.appendChild(refreshBtn);

  actions.appendChild(actionsTop);

  const openAppBtn = document.createElement("button");
  openAppBtn.className = "open-app-btn";
  openAppBtn.title = "Open this video in Zoltraak Desktop App";
  openAppBtn.innerHTML = `<span>Open in App</span><svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14L21 3"/></svg>`;
  openAppBtn.addEventListener("click", async () => {
    openAppBtn.disabled = true;
    try {
      const res = await chrome.runtime.sendMessage({
        type: "zoltraak-prefill-video",
        url: metadata.webpageUrl || location.href,
        title: metadata.title,
        referrer: location.href,
      });
      if (res?.ok) {
        openAppBtn.textContent = "Opened in App";
      } else {
        openAppBtn.textContent = "Could not reach app";
      }
    } catch {
      openAppBtn.textContent = "Failed";
    }
  });
  actions.appendChild(openAppBtn);

  header.appendChild(actions);
  panel.appendChild(header);

  // 2. Format List (matching FormatRow in VideoView)
  for (const format of metadata.formats) {
    const row = document.createElement("div");
    row.className = "row";

    // Row Thumbnail
    if (metadata.thumbnail) {
      const rowImg = document.createElement("img");
      rowImg.className = "row-thumb";
      rowImg.src = metadata.thumbnail;
      rowImg.alt = "";
      rowImg.loading = "lazy";
      rowImg.onerror = () => {
        rowImg.style.display = "none";
        if (rowFallback) rowFallback.style.display = "flex";
      };
      row.appendChild(rowImg);
    }
    const rowFallback = document.createElement("div");
    rowFallback.className = "row-thumb-fallback";
    rowFallback.style.display = metadata.thumbnail ? "none" : "flex";
    rowFallback.innerHTML = `<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>`;
    row.appendChild(rowFallback);

    // Row Info
    const info = document.createElement("div");
    info.className = "fmt-info";

    const head = document.createElement("div");
    head.className = "fmt-head";

    const label = document.createElement("span");
    label.className = "fmt-label";
    label.textContent = formatLabel(format);
    head.appendChild(label);

    if (format.container) {
      const container = document.createElement("span");
      container.className = "fmt-container";
      container.textContent = format.container.toUpperCase();
      head.appendChild(container);
    }

    const note = audioNote(format);
    if (note) {
      const badge = document.createElement("span");
      badge.className = note.warn ? "fmt-badge warn" : "fmt-badge";
      badge.textContent = note.text.toUpperCase();
      head.appendChild(badge);
    }
    info.appendChild(head);

    const meta = document.createElement("div");
    meta.className = "fmt-meta";
    meta.textContent =
      [
        format.bitrate ? `${Math.round(format.bitrate / 1000)} kbps` : null,
        format.filesize ? formatBytesShort(format.filesize) : null,
        format.videoCodec,
      ]
        .filter(Boolean)
        .join(" · ") || "—";
    info.appendChild(meta);

    row.appendChild(info);

    const btnGroup = document.createElement("div");
    btnGroup.className = "dl-btn-group";

    // Download Button (green pill with down arrow)
    const button = document.createElement("button");
    button.type = "button";
    button.className = "dl-btn";
    button.innerHTML = `<span>Download</span><span class="dl-icon-circle"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v12M6 12l6 6 6-6"/></svg></span>`;

    button.addEventListener("click", async () => {
      if (!matchesPlayer(selectedVideo, binding)) { void triggerFindVideo(panel, true); return; }
      button.disabled = true;
      button.innerHTML = `<span>Starting…</span><span class="spinner"></span>`;
      try {
        const started = await chrome.runtime.sendMessage({
          type: "zoltraak-download",
          title: metadata.title,
          webpageUrl: metadata.webpageUrl,
          format,
          playerBinding: binding,
          documentStartedAt: performance.timeOrigin,
          autoDiscord: false,
        });
        if (started?.ok) {
          scheduleImmediateRepoll();
          button.className = "dl-btn success";
          button.innerHTML = `<span>Started</span><span class="dl-icon-circle"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg></span>`;
        } else {
          button.className = "dl-btn";
          button.textContent = "Failed";
          button.title = started?.error || "";
          button.disabled = false;
        }
      } catch {
        button.className = "dl-btn";
        button.textContent = "Failed";
        button.disabled = false;
      }
    });
    btnGroup.appendChild(button);

    // Download & Auto-send to Discord button
    const discordBtn = document.createElement("button");
    discordBtn.type = "button";
    discordBtn.className = "dl-discord-btn";
    discordBtn.title = "Download and send to Discord";
    discordBtn.setAttribute("aria-label", "Download and send to Discord");
    discordBtn.replaceChildren(createDiscordIcon());

    discordBtn.addEventListener("click", async () => {
      if (!matchesPlayer(selectedVideo, binding)) { void triggerFindVideo(panel, true); return; }
      discordBtn.disabled = true;
      discordBtn.innerHTML = `<span class="spinner" style="width:14px;height:3px;"></span>`;
      try {
        const started = await chrome.runtime.sendMessage({
          type: "zoltraak-download",
          title: metadata.title,
          webpageUrl: metadata.webpageUrl,
          format,
          playerBinding: binding,
          documentStartedAt: performance.timeOrigin,
          autoDiscord: true,
        });
        if (started?.ok) {
          scheduleImmediateRepoll();
          discordBtn.className = "dl-discord-btn success";
          discordBtn.title = "Started & sending to Discord";
          discordBtn.innerHTML = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg>`;
        } else {
          discordBtn.className = "dl-discord-btn";
          discordBtn.replaceChildren(createDiscordIcon());
          discordBtn.title = started?.error || "Failed";
          discordBtn.disabled = false;
        }
      } catch {
        discordBtn.className = "dl-discord-btn";
        discordBtn.replaceChildren(createDiscordIcon());
        discordBtn.title = "Failed";
        discordBtn.disabled = false;
      }
    });
    btnGroup.appendChild(discordBtn);

    row.appendChild(btnGroup);
    panel.appendChild(row);
  }
  panel.classList.add("open");
}

function reportStarted(panel, res) {
  if (res?.ok) {
    scheduleImmediateRepoll();
    if (res.complete === false) {
      setPanelStatus(panel, `Downloading ${res.label} — no audio track`, true, "warn");
    } else if (res.audioStatus === "unknown") {
      setPanelStatus(panel, `Downloading ${res.label || "best quality"} · audio unverified`, true, "warn");
    } else {
      setPanelStatus(panel, `Downloading ${res.label || "best quality"} · video + audio`, true);
    }
    return;
  }
  setPanelStatus(panel, res?.error || "Could not reach the Zoltraak app.", true, "bad");
}

let isAppRunning = false;
let lastHealthCheck = 0;
let healthCheckPromise = null;

async function checkAppHealth() {
  if (healthCheckPromise) {
    await healthCheckPromise.catch(() => null);
    return isAppRunning;
  }
  const now = Date.now();
  if (now - lastHealthCheck < 1000) {
    return isAppRunning;
  }
  lastHealthCheck = now;
  if (!chrome.runtime?.id) {
    isAppRunning = false;
    return false;
  }
  try {
    healthCheckPromise = chrome.runtime.sendMessage({ type: "zoltraak-health" });
    const res = await healthCheckPromise;
    isAppRunning = Boolean(res?.ok && res?.up);
  } catch {
    isAppRunning = false;
  } finally {
    healthCheckPromise = null;
  }
  return isAppRunning;
}

function refresh() {
  void checkAndRefresh();
}

async function checkAndRefresh() {
  // Sub-frames and background/hidden tabs never show the button.
  if (!overlaysAllowed()) {
    currentVideo = null;
    teardownHost();
    return;
  }

  // On X.com, inject the three-dot dropdown menu hook while still allowing
  // the floating overlay button to render on active videos.
  if (isXHost()) {
    injectXDropdownMenu();
  }

  // Switched off in the popup. The X dropdown item above is a separate
  // integration and stays — this switch is only about the floating button.
  if (!overlayPrefs.showMediaButton) {
    currentVideo = null;
    teardownHost();
    return;
  }

  const up = await checkAppHealth();
  if (!up) {
    currentVideo = null;
    teardownHost();
    return;
  }

  const video = pickVideo();
  const binding = playerBinding(video);
  if (video === currentVideo && binding?.generation === currentPlayerGeneration) {
    if (currentVideo) updateHostPosition();
    return;
  }

  currentVideo = video;
  currentPlayerGeneration = binding?.generation;
  currentVideoSince = binding?.playbackStartedAt || 0;
  lastAnalyzedVideo = null;
  lastAnalyzedMetadata = null;
  if (!video) {
    teardownHost();
    lastAnalyzedVideo = null;
    lastAnalyzedMetadata = null;
    return;
  }
  if (lastAnalyzedVideo !== video) {
    lastAnalyzedVideo = null;
    lastAnalyzedMetadata = null;
  }
  renderButton();
}

function setupVideoListeners() {
  // Route through refresh()/pickVideo() rather than forcing a render here —
  // some players refire "playing" repeatedly on the *same* video (buffering,
  // seeking, ad breaks), and unconditionally resetting dismissedVideo made a
  // dismissed button reappear right after the user closed it.
  const handlePlay = (e) => {
    if (e.target && e.target.tagName === "VIDEO") refresh();
  };

  for (const event of ["loadstart", "emptied"]) {
    document.addEventListener(event, (e) => {
      if (e.target?.tagName !== "VIDEO") return;
      const binding = playerBinding(e.target, true);
      if (e.target === currentVideo && binding.generation !== currentPlayerGeneration) {
        lastAnalyzedVideo = null;
        lastAnalyzedMetadata = null;
        refresh();
      }
    }, true);
  }
  document.addEventListener("loadedmetadata", handlePlay, true);
  document.addEventListener("durationchange", handlePlay, true);
  document.addEventListener("play", handlePlay, true);
  document.addEventListener("playing", handlePlay, true);

  // Keep currentVideo synchronized when hovering over video players,
  // provided the panel is not currently open.
  let hoverTimer = null;
  const handleHover = (e) => {
    const video = e.target?.closest?.("video") || (e.target?.tagName === "VIDEO" ? e.target : null);
    if (!video || !isEligible(video)) return;
    if (video === currentVideo) return;
    if (shadow?.getElementById("panel")?.classList?.contains("open")) return;

    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(() => {
      if (video.isConnected && isEligible(video) && !shadow?.getElementById("panel")?.classList?.contains("open")) {
        currentVideo = video;
        const binding = playerBinding(video);
        currentPlayerGeneration = binding.generation;
        currentVideoSince = binding.playbackStartedAt;
        lastAnalyzedVideo = null;
        lastAnalyzedMetadata = null;
        renderButton();
      }
    }, 120);
  };
  document.addEventListener("pointerover", handleHover, true);
}

function watchSpaNavigation() {
  const notify = () => setTimeout(refresh, 300);
  for (const name of ["pushState", "replaceState"]) {
    const original = history[name];
    if (typeof original === "function") {
      history[name] = function (...args) {
        const result = original.apply(this, args);
        notify();
        return result;
      };
    }
  }
  window.addEventListener("popstate", notify);
}

/** The canonical tweet permalink for whatever node was right-clicked: a
 * `/status/<id>` link the click is inside, else one within the enclosing tweet
 * <article>, else the page URL. This is the address the app's Video view (and
 * X extractor) needs — the specific item, not the timeline. */
function formatXUrl(url) {
  if (!url || typeof url !== "string") return url;
  if (/https?:\/\/(?:[a-z0-9-]+\.)*(?:x\.com|twitter\.com)\//i.test(url)) {
    return url.split("?")[0];
  }
  return url;
}

function xPermalinkFromNode(node) {
  const el = node instanceof Element ? node : node?.parentElement;
  if (!el) return formatXUrl(location.href);
  const article = el.closest("article");
  const link =
    el.closest("a[href*='/status/']") ||
    (article && article.querySelector("a[href*='/status/']"));
  const href = link?.href;
  if (href && /^https?:\/\//i.test(href)) {
    // X wraps each player in a link to `/status/<id>/video/<n>`. That trailing
    // index is the only thing that says *which* of a tweet's media items this
    // is, so it is kept; the engine reads it and downloads that one instead of
    // every clip in the tweet.
    const m = href.match(/^(https?:\/\/[^/]+\/[^/]+\/status\/\d+(?:\/(?:video|photo)\/\d+)?)/);
    const raw = m ? m[1] : href;
    return formatXUrl(raw);
  }
  return formatXUrl(location.href);
}

// Remember what the last right-click landed on, so the background context-menu
// handler can ask this frame for the exact tweet URL when the menu is chosen.
let lastContextUrl = null;
document.addEventListener(
  "contextmenu",
  (e) => {
    let lastContextVideo = e.target?.closest?.("video") || null;
    const node = e.target instanceof Element ? e.target : e.target?.parentElement;
    if (!lastContextVideo && /(^|\.)(instagram\.com|tiktok\.com)$/i.test(location.hostname)) {
      // Reel controls sit beside the video, rather than inside the element.
      for (let el = node; el && el.tagName !== "BODY"; el = el.parentElement) {
        const videos = el.querySelectorAll("video");
        if (videos.length) {
          if (videos.length === 1) lastContextVideo = videos[0];
          break;
        }
      }
    }
    lastContextUrl = isXHost()
      ? xPermalinkFromNode(e.target)
      : (/(^|\.)facebook\.com$/i.test(location.hostname) && node
          ? facebookPermalinkFromNode(node) : (lastContextVideo && videoPermalink(lastContextVideo)) || nearestPermalink(node)) ||
        (isItemPermalink(location.href) ? location.href : null);
  },
  true,
);

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === "zoltraak-show-video-choices") {
    const video = Array.from(document.querySelectorAll("video")).find((v) => matchesPlayer(v, message.playerBinding));
    if (!video || !overlayPrefs.showMediaButton ||
        (message.documentStartedAt && message.documentStartedAt !== performance.timeOrigin)) {
      sendResponse({ ok: false }); return false;
    }
    currentVideo = video;
    currentPlayerGeneration = playerBinding(video).generation;
    renderButton();
    renderVideoChoices(shadow.getElementById("panel"), message.choices, video, message.playerBinding);
    sendResponse({ ok: true });
    return false;
  }
  if (message?.type === "zoltraak-toggle-pill") {
    // The keyboard shortcut opens or folds the Dispatch Board pill in this tab.
    if (window.self !== window.top || !overlayPrefs.showSessionPill) {
      sendResponse({ ok: false });
      return false;
    }
    panelExpanded = !panelExpanded;
    lastRenderSignature = "";
    void pollSession();
    sendResponse({ ok: true });
    return false;
  }
  if (message?.type === "zoltraak-check-video-identity") {
    const video = Array.from(document.querySelectorAll("video")).find((v) =>
      matchesPlayer(v, message.playerBinding));
    sendResponse({ ok: Boolean(video), documentStartedAt: performance.timeOrigin });
    return false;
  }
  if (message?.type === "zoltraak-video-sources") {
    const video = pickVideo();
    (async () => ({
      candidates: video ? (await candidateTargets(video) || await gatherSources(video)) : [location.href],
      videoContext: getVideoContext(video),
      hlsCandidates: hlsFallbackSources(video),
      documentStartedAt: performance.timeOrigin,
    }))().then(sendResponse).catch(() => sendResponse(null));
    return true;
  } else if (message?.type === "zoltraak-x-permalink") {
    sendResponse({ url: lastContextUrl || formatXUrl(location.href) });
  } else if (message?.type === "zoltraak-facebook-attachments") {
    // The helper only loads on facebook.com frames; elsewhere there is
    // nothing to collect, but the caller still needs its answer.
    if (!globalThis.ZoltraakFacebook) {
      sendResponse({ browserDocuments: [], documentStartedAt: performance.timeOrigin });
      return false;
    }
    globalThis.ZoltraakFacebook.collect(message.url)
      .then((browserDocuments) => sendResponse({ browserDocuments, documentStartedAt: performance.timeOrigin }))
      .catch(() => sendResponse({ browserDocuments: [] }));
    return true;
  } else if (message?.type === "zoltraak-post-permalink") {
    if (message.playerBinding) {
      const video = Array.from(document.querySelectorAll("video"))
        .find((video) => matchesPlayer(video, message.playerBinding));
      sendResponse({ url: video ? socialPostPermalink(video) : null, documentStartedAt: performance.timeOrigin });
    } else sendResponse({ url: lastContextUrl || (isItemPermalink(location.href) ? location.href : null) });
  } else if (message?.type === "zoltraak-prepare-refresh") {
    const hadOverlay = OVERLAY_HOST_IDS.some((id) => document.getElementById(id));
    teardownHost();
    teardownSessionHost();
    sendResponse({ ok: true, hadOverlay });
  }
  return false;
});

let lastCaretClickedNode = null;

document.addEventListener(
  "pointerdown",
  (e) => {
    if (!isXHost()) return;
    const el = e.target instanceof Element ? e.target : e.target?.parentElement;
    if (el && !el.closest('[data-testid="Dropdown"]')) {
      lastCaretClickedNode = el;
    }
  },
  true,
);

function tweetUrlFromDropdown(dropdown) {
  if (!dropdown) return null;
  const links = dropdown.querySelectorAll("a[href]");
  for (const link of links) {
    const href = link.getAttribute("href") || "";
    const statusMatch = href.match(/^\/([A-Za-z0-9_]+)\/status\/(\d+)/);
    if (statusMatch) {
      return formatXUrl(`https://x.com/${statusMatch[1]}/status/${statusMatch[2]}`);
    }
    const noteMatch = href.match(/\/noterequest\/(\d+)/);
    if (noteMatch) {
      return formatXUrl(`https://x.com/i/status/${noteMatch[1]}`);
    }
    const idMatch = href.match(/\/status\/(\d+)/);
    if (idMatch) {
      return formatXUrl(`https://x.com/i/status/${idMatch[1]}`);
    }
  }
  return null;
}

function resolveTweetUrlForDropdown(dropdown) {
  const fromDropdown = tweetUrlFromDropdown(dropdown);
  if (fromDropdown) return formatXUrl(fromDropdown);

  if (lastCaretClickedNode) {
    const fromCaret = xPermalinkFromNode(lastCaretClickedNode);
    if (fromCaret && fromCaret !== location.href && !fromCaret.includes("/home")) {
      return formatXUrl(fromCaret);
    }
  }

  if (lastContextUrl && !lastContextUrl.includes("/home")) {
    return formatXUrl(lastContextUrl);
  }

  return formatXUrl(location.href);
}

function dismissXDropdown(dropdown) {
  try {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", keyCode: 27, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", keyCode: 27, bubbles: true }));
  } catch {}

  const mask =
    document.querySelector('[data-testid="mask"]') ||
    document.querySelector('#layers div[tabindex="0"]');
  if (mask) {
    try {
      mask.click();
    } catch {}
  }

  const popover =
    dropdown.closest('[data-testid="popover"]') ||
    dropdown.closest('[role="menu"]') ||
    dropdown.parentElement ||
    dropdown;
  if (popover) {
    try {
      popover.style.display = "none";
      popover.remove();
    } catch {}
  }
}

function injectXDropdownMenu() {
  if (!isXHost()) return;

  const dropdowns = document.querySelectorAll('div[data-testid="Dropdown"]');
  for (const dropdown of dropdowns) {
    if (dropdown.querySelector('[data-zoltraak-item="true"]')) continue;

    const firstItem = dropdown.querySelector('[role="menuitem"]');
    if (!firstItem) continue;

    const newItem = firstItem.cloneNode(true);
    newItem.setAttribute("data-zoltraak-item", "true");

    if (newItem.tagName === "A") {
      newItem.removeAttribute("href");
      newItem.removeAttribute("data-testid");
    }

    const span = newItem.querySelector("span");
    if (span) {
      span.textContent = "Download Video with Zoltraak";
    }

    const svgG = newItem.querySelector("svg g");
    if (svgG) {
      svgG.innerHTML =
        '<path d="M12 15.5l-5.5-5.5 1.41-1.41L11 11.67V3h2v8.67l3.09-3.08 1.41 1.41L12 15.5zM5 18h14v2H5v-2z" fill="currentColor"></path>';
    }

    if (dropdown.firstChild) {
      dropdown.insertBefore(newItem, dropdown.firstChild);
    } else {
      dropdown.appendChild(newItem);
    }

    newItem.addEventListener(
      "click",
      async (e) => {
        e.preventDefault();
        e.stopPropagation();

        const tweetUrl = resolveTweetUrlForDropdown(dropdown);

        dismissXDropdown(dropdown);

        try {
          await chrome.runtime.sendMessage({
            type: "zoltraak-prefill-video",
            url: tweetUrl,
            referrer: location.href,
          });
        } catch {}
      },
      true,
    );
  }
}

window.addEventListener("scroll", () => updateHostPosition(), { passive: true });
window.addEventListener("resize", () => updateHostPosition(), { passive: true });

new MutationObserver(() => {
  refresh();
  injectXDropdownMenu();
}).observe(document.documentElement, {
  childList: true,
  subtree: true,
});

// Relay the poToken / visitorData harvested by the MAIN-world yt-inject.js
// (which cannot use chrome.*) into extension storage, where the background
// worker reads it to arm the engine's YouTube extractor against the bot check.
// Store complete snapshots so tokens cannot be mixed across videos/tabs.
if (isYouTubeHost() && window.self === window.top) {
  window.addEventListener("message", (e) => {
    if (e.source !== window) return;
    const d = e.data;
    if (!d || d.source !== "zoltraak-yt-auth") return;
    if (!d.poToken && !d.playerToken && !d.visitorData) return;
    if (!chrome.runtime?.id) return;
    if (typeof d.videoId !== "string" || !/^[\w-]{11}$/.test(d.videoId)) return;
    // The page sends a complete snapshot. Never merge credentials from
    // different tabs/videos, or renew a stream token on a visitor-only ping.
    const text = (value) => typeof value === "string" && value.length <= 16384 ? value : null;
    chrome.storage.local.set({ ytPot: {
      poToken: text(d.poToken),
      playerToken: text(d.playerToken),
      visitorData: text(d.visitorData),
      videoId: d.videoId,
      ts: Number.isFinite(d.ts) && d.ts > 0 && d.ts <= Date.now() ? d.ts : 0,
    } });
  });
  window.postMessage({ source: "zoltraak-yt-auth-request" }, "*");
}

setupVideoListeners();
watchSpaNavigation();
refresh();
setInterval(refresh, 1000);

// Switching tabs (or minimizing) flips visibility — tear the overlays down
// immediately on the tab being left, and bring them back on return, instead of
// waiting for the next poll tick.
document.addEventListener("visibilitychange", () => {
  refresh();
  void pollSession();
});

/* --- Floating session panel: recent downloads for this browser session ---
 * Collapses to a small pill launcher; expands into a scrollable list with
 * per-item pause/resume/retry/reveal/remove. Built with DOM APIs rather than
 * innerHTML templates because filenames and error strings pass through
 * untrusted (page/video-derived) text. */
const SESSION_ICONS = {
  folder: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"/></svg>`,
  pause: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M8 6v12M16 6v12"/></svg>`,
  play: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M7 5.5v13l11-6.5-11-6.5Z"/></svg>`,
  retry: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 10a8 8 0 0 1 14.5-4.5M20 14a8 8 0 0 1-14.5 4.5"/><path d="M18.5 3.5v3.5H15M5.5 20.5V17H9"/></svg>`,
  trash: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2m-9 0 1 12a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-12"/></svg>`,
  chevron: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>`,
  mark: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12m0 0-4-4m4 4 4-4"/><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2"/></svg>`,
  copy: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h8"/></svg>`,
};

/**
 * Signal aspects, mirroring `app/src/lib/aspect.ts`.
 *
 * Labels stay the engine's own words: a panel earns its clarity from the lamp,
 * not from renaming states the user already knows. `fill` is what an occupied
 * cell is painted — finished work fills neutral so it reads as occupied without
 * asking for attention.
 */
const STATE_META = {
  waiting: { label: "WAITING", color: "var(--caution)", fill: "var(--caution)" },
  downloading: { label: "DOWNLOADING", color: "var(--clear)", fill: "var(--clear)" },
  merging: { label: "MERGING", color: "var(--route)", fill: "var(--route)" },
  paused: { label: "PAUSED", color: "var(--danger)", fill: "var(--danger)" },
  finished: { label: "FINISHED", color: "var(--ink-faint)", fill: "var(--edge-strong)" },
  failed: { label: "FAILED", color: "var(--danger)", fill: "var(--danger)" },
  scheduled: { label: "SCHEDULED", color: "var(--brass)", fill: "var(--brass)" },
  unknown: { label: "ENGINE DETACHED", color: "var(--ink-faint)", fill: "var(--lamp-off)" },
};

/**
 * A transfer's length of track.
 *
 * The cell count is the engine's real connection count and nothing else — no
 * padding a single-connection transfer up to a prettier minimum. A video job
 * has no byte-range resume, so it gets one uncut bar rather than invented
 * cells. Mirrors `app/src/lib/occupancy.ts`.
 */
function buildOccupancy(item, pct, fill) {
  const strip = document.createElement("div");
  strip.className = "row-bar-bg";

  const isStream = !item.connections || item.connections <= 1;
  const cells = isStream ? 1 : Math.min(item.connections, 24);
  const lit = isStream ? 0 : Math.floor((pct / 100) * cells);

  for (let i = 0; i < cells; i += 1) {
    const cell = document.createElement("span");
    if (isStream) {
      // One continuous stream: the fill is a width, not a count of ranges.
      const inner = document.createElement("span");
      inner.style.cssText = `display:block;height:100%;width:${pct}%;background:${fill};border-radius: var(--hw-r-sm, 1px);`;
      cell.style.cssText = "position:relative;overflow:hidden;";
      cell.appendChild(inner);
    } else if (i < lit) {
      cell.style.background = fill;
    }
    strip.appendChild(cell);
  }
  return strip;
}

const ACTIVE_STATES = new Set(["downloading", "merging", "waiting", "scheduled"]);

let sessionHost = null;
let sessionShadow = null;
let panelExpanded = false;
let lastRenderSignature = "";
let repollTimer = null;

let lastPointer = { x: 0, y: 0 };
let focusRestoreTimer = null;
let handleRevealTimer = null;
let isDragging = false;
let dragStartX = 0;
let dragStartY = 0;
let hostStartX = 0;
let hostStartY = 0;
let hasMoved = false;

const SESSION_STYLES = OVERLAY_TOKENS + `
  .wrap {
    pointer-events: auto; display: flex; gap: 10px;
    transform: scale(var(--pill-scale, 1));
    transform-origin: var(--pill-origin, left bottom);
    transition: opacity 160ms;
  }
  .wrap.v-bottom { flex-direction: column-reverse; }
  .wrap.v-top, .wrap.v-middle { flex-direction: column; }
  /* A middle anchor centres the collapsed pill; the panel then opens below it. */
  .wrap.v-middle { margin-top: -17px; }
  .wrap.h-left { align-items: flex-start; }
  .wrap.h-right { align-items: flex-end; }
  .wrap.h-center { align-items: center; }
  .wrap.idle { opacity: var(--pill-idle, 1); }
  .wrap.idle:hover, .wrap.idle:focus-within { opacity: 1; }
  .wrap.frosted .launcher, .wrap.frosted .panel {
    background: color-mix(in srgb, var(--face) 78%, transparent);
    -webkit-backdrop-filter: blur(12px); backdrop-filter: blur(12px);
  }
  .wrap.size-compact .launcher .count { display: none; }
  .wrap.size-compact .launcher { padding: calc(8px * var(--density, 1)) 10px; gap: 7px; }

  /* Hidden while idle: only a thin tab on the anchored edge stays, and
     hovering or focusing it brings the whole pill back. */
  .launcher.handle { padding: 0 0; gap: 0; width: 14px; height: 30px; opacity: 0.7; }
  .launcher.handle > * { opacity: 0; width: 0; min-width: 0; overflow: hidden; }
  .launcher.handle:hover, .launcher.handle:focus-visible {
    width: auto; height: auto; padding: calc(8px * var(--density, 1)) 12px; gap: 9px; opacity: 1;
  }
  .launcher.handle:hover > *, .launcher.handle:focus-visible > * { opacity: 1; width: auto; overflow: visible; }

  .launcher:focus-visible, .icon-btn:focus-visible, .foot-btn:focus-visible, .row:focus-visible {
    outline: 2px solid var(--route); outline-offset: 1px;
  }

  @media (prefers-reduced-motion: reduce) {
    *, *::before, *::after { animation: none !important; transition: none !important; }
  }

  /* The pill is a small machined plate, not a lozenge. It carries its state in
     a lamp and, while anything is running, in a length of track along its
     bottom edge — occupancy the user can read without opening the panel, which
     is what the old breathing glow was reaching for and could not say. */
  .launcher {
    position: relative;
    display: flex; align-items: center; gap: 9px;
    background: var(--face);
    border: 1px solid var(--edge-strong);
    border-radius: var(--hw-r-lg, 3px);
    padding: calc(8px * var(--density, 1)) 12px;
    box-shadow: var(--shadow-plate);
    cursor: grab;
    color: var(--ink);
    overflow: hidden;
  }
  .launcher:active { cursor: grabbing; }
  .launcher:hover { background: var(--panel); }

  .ring {
    position: relative; width: 16px; height: 16px; flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
  }
  .ring .core {
    width: 16px; height: 16px;
    display: flex; align-items: center; justify-content: center;
    color: var(--ink-dim);
  }
  .launcher.has-active .ring .core { color: var(--clear); }
  .ring .core svg { width: 14px; height: 14px; display: block; }

  .launcher .count {
    font-stretch: 78%;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.11em;
    font-size: 11px;
    white-space: nowrap;
  }
  .launcher .badge {
    min-width: 17px; height: 15px; padding: 0 4px; border-radius: var(--hw-r, 2px);
    background: var(--route); color: var(--on-signal);
    font-family: "Martian Mono Variable", ui-monospace, monospace;
    font-stretch: 75%; font-size: 11px; font-variant-numeric: tabular-nums;
    display: flex; align-items: center; justify-content: center;
  }

  /* Aggregate occupancy, flush to the plate's bottom edge. */
  .launcher-track {
    position: absolute;
    left: 0; right: 0; bottom: 0;
    height: 3px;
    display: none;
  }
  .launcher.has-active .launcher-track { display: flex; }

  .panel {
    width: 336px; max-width: calc(100vw - 40px);
    background: var(--face);
    border: 1px solid var(--edge-strong);
    border-radius: var(--hw-r-lg, 3px);
    box-shadow: var(--shadow-lift);
    color: var(--ink);
    overflow: hidden;
  }

  .panel-head {
    display: flex; align-items: center; justify-content: space-between; gap: 10px;
    padding: calc(10px * var(--density, 1)) 12px; border-bottom: 1px solid var(--edge);
    background: var(--recess);
    cursor: grab;
  }
  .panel-head:active { cursor: grabbing; }
  .panel-head .title {
    display: flex; align-items: center; gap: 8px;
    font-stretch: 78%; font-weight: 600; text-transform: uppercase;
    letter-spacing: 0.11em; font-size: 11px; color: var(--ink);
  }
  .panel-head .title .mark { width: 15px; height: 15px; color: var(--ink-dim); display: flex; }
  .panel-head .title .mark svg { width: 15px; height: 15px; display: block; }
  .panel-head .sub {
    font-family: "Martian Mono Variable", ui-monospace, monospace;
    font-stretch: 75%; font-size: 11px; font-variant-numeric: tabular-nums;
    color: var(--ink-faint);
  }

  .icon-btn {
    background: transparent; border: 1px solid transparent;
    border-radius: var(--hw-r, 2px); width: 24px; height: 24px;
    display: flex; align-items: center; justify-content: center;
    color: var(--ink-dim); cursor: pointer; flex-shrink: 0;
    transition: background 120ms, color 120ms, border-color 120ms;
  }
  .icon-btn svg { width: 13px; height: 13px; display: block; }
  .icon-btn:hover { background: var(--panel); border-color: var(--edge); color: var(--ink); }
  .icon-btn:disabled { opacity: 0.4; cursor: default; background: transparent; border-color: transparent; }
  .icon-btn.danger:hover { border-color: var(--danger); color: var(--danger); }
  .icon-btn.accent:hover { border-color: var(--route); color: var(--route); }

  .list { max-height: min(320px, calc(100vh - 170px)); overflow-y: auto; }
  .wrap.v-middle .list { max-height: min(320px, calc(50vh - 110px)); }
  .list::-webkit-scrollbar { width: 10px; }
  .list::-webkit-scrollbar-track { background: var(--recess); }
  .list::-webkit-scrollbar-thumb {
    background: var(--edge); border: 3px solid var(--recess); border-radius: var(--hw-r, 2px);
  }
  .list::-webkit-scrollbar-thumb:hover { background: var(--edge-strong); }

  .empty {
    padding: calc(26px * var(--density, 1)) 14px; text-align: center;
    font-size: 12px; line-height: 1.4; color: var(--ink-dim);
  }

  /* A register row: lamp, description, occupancy, measured values. */
  .row {
    padding: calc(9px * var(--density, 1)) 12px;
    border-bottom: 1px solid var(--edge);
    cursor: pointer;
    transition: background 120ms;
  }
  .row:last-child { border-bottom: none; }
  .row:hover { background: var(--panel); }

  /* One short flash so a completion registers in a half-watched tab. It tints
     the row in the state's own aspect and settles; nothing scales or glows. */
  .row.just-finished { animation: rowFinish 1.8s ease-out; }
  .row.just-failed { animation: rowFail 1.8s ease-out; }
  @keyframes rowFinish {
    0% { background: color-mix(in srgb, var(--clear) 22%, transparent); }
    100% { background: transparent; }
  }
  @keyframes rowFail {
    0% { background: color-mix(in srgb, var(--danger) 22%, transparent); }
    100% { background: transparent; }
  }

  .row-top { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 7px; }
  .row-name {
    font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
    max-width: 190px; color: var(--ink); user-select: text;
  }
  .row-actions { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
  .row-actions .icon-btn { width: 22px; height: 22px; }
  .row-actions .icon-btn svg { width: 11px; height: 11px; display: block; }

  /* Occupancy is cells. The old rounded, glowing bar was the one shape this
     world does not draw. */
  .row-bar-bg {
    display: flex; gap: 2px; height: 4px; margin-bottom: 7px;
  }
  .row-bar-fill { display: none; }

  .row-meta {
    display: flex; align-items: center; justify-content: space-between; gap: 8px;
    font-family: "Martian Mono Variable", ui-monospace, monospace;
    font-stretch: 75%; font-size: 11px; font-variant-numeric: tabular-nums;
    color: var(--ink-dim);
  }
  .row-state {
    font-family: "Archivo Variable", ui-sans-serif, sans-serif;
    font-stretch: 78%; font-weight: 600; text-transform: uppercase;
    letter-spacing: 0.11em; font-size: 11px;
  }

  .panel-foot {
    display: flex; align-items: center; justify-content: space-between; gap: 8px;
    padding: calc(8px * var(--density, 1)) 12px; border-top: 1px solid var(--edge);
    background: var(--recess);
  }
  .foot-btn {
    background: transparent; border: 1px solid transparent; border-radius: var(--hw-r, 2px);
    color: var(--ink-dim);
    font-stretch: 78%; font-weight: 600; text-transform: uppercase;
    letter-spacing: 0.11em; font-size: 11px;
    cursor: pointer; padding: calc(4px * var(--density, 1)) 7px;
    transition: color 120ms, border-color 120ms;
  }
  .foot-btn:hover { color: var(--ink); border-color: var(--edge); }
  .foot-btn:disabled { opacity: 0.4; cursor: default; border-color: transparent; }
  .status-dot {
    display: flex; align-items: center; gap: 7px;
    font-stretch: 78%; font-weight: 600; text-transform: uppercase;
    letter-spacing: 0.11em; font-size: 11px;
    color: var(--ink-dim);
  }
  .status-dot .dot {
    width: 8px; height: 8px; border-radius: var(--hw-r-sm, 1px); flex-shrink: 0;
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.25);
  }

  .toast-container {
    position: fixed;
    top: 20px;
    right: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 2147483647;
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    width: 320px;
    max-width: calc(100vw - 40px);
    background: var(--face);
    border: 1px solid var(--edge-strong);
    border-radius: var(--hw-r-lg, 3px);
    padding: calc(11px * var(--density, 1)) 12px;
    box-shadow: var(--shadow-lift);
    color: var(--ink);
    display: flex;
    align-items: flex-start;
    gap: 10px;
    transition: opacity 200ms, transform 200ms;
  }
  .toast.dismissing { opacity: 0; transform: translateY(-6px); }
  .toast-icon {
    width: 16px;
    height: 16px;
    color: var(--clear);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    margin-top: 2px;
  }
  .toast-icon svg { width: 15px; height: 15px; display: block; }
  .toast-body { flex: 1; min-width: 0; }
  .toast-title {
    font-stretch: 78%; font-weight: 600; text-transform: uppercase;
    letter-spacing: 0.11em; font-size: 11px;
    color: var(--clear); margin-bottom: 3px;
  }
  .toast-name {
    font-size: 12px; color: var(--ink);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .toast-meta {
    font-family: "Martian Mono Variable", ui-monospace, monospace;
    font-stretch: 75%; font-size: 11px; font-variant-numeric: tabular-nums;
    color: var(--ink-dim); margin-top: 3px;
  }
  .toast-close {
    background: transparent; border: 1px solid transparent; border-radius: var(--hw-r, 2px);
    color: var(--ink-faint); cursor: pointer; padding: calc(2px * var(--density, 1)) 2px;
    display: flex; align-items: center;
    transition: color 120ms, border-color 120ms;
  }
  .toast-close svg { display: block; }
  .toast-close:hover { color: var(--ink); border-color: var(--edge); }
`;

function prefersReducedMotion() {
  try { return window.matchMedia("(prefers-reduced-motion: reduce)").matches; } catch { return false; }
}

function effectivePillPlacement() {
  return PillPlacement.resolveForSite(overlayPrefs.pill, overlayPrefs.pillSites, location.hostname);
}

const PILL_EDGE_PAD = 8;
// The strip along a player's bottom edge that its own transport controls use.
const PLAYER_CONTROL_ZONE = 64;

/** How far to lift the pill so it is not drawn over the playing video's control bar. */
function playerControlDodge(rect) {
  const video = currentVideo;
  if (!video?.isConnected) return 0;
  const player = video.getBoundingClientRect();
  if (player.width < 320 || player.height < 180) return 0;
  const zoneTop = player.bottom - PLAYER_CONTROL_ZONE;
  const overlaps = rect.right > player.left && rect.left < player.right
    && rect.bottom > zoneTop && rect.top < player.bottom;
  if (!overlaps) return 0;
  const lift = rect.bottom - zoneTop + PILL_EDGE_PAD;
  return rect.top - lift >= PILL_EDGE_PAD ? -lift : 0;
}

/**
 * Keep the pill fully on screen (a free-dragged spot can be stranded by a
 * smaller window) and, when asked, off the player's controls. Transient: the
 * saved placement is never rewritten from here.
 */
function adjustPillPosition() {
  if (!sessionHost || isDragging) return;
  const wrap = sessionShadow?.querySelector(".wrap");
  if (!wrap) return;
  sessionHost.style.translate = "0px 0px";
  const rect = wrap.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  let dx = 0;
  let dy = 0;
  if (rect.left < PILL_EDGE_PAD) dx = PILL_EDGE_PAD - rect.left;
  else if (rect.right > window.innerWidth - PILL_EDGE_PAD) dx = window.innerWidth - PILL_EDGE_PAD - rect.right;
  if (rect.top < PILL_EDGE_PAD) dy = PILL_EDGE_PAD - rect.top;
  else if (rect.bottom > window.innerHeight - PILL_EDGE_PAD) dy = window.innerHeight - PILL_EDGE_PAD - rect.bottom;
  if (effectivePillPlacement().autoDodge) {
    dy += playerControlDodge({
      left: rect.left + dx, right: rect.right + dx, top: rect.top + dy, bottom: rect.bottom + dy,
    });
  }
  sessionHost.style.translate = `${dx}px ${dy}px`;
}

let lastHostSignature = "";

function applyHostPosition() {
  if (!sessionHost || isDragging) return;
  const placement = effectivePillPlacement();
  // Renders come every poll; only a real change in where or how big the pill
  // is needs the host restyled and re-measured.
  const signature = JSON.stringify([placement, innerWidth, innerHeight]);
  if (signature === lastHostSignature) return;
  lastHostSignature = signature;
  sessionHost.style.transition = prefersReducedMotion()
    ? "none"
    : "top 0.3s cubic-bezier(0.16, 1, 0.3, 1), bottom 0.3s cubic-bezier(0.16, 1, 0.3, 1), left 0.3s cubic-bezier(0.16, 1, 0.3, 1), right 0.3s cubic-bezier(0.16, 1, 0.3, 1)";
  Object.assign(sessionHost.style, PillPlacement.hostStyle(placement));
  sessionHost.style.setProperty("--pill-scale", String(placement.scale / 100));
  sessionHost.style.setProperty("--pill-origin", PillPlacement.transformOrigin(placement.anchor));
  sessionHost.style.setProperty("--pill-idle", String(placement.idleOpacity / 100));
  // Measure once the slide has finished, not while the pill is mid-transition.
  sessionHost.style.translate = "0px 0px";
  scheduleAdjust(prefersReducedMotion() ? 0 : 340, true);
}

let adjustTimer = null;
function scheduleAdjust(delay = 120, replace = false) {
  if (adjustTimer && !replace) return;
  if (adjustTimer) clearTimeout(adjustTimer);
  adjustTimer = setTimeout(() => {
    adjustTimer = null;
    adjustPillPosition();
  }, delay);
}
window.addEventListener("resize", scheduleAdjust);
window.addEventListener("scroll", scheduleAdjust, { passive: true, capture: true });

function ensureSessionHost() {
  if (window.self !== window.top) return;
  if (sessionHost) return;
  sessionHost = document.createElement("div");
  sessionHost.id = "zoltraak-session-host";
  sessionHost.dataset.zoltraakOwner = EXTENSION_OWNER;
  sessionHost.style.cssText = "position:fixed;z-index:2147483647;pointer-events:none;";
  sessionShadow = sessionHost.attachShadow({ mode: "open" });
  globalThis.ZoltraakTheme?.register(sessionHost);
  const styleEl = document.createElement("style");
  styleEl.textContent = SESSION_STYLES;
  sessionShadow.appendChild(styleEl);
  (document.body || document.documentElement).appendChild(sessionHost);
  applyHostPosition();
}

function teardownSessionHost() {
  if (sessionHost) {
    sessionHost.remove();
    sessionHost = null;
    sessionShadow = null;
  }
  lastRenderSignature = "";
  lastHostSignature = "";
}

function playSfx(type) {
  try {
    chrome.storage.local.get({ enableSfx: true }, (res) => {
      if (res.enableSfx === false) return;
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      if (!AudioCtx) return;
      const ctx = new AudioCtx();
      if (ctx.state === "suspended") {
        void ctx.resume();
      }
      const now = ctx.currentTime;

      if (type === "start") {
        const osc = ctx.createOscillator();
        const gain = ctx.createGain();
        osc.type = "sine";
        osc.frequency.setValueAtTime(520, now);
        osc.frequency.exponentialRampToValueAtTime(660, now + 0.12);

        gain.gain.setValueAtTime(0.08, now);
        gain.gain.exponentialRampToValueAtTime(0.001, now + 0.15);

        osc.connect(gain);
        gain.connect(ctx.destination);
        osc.start(now);
        osc.stop(now + 0.15);
      } else if (type === "finish") {
        const osc1 = ctx.createOscillator();
        const osc2 = ctx.createOscillator();
        const osc3 = ctx.createOscillator();
        const gain = ctx.createGain();

        osc1.type = "sine";
        osc2.type = "sine";
        osc3.type = "sine";

        // Ascending 3-tone arpeggio: C6 -> E6 -> G6
        osc1.frequency.setValueAtTime(1046.5, now);
        osc2.frequency.setValueAtTime(1318.5, now + 0.08);
        osc3.frequency.setValueAtTime(1567.98, now + 0.16);

        gain.gain.setValueAtTime(0.12, now);
        gain.gain.exponentialRampToValueAtTime(0.001, now + 0.42);

        osc1.connect(gain);
        osc2.connect(gain);
        osc3.connect(gain);
        gain.connect(ctx.destination);

        osc1.start(now);
        osc1.stop(now + 0.12);
        osc2.start(now + 0.08);
        osc2.stop(now + 0.20);
        osc3.start(now + 0.16);
        osc3.stop(now + 0.42);
      }
    });
  } catch (_) {}
}

function ensureToastContainer() {
  ensureSessionHost();
  if (!sessionShadow) return null;
  let container = sessionShadow.querySelector(".toast-container");
  if (!container) {
    container = document.createElement("div");
    container.className = "toast-container";
    sessionShadow.appendChild(container);
  }
  return container;
}

function showBrowserToast(item) {
  const container = ensureToastContainer();
  if (!container) return;

  const isFailed = item.state === "failed";
  const toast = document.createElement("div");
  toast.className = "toast";

  const icon = document.createElement("div");
  icon.className = "toast-icon";
  setSvgContent(icon, SESSION_ICONS.mark);

  const body = document.createElement("div");
  body.className = "toast-body";

  const title = document.createElement("div");
  title.className = "toast-title";
  title.textContent = isFailed ? "FAILED" : "FINISHED";

  if (isFailed) {
    icon.style.color = "var(--danger)";
    title.style.color = "var(--danger)";
  }

  const name = document.createElement("div");
  name.className = "toast-name";
  name.textContent = decodeHtmlEntities(item.filename || item.title || "Download");

  const meta = document.createElement("div");
  meta.className = "toast-meta";
  const size = item.totalBytes || item.downloadedBytes;
  meta.textContent = size ? formatBytesShort(size) : "Zoltraak";

  body.appendChild(title);
  body.appendChild(name);
  body.appendChild(meta);

  const close = document.createElement("button");
  close.type = "button";
  close.className = "toast-close";
  close.setAttribute("aria-label", "Dismiss");
  close.innerHTML =
    `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" ` +
    `stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">` +
    `<path d="M6 6l12 12M18 6 6 18"/></svg>`;
  close.onclick = () => {
    toast.classList.add("dismissing");
    setTimeout(() => toast.remove(), 300);
  };

  toast.appendChild(icon);
  toast.appendChild(body);
  toast.appendChild(close);

  container.appendChild(toast);

  setTimeout(() => {
    if (toast.parentNode) {
      toast.classList.add("dismissing");
      setTimeout(() => toast.remove(), 300);
    }
  }, 4500);
}

const previousItemStates = new Map();
// When an item last flipped to "finished"/"failed" — buildRow() reads this to
// decide whether to play the one-shot glow flash on that row. Rows are
// recreated from scratch on every render, so the CSS animation naturally
// plays exactly once: the item's row only differs (triggering a re-render at
// all) on the poll where the transition happened.
const recentTransitionAt = new Map();

function trackItemStates(items) {
  if (!items || !items.length) return;
  const liveIds = new Set(items.map((i) => i.id));
  for (const id of recentTransitionAt.keys()) {
    if (!liveIds.has(id)) recentTransitionAt.delete(id);
  }
  for (const item of items) {
    const prevState = previousItemStates.get(item.id);
    if (!prevState && (item.state === "downloading" || item.state === "waiting")) {
      playSfx("start");
    } else if (prevState && prevState !== "finished" && item.state === "finished") {
      playSfx("finish");
      showBrowserToast(item);
      recentTransitionAt.set(item.id, Date.now());
    } else if (prevState && prevState !== "failed" && item.state === "failed") {
      showBrowserToast(item);
      recentTransitionAt.set(item.id, Date.now());
    }
    previousItemStates.set(item.id, item.state);
  }
}

const pointerX = (event) => event.clientX ?? event.touches?.[0]?.clientX ?? 0;
const pointerY = (event) => event.clientY ?? event.touches?.[0]?.clientY ?? 0;

/** Remember where a drag ended: on this site only, or for every site. */
function savePillDrop(drop) {
  if (overlayPrefs.pill.perSite) {
    overlayPrefs.pillSites = { ...overlayPrefs.pillSites, [location.hostname]: drop };
    void chrome.storage.local.set({ pillSites: overlayPrefs.pillSites });
  } else {
    overlayPrefs.pill = { ...overlayPrefs.pill, ...drop };
    void chrome.storage.local.set({ pillPlacement: overlayPrefs.pill });
  }
  lastRenderSignature = "";
  applyHostPosition();
  void pollSession();
}

function setupDrag(dragElement) {
  if (!dragElement) return;

  const onPointerDown = (e) => {
    if (e.button !== undefined && e.button !== 0) return;
    if (e.target.closest("button") && !e.target.closest(".launcher") && !e.target.closest(".panel-head")) return;
    const wrap = sessionShadow?.querySelector(".wrap");
    if (!wrap) return;

    isDragging = true;
    hasMoved = false;
    // The host is about to be moved by hand, so the next apply must restyle it
    // even when the saved placement ends up unchanged.
    lastHostSignature = "";
    dragStartX = pointerX(e);
    dragStartY = pointerY(e);
    lastPointer = { x: dragStartX, y: dragStartY };

    // Measure first: the host's own transforms (centring, dodge) are part of
    // where it visibly is, and are dropped while it follows the pointer.
    const hostRect = sessionHost.getBoundingClientRect();
    const wrapRect = wrap.getBoundingClientRect();
    hostStartX = hostRect.left;
    hostStartY = hostRect.top;

    sessionHost.style.transition = "none";
    sessionHost.style.translate = "0px 0px";
    sessionHost.style.transform = "none";
    sessionHost.style.top = `${hostStartY}px`;
    sessionHost.style.left = `${hostStartX}px`;
    sessionHost.style.bottom = "auto";
    sessionHost.style.right = "auto";

    const onPointerMove = (moveEvt) => {
      if (!isDragging) return;
      lastPointer = { x: pointerX(moveEvt), y: pointerY(moveEvt) };
      const deltaX = lastPointer.x - dragStartX;
      const deltaY = lastPointer.y - dragStartY;
      if (Math.abs(deltaX) > 4 || Math.abs(deltaY) > 4) hasMoved = true;

      // Bounds come from the visible (scaled) pill, not the unscaled host box.
      const minX = 10 - wrapRect.left;
      const maxX = window.innerWidth - 10 - wrapRect.right;
      const minY = 10 - wrapRect.top;
      const maxY = window.innerHeight - 10 - wrapRect.bottom;
      sessionHost.style.left = `${hostStartX + Math.max(minX, Math.min(maxX, deltaX))}px`;
      sessionHost.style.top = `${hostStartY + Math.max(minY, Math.min(maxY, deltaY))}px`;
    };

    const onPointerUp = () => {
      if (!isDragging) return;
      isDragging = false;
      window.removeEventListener("mousemove", onPointerMove);
      window.removeEventListener("mouseup", onPointerUp);
      window.removeEventListener("touchmove", onPointerMove);
      window.removeEventListener("touchend", onPointerUp);

      if (!hasMoved) {
        applyHostPosition();
        return;
      }
      const placement = effectivePillPlacement();
      savePillDrop(PillPlacement.placementFromDrop({
        rect: (sessionShadow?.querySelector(".wrap") || wrap).getBoundingClientRect(),
        pointer: lastPointer,
        viewport: { width: window.innerWidth, height: window.innerHeight },
        margin: placement.margin,
      }));
    };

    window.addEventListener("mousemove", onPointerMove);
    window.addEventListener("mouseup", onPointerUp);
    window.addEventListener("touchmove", onPointerMove, { passive: false });
    window.addEventListener("touchend", onPointerUp);
  };

  dragElement.addEventListener("mousedown", onPointerDown);
  dragElement.addEventListener("touchstart", onPointerDown, { passive: true });
}

function formatBytesShort(bytes) {
  if (!bytes) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.min(sizes.length - 1, Math.floor(Math.log(bytes) / Math.log(k)));
  return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
}

function itemProgressPct(item) {
  const total = item.totalBytes || 0;
  const downloaded = item.downloadedBytes || 0;
  if (item.state === "finished") return 100;
  if (total > 0) return Math.min(100, Math.round((downloaded / total) * 100));
  return 0;
}

/** A small icon-only button; the SVG markup is parsed via DOMParser text/html
 * so SVG elements acquire the SVG namespace and render correctly. */
function setSvgContent(element, svgString) {
  element.textContent = "";
  try {
    const doc = new DOMParser().parseFromString(svgString, "text/html");
    const svg = doc.body.querySelector("svg");
    if (svg) {
      element.appendChild(document.importNode(svg, true));
    }
  } catch (_) {}
}

function iconButton(action, iconSvg, title, extraClass) {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = `icon-btn${extraClass ? ` ${extraClass}` : ""}`;
  btn.dataset.action = action;
  btn.title = title;
  setSvgContent(btn, iconSvg);
  return btn;
}

function buildRowActions(item) {
  const wrap = document.createElement("div");
  wrap.className = "row-actions";
  const isDownload = item.kind === "download";

  if (isDownload && (item.state === "downloading" || item.state === "waiting" || item.state === "scheduled")) {
    wrap.appendChild(iconButton("pause", SESSION_ICONS.pause, "Pause"));
  }
  if (isDownload && item.state === "paused") {
    wrap.appendChild(iconButton("resume", SESSION_ICONS.play, "Resume", "accent"));
  }
  if (isDownload && item.state === "failed" && item.url) {
    wrap.appendChild(iconButton("retry", SESSION_ICONS.retry, "Retry", "accent"));
  }
  if (item.url) {
    wrap.appendChild(iconButton("copy", SESSION_ICONS.copy, "Copy link"));
  }
  if (item.state !== "unknown") {
    wrap.appendChild(iconButton("reveal", SESSION_ICONS.folder, "Open folder"));
  }
  wrap.appendChild(iconButton("remove", SESSION_ICONS.trash, "Remove", "danger"));
  return wrap;
}

function decodeHtmlEntities(str) {
  if (!str) return "";
  try {
    const doc = new DOMParser().parseFromString(str, "text/html");
    return doc.body.textContent || str;
  } catch {
    return str;
  }
}

function buildRow(item, index) {
  const meta = STATE_META[item.state] || STATE_META.unknown;
  const pct = itemProgressPct(item);
  const total = item.totalBytes || 0;
  const downloaded = item.downloadedBytes || 0;
  const sizeText =
    item.state === "finished"
      ? formatBytesShort(total || downloaded)
      : total > 0
        ? `${formatBytesShort(downloaded)} / ${formatBytesShort(total)}`
        : formatBytesShort(downloaded);
  const speed = item.bytesPerSecond ? ` · ${formatBytesShort(item.bytesPerSecond)}/s` : "";
  const rightMetaText = item.state === "failed" ? decodeHtmlEntities(item.error || "Failed") : `${sizeText}${item.state === "downloading" ? speed : ""}`;

  const transitionedAt = recentTransitionAt.get(item.id);
  const justTransitioned = transitionedAt && Date.now() - transitionedAt < 2200;

  const row = document.createElement("div");
  row.className = justTransitioned
    ? `row ${item.state === "failed" ? "just-failed" : "just-finished"}`
    : "row";
  row.style.animationDelay = `${Math.min(index, 8) * 30}ms`;
  row.dataset.id = item.id;
  row.title = item.state === "finished" ? "Double-click or press Enter to open file" : "Double-click or press Enter to show in folder";
  // Arrow keys move between rows, Enter opens, Delete removes.
  row.tabIndex = 0;
  row.addEventListener("keydown", (e) => {
    if (e.target !== row) return;
    if (e.key === "Enter") {
      e.preventDefault();
      void chrome.runtime.sendMessage({
        type: item.state === "finished" ? "zoltraak-open" : "zoltraak-reveal",
        id: item.id,
        path: item.path || item.directory,
      });
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      row.nextElementSibling?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      row.previousElementSibling?.focus();
    } else if (e.key === "Delete") {
      e.preventDefault();
      void chrome.runtime.sendMessage({ type: "zoltraak-remove", id: item.id }).then(scheduleImmediateRepoll);
    }
  });

  row.addEventListener("dblclick", async (e) => {
    if (e.target.closest("button") || e.target.closest(".icon-btn")) return;
    e.preventDefault();
    e.stopPropagation();

    try {
      window.getSelection()?.removeAllRanges();
    } catch (_) {}

    row.classList.remove("opening");
    void row.offsetWidth;
    row.classList.add("opening");
    setTimeout(() => row.classList.remove("opening"), 400);

    if (item.state === "finished") {
      await chrome.runtime.sendMessage({
        type: "zoltraak-open",
        id: item.id,
        path: item.path || item.directory,
      });
    } else {
      await chrome.runtime.sendMessage({
        type: "zoltraak-reveal",
        id: item.id,
        path: item.path || item.directory,
      });
    }
  });

  const top = document.createElement("div");
  top.className = "row-top";
  const name = document.createElement("div");
  name.className = "row-name";
  name.textContent = decodeHtmlEntities(item.filename || item.title || "Download");
  name.title = name.textContent;
  top.appendChild(name);
  top.appendChild(buildRowActions(item));
  row.appendChild(top);

  if (item.state !== "unknown") {
    row.appendChild(
      buildOccupancy(item, item.state === "finished" ? 100 : pct, meta.fill),
    );
  }

  const metaRow = document.createElement("div");
  metaRow.className = "row-meta";
  const stateSpan = document.createElement("span");
  stateSpan.className = "row-state";
  stateSpan.style.color = meta.color;
  stateSpan.textContent = item.state === "downloading" ? `${pct}%` : meta.label;
  const rightSpan = document.createElement("span");
  rightSpan.textContent = rightMetaText;
  metaRow.appendChild(stateSpan);
  metaRow.appendChild(rightSpan);
  row.appendChild(metaRow);

  return row;
}

let lastFinishedTime = 0;

function trackItemStates(items) {
  if (!items || !items.length) return;
  const liveIds = new Set(items.map((i) => i.id));
  for (const id of recentTransitionAt.keys()) {
    if (!liveIds.has(id)) recentTransitionAt.delete(id);
  }
  for (const item of items) {
    const prevState = previousItemStates.get(item.id);
    if (!prevState && (item.state === "downloading" || item.state === "waiting")) {
      playSfx("start");
    } else if (prevState && prevState !== "finished" && item.state === "finished") {
      playSfx("finish");
      showBrowserToast(item);
      recentTransitionAt.set(item.id, Date.now());
      lastFinishedTime = Date.now();
    } else if (prevState && prevState !== "failed" && item.state === "failed") {
      showBrowserToast(item);
      recentTransitionAt.set(item.id, Date.now());
    }
    previousItemStates.set(item.id, item.state);
  }
}

function buildLauncher(activeCount, itemCount, aggPct, paired, handle) {
  const btn = document.createElement("button");
  btn.type = "button";
  const isRecentlyFinished = Date.now() - lastFinishedTime < 3000;
  const classes = ["launcher"];
  if (activeCount > 0 && paired) classes.push("has-active");
  if (isRecentlyFinished) classes.push("just-finished");
  if (handle) classes.push("handle");
  btn.className = classes.join(" ");
  btn.dataset.action = "toggle";
  btn.title = paired ? "Zoltraak downloads (Alt+Shift+P)" : "Zoltraak — app not running";
  btn.setAttribute("aria-label", btn.title);

  const lamp = document.createElement("span");
  lamp.className = !paired ? "lamp" : activeCount > 0 ? "lamp clear" : "lamp";
  btn.appendChild(lamp);

  const ring = document.createElement("span");
  ring.className = "ring";
  const core = document.createElement("span");
  core.className = "core";
  setSvgContent(core, SESSION_ICONS.mark);
  ring.appendChild(core);

  const count = document.createElement("span");
  count.className = "count";
  count.textContent = !paired
    ? "engine detached"
    : activeCount > 0
      ? (overlayPrefs.pill.size === "detailed" && aggPct > 0 ? `${activeCount} running · ${aggPct}%` : `${activeCount} running`)
      : "Zoltraak";

  btn.appendChild(ring);
  btn.appendChild(count);

  if (paired && itemCount > 0) {
    const badge = document.createElement("span");
    badge.className = "badge";
    badge.textContent = String(itemCount);
    btn.appendChild(badge);
  }

  // Aggregate occupancy flush to the plate's bottom edge: progress readable
  // without opening the panel, and without a glow claiming to be information.
  if (paired && activeCount > 0) {
    const track = document.createElement("div");
    track.className = "launcher-track occupancy";
    const cells = 12;
    const lit = Math.floor((aggPct / 100) * cells);
    for (let i = 0; i < cells; i += 1) {
      const cell = document.createElement("span");
      if (i < lit) cell.className = "lit";
      track.appendChild(cell);
    }
    btn.appendChild(track);
  }
  return btn;
}

function buildPanel(items, paired) {
  const panel = document.createElement("div");
  panel.className = "panel";

  const head = document.createElement("div");
  head.className = "panel-head";
  const title = document.createElement("div");
  title.className = "title";
  const mark = document.createElement("span");
  mark.className = "mark";
  setSvgContent(mark, SESSION_ICONS.mark);
  title.appendChild(mark);
  title.appendChild(document.createTextNode("Session downloads"));
  const sub = document.createElement("span");
  sub.className = "sub";
  sub.textContent = ` · ${items.length}`;
  title.appendChild(sub);
  head.appendChild(title);
  head.appendChild(iconButton("toggle", SESSION_ICONS.chevron, "Collapse"));
  panel.appendChild(head);

  const list = document.createElement("div");
  list.className = "list";
  if (items.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = paired
      ? "Nothing downloaded yet this session."
      : "Zoltraak isn't running. Start the app and this list fills in on its own.";
    list.appendChild(empty);
  } else {
    items.forEach((item, i) => list.appendChild(buildRow(item, i)));
  }
  panel.appendChild(list);

  const foot = document.createElement("div");
  foot.className = "panel-foot";
  const statusDot = document.createElement("span");
  statusDot.className = "status-dot";
  const dot = document.createElement("span");
  // A lamp, not a coloured dot: unlit is a housing, and the engine being away
  // is an absence of light rather than a fault the user caused.
  dot.className = paired ? "dot lamp clear" : "dot lamp";
  statusDot.appendChild(dot);
  statusDot.appendChild(document.createTextNode(paired ? "line open" : "engine detached"));
  foot.appendChild(statusDot);

  const footButton = (action, label, disabled = false) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "foot-btn";
    button.dataset.action = action;
    button.textContent = label;
    button.disabled = disabled;
    return button;
  };
  const footActions = document.createElement("div");
  footActions.style.cssText = "display:flex;gap:4px;";
  if (!paired) {
    footActions.appendChild(footButton("retry-now", "Retry"));
  } else {
    const downloads = items.filter((i) => i.kind === "download");
    const pausable = downloads.some((i) => i.state === "downloading" || i.state === "waiting" || i.state === "scheduled");
    const resumable = downloads.some((i) => i.state === "paused");
    if (pausable || resumable) {
      footActions.appendChild(footButton(pausable ? "pause-all" : "resume-all", pausable ? "Pause all" : "Resume all"));
    }
    footActions.appendChild(
      footButton("clear-finished", "Clear finished", !items.some((i) => i.state === "finished" || i.state === "failed")),
    );
  }
  foot.appendChild(footActions);

  panel.appendChild(foot);
  return panel;
}

// How long the pill stays up after the last download finishes before it
// folds into its edge tab (when "hide when idle" is on).
const HANDLE_GRACE_MS = 6000;
const RESTORABLE_ACTIONS = new Set(["toggle", "clear-finished", "pause-all", "resume-all", "retry-now"]);

/** Rows are rebuilt on every progress tick, so keyboard focus has to be carried across. */
function captureSessionFocus() {
  const active = sessionShadow?.activeElement;
  if (!active) return null;
  const row = active.closest?.(".row");
  if (row) return { kind: "row", id: row.dataset.id };
  const action = active.dataset?.action;
  return action && RESTORABLE_ACTIONS.has(action) ? { kind: "action", action } : null;
}

function restoreSessionFocus(saved) {
  if (!saved || !sessionShadow) return;
  const target = saved.kind === "row"
    ? Array.from(sessionShadow.querySelectorAll(".row")).find((row) => row.dataset.id === saved.id)
    : sessionShadow.querySelector(`[data-action="${saved.action}"]`);
  target?.focus({ preventScroll: true });
}

function renderSessionPanel(items, paired) {
  // A drag holds the pill's elements; the next poll after the drop redraws it.
  if (isDragging) return;
  const placement = effectivePillPlacement();
  const activeCount = items.filter((i) => ACTIVE_STATES.has(i.state)).length;
  const idle = !paired || activeCount === 0;
  const handle = placement.hideWhenIdle && !panelExpanded
    && (!paired || (activeCount === 0 && Date.now() - lastFinishedTime > HANDLE_GRACE_MS));

  const signature = JSON.stringify({ panelExpanded, items, paired, placement, handle });
  if (signature === lastRenderSignature) return;
  lastRenderSignature = signature;

  ensureSessionHost();
  const activeWithTotal = items.filter((i) => ACTIVE_STATES.has(i.state) && (i.totalBytes || 0) > 0);
  const aggPct = activeWithTotal.length
    ? Math.round(
        (activeWithTotal.reduce((s, i) => s + (i.downloadedBytes || 0), 0) /
          activeWithTotal.reduce((s, i) => s + (i.totalBytes || 0), 0)) * 100
      )
    : 0;

  const { v, h } = PillPlacement.layout(placement.anchor);
  const wrap = document.createElement("div");
  wrap.className = [
    "wrap", `v-${v}`, `h-${h}`, `size-${placement.size}`,
    idle && !panelExpanded ? "idle" : "",
    placement.frosted ? "frosted" : "",
  ].filter(Boolean).join(" ");

  const childEl = panelExpanded
    ? buildPanel(items, paired)
    : buildLauncher(activeCount, items.length, aggPct, paired, handle);
  wrap.appendChild(childEl);
  wrap.addEventListener("click", (e) => onSessionClick(e, items));

  const dragTarget = panelExpanded ? childEl.querySelector(".panel-head") : childEl;
  setupDrag(dragTarget);

  const savedFocus = captureSessionFocus();
  // Only the previous pill is replaced; a toast that is still showing stays.
  sessionShadow.querySelectorAll(".wrap").forEach((node) => node.remove());
  sessionShadow.appendChild(wrap);
  restoreSessionFocus(savedFocus);
  applyHostPosition();
}

/** Copy a download's source link, falling back for pages that block the async clipboard. */
async function copyLink(url, button) {
  let copied = false;
  try {
    await navigator.clipboard.writeText(url);
    copied = true;
  } catch {
    const field = document.createElement("textarea");
    field.value = url;
    field.style.cssText = "position:fixed;opacity:0;pointer-events:none;";
    (document.body || document.documentElement).appendChild(field);
    field.select();
    try { copied = document.execCommand("copy"); } catch { copied = false; }
    field.remove();
  }
  const original = button.title;
  button.title = copied ? "Copied" : "Could not copy";
  setTimeout(() => { button.title = original; }, 1500);
}

function scheduleImmediateRepoll() {
  if (repollTimer) clearTimeout(repollTimer);
  repollTimer = setTimeout(() => void pollSession(), 250);
}

async function onSessionClick(e, items) {
  if (hasMoved) return;
  const target = e.target.closest("[data-action]");
  if (!target || target.disabled) return;
  const action = target.dataset.action;

  if (action === "toggle") {
    panelExpanded = !panelExpanded;
    lastRenderSignature = "";
    void pollSession();
    return;
  }
  if (action === "clear-finished") {
    const ids = items.filter((i) => i.state === "finished" || i.state === "failed").map((i) => i.id);
    if (!ids.length) return;
    await chrome.runtime.sendMessage({ type: "zoltraak-clear-session", ids });
    scheduleImmediateRepoll();
    return;
  }

  if (action === "retry-now") {
    lastRenderSignature = "";
    void pollSession();
    return;
  }
  if (action === "pause-all" || action === "resume-all") {
    const pausing = action === "pause-all";
    const wanted = pausing ? new Set(["downloading", "waiting", "scheduled"]) : new Set(["paused"]);
    await Promise.all(
      items
        .filter((i) => i.kind === "download" && wanted.has(i.state))
        .map((i) => chrome.runtime.sendMessage({ type: "zoltraak-set-state", id: i.id, action: pausing ? "pause" : "resume" })),
    );
    scheduleImmediateRepoll();
    return;
  }

  const row = target.closest(".row");
  const id = row?.dataset.id;
  if (!id) return;
  const item = items.find((i) => i.id === id);
  if (action === "copy") {
    if (item?.url) void copyLink(item.url, target);
    return;
  }
  target.disabled = true;

  try {
    if (action === "pause") await chrome.runtime.sendMessage({ type: "zoltraak-set-state", id, action: "pause" });
    if (action === "resume") await chrome.runtime.sendMessage({ type: "zoltraak-set-state", id, action: "resume" });
    if (action === "reveal") await chrome.runtime.sendMessage({ type: "zoltraak-reveal", id, path: item?.path || item?.directory });
    if (action === "remove") await chrome.runtime.sendMessage({ type: "zoltraak-remove", id });
    if (action === "retry" && item) {
      await chrome.runtime.sendMessage({ type: "zoltraak-retry", oldId: id, url: item.url, filename: item.filename });
    }
  } finally {
    scheduleImmediateRepoll();
  }
}

let pollTimer = null;

function scheduleNextPoll(items, offline = false) {
  if (pollTimer) clearTimeout(pollTimer);
  const hasActive = items && items.some((i) => ACTIVE_STATES.has(i.state));
  // Tight enough while something is actually moving that progress reads as
  // live, without hammering the local app the rest of the time. An app that
  // is not running is checked more slowly still.
  const delay = hasActive ? 200 : offline ? 3000 : 900;
  pollTimer = setTimeout(() => void pollSession(), delay);
}

function stopPolling() {
  if (pollTimer) clearTimeout(pollTimer);
  pollTimer = null;
}

async function pollSession() {
  // The pill disappears for exactly two reasons: this frame/tab isn't allowed
  // to show overlays at all (a sub-frame, or the tab is backgrounded/hidden),
  // or the user switched it off in the popup. Every other state — offline,
  // paired but idle, mid-download — renders the pill so it's always on screen
  // while the tab is actually visible.
  // Nothing is drawn until the saved placement is known; loading it polls again.
  if (!pillPrefsLoaded) return;
  if (!overlaysAllowed() || !overlayPrefs.showSessionPill) {
    teardownSessionHost();
    panelExpanded = false;
    // No timer while hidden or switched off: becoming visible or flipping the
    // switch wakes the loop, so a background tab costs nothing.
    stopPolling();
    return;
  }
  if (!chrome.runtime?.id) return;
  try {
    const res = await chrome.runtime.sendMessage({ type: "zoltraak-get-session" });
    if (!res?.ok || res.paired === false) {
      isAppRunning = false;
      lastHealthCheck = Date.now();
      renderSessionPanel([], false);
      scheduleNextPoll([], true);
      return;
    }

    isAppRunning = true;
    lastHealthCheck = Date.now();

    const items = res.items || [];
    trackItemStates(items);
    renderSessionPanel(items, true);
    scheduleNextPoll(items);
  } catch (err) {
    if (!chrome.runtime?.id || err?.message?.includes("context invalidated")) {
      teardownSessionHost();
      return;
    }
    isAppRunning = false;
    renderSessionPanel([], false);
    scheduleNextPoll([], true);
  }
}

document.addEventListener("visibilitychange", () => {
  if (window.self === window.top && document.visibilityState === "visible") void pollSession();
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    if (panelExpanded) {
      panelExpanded = false;
      lastRenderSignature = "";
      void pollSession();
    }
    if (host && shadow) {
      const panel = shadow.getElementById("panel");
      if (panel && panel.classList.contains("open")) {
        panel.classList.remove("open");
        panel.textContent = "";
        if (statusHideTimer) {
          clearTimeout(statusHideTimer);
          statusHideTimer = null;
        }
      }
    }
  }
});

function setupClickOutsideListeners() {
  document.addEventListener(
    "pointerdown",
    (e) => {
      const path = typeof e.composedPath === "function" ? e.composedPath() : [];

      // 1. Video overlay choices dropdown card
      if (host && shadow) {
        const panel = shadow.getElementById("panel");
        if (panel && panel.classList.contains("open")) {
          if (!path.includes(host)) {
            panel.classList.remove("open");
            panel.textContent = "";
            if (statusHideTimer) {
              clearTimeout(statusHideTimer);
              statusHideTimer = null;
            }
          }
        }
      }

      // 2. Session download pill popup
      if (sessionHost && panelExpanded && !isDragging) {
        if (!path.includes(sessionHost)) {
          panelExpanded = false;
          lastRenderSignature = "";
          void pollSession();
        }
      }
    },
    true,
  );

  window.addEventListener("blur", () => {
    if (host && shadow) {
      const panel = shadow.getElementById("panel");
      if (panel && panel.classList.contains("open")) {
        panel.classList.remove("open");
        panel.textContent = "";
        if (statusHideTimer) {
          clearTimeout(statusHideTimer);
          statusHideTimer = null;
        }
      }
    }
    if (sessionHost && panelExpanded && !isDragging) {
      panelExpanded = false;
      lastRenderSignature = "";
      void pollSession();
    }
  });
}

setupClickOutsideListeners();

if (window.self === window.top) {
  void pollSession();
}
