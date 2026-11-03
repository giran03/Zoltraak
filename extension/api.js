// Talks to the Zoltraak desktop app's local API. Nothing here is engine logic —
// this only shapes requests and reads `chrome.storage` for the token the user
// pastes in from the app's pairing card.

import "./video-identity.js";

export const ENGINE_ORIGIN = "http://127.0.0.1:1450";
const NATIVE_HOST = "app.zoltraak.host";

/**
 * The token the app baked into this file when it wrote the unpacked Chromium
 * build. Empty everywhere else — the signed Zen/Firefox XPI is immutable and
 * pairs over native messaging instead.
 *
 * The app rewrites the line below by exact string match (see
 * `write_extension_files` in `browser.rs`), so its shape is load-bearing: one
 * `const`, one double-quoted empty string, on one line.
 */
const BAKED_TOKEN = "";
const BAKED_BROWSER_ID = "";
const CLIENT_ID_KEY = "zoltraakClientId";
const BROWSER_ID_KEY = "zoltraakBrowserId";
const COOKIE_SOURCE_KEY = "zoltraakCookieSource";
const COOKIE_SOURCE_MAX_AGE_MS = 65_000;
const STATUS_CURSOR_KEY = "zoltraakStatusCursor";

function newClientId() {
  if (crypto.randomUUID) return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export async function extensionClientId() {
  const stored = await chrome.storage.local.get(CLIENT_ID_KEY);
  if (stored[CLIENT_ID_KEY]) return stored[CLIENT_ID_KEY];
  const clientId = newClientId();
  await chrome.storage.local.set({ [CLIENT_ID_KEY]: clientId });
  return clientId;
}

function detectedBrowserId() {
  const brands = navigator.userAgentData?.brands?.map((item) => item.brand.toLowerCase()) ?? [];
  const ua = navigator.userAgent || "";
  if (navigator.brave) return "brave";
  if (brands.some((brand) => brand.includes("edge")) || /Edg\//.test(ua)) return "edge";
  if (brands.some((brand) => brand.includes("opera")) || /OPR\//.test(ua)) return "opera";
  if (/Vivaldi\//.test(ua)) return "vivaldi";
  if (brands.some((brand) => brand.includes("chrome"))) return "chrome";
  if (/Firefox\//.test(ua)) return "firefox";
  return /Chrom(?:e|ium)\//.test(ua) ? "chromium" : "browser";
}

export async function extensionBrowserId() {
  if (BAKED_BROWSER_ID) return BAKED_BROWSER_ID;
  const stored = await chrome.storage.local.get(BROWSER_ID_KEY);
  if (stored[BROWSER_ID_KEY]) return stored[BROWSER_ID_KEY];
  try {
    const reply = await chrome.runtime?.sendNativeMessage?.(NATIVE_HOST, { type: "identity" });
    if (reply?.browserId) {
      await chrome.storage.local.set({ [BROWSER_ID_KEY]: reply.browserId });
      return reply.browserId;
    }
  } catch {
    // Native messaging is optional for unpacked Chromium installs.
  }
  const browserId = detectedBrowserId();
  await chrome.storage.local.set({ [BROWSER_ID_KEY]: browserId });
  return browserId;
}

export async function extensionIdentityHeaders() {
  const [clientId, browserId] = await Promise.all([extensionClientId(), extensionBrowserId()]);
  return {
    "X-Zoltraak-Client-Id": clientId,
    "X-Zoltraak-Browser-Id": browserId,
    "X-Zoltraak-Extension-Version": chrome.runtime.getManifest?.().version ?? "",
  };
}

export async function setCookieSourceState(active, revision) {
  const before = await chrome.storage.local.get(COOKIE_SOURCE_KEY);
  const previous = before[COOKIE_SOURCE_KEY];
  const next = { active: Boolean(active), revision: Number(revision) || 0, confirmedAt: Date.now() };
  await chrome.storage.local.set({ [COOKIE_SOURCE_KEY]: next });
  return previous?.active !== next.active || previous?.revision !== next.revision;
}

export async function canUseBrowserCookies() {
  const stored = await chrome.storage.local.get(COOKIE_SOURCE_KEY);
  const state = stored[COOKIE_SOURCE_KEY];
  return Boolean(state?.active && Date.now() - Number(state.confirmedAt || 0) <= COOKIE_SOURCE_MAX_AGE_MS);
}

export async function extensionStatusCursor() {
  const stored = await chrome.storage.local.get(STATUS_CURSOR_KEY);
  const cursor = stored[STATUS_CURSOR_KEY];
  return {
    reloadVersion: Number(cursor?.reloadVersion) || 0,
    sourceRevision: Number(cursor?.sourceRevision) || 0,
  };
}

export async function setExtensionStatusCursor(reloadVersion, sourceRevision) {
  await chrome.storage.local.set({
    [STATUS_CURSOR_KEY]: {
      reloadVersion: Number(reloadVersion) || 0,
      sourceRevision: Number(sourceRevision) || 0,
    },
  });
}

/** What is actually in storage, with no fallback. */
async function storedToken() {
  const { token } = await chrome.storage.local.get("token");
  return token ?? "";
}

export async function getToken() {
  return (await storedToken()) || BAKED_TOKEN;
}

export async function setToken(token) {
  await chrome.storage.local.set({ token });
}

/**
 * Ask the Zoltraak app for the API token over native messaging.
 *
 * The Zen/Firefox build ships as an immutable *signed* XPI, so the token
 * cannot be baked in the way the Chromium dev-load build can — instead the
 * browser launches the app's registered native host, which reads the token
 * from the same store the app uses and hands it back. No copy-paste, no
 * pairing card. On Chromium (or wherever no host is registered) the call
 * throws and the caller falls back to the injected/stored token.
 *
 * @returns {Promise<boolean>} whether a token was obtained and stored.
 */
async function pairViaNativeHost() {
  try {
    if (!chrome.runtime?.sendNativeMessage) return false;
    const reply = await chrome.runtime.sendNativeMessage(NATIVE_HOST, { type: "pair" });
    if (reply && reply.token) {
      await setToken(reply.token);
      if (reply.browserId) {
        await chrome.storage.local.set({ [BROWSER_ID_KEY]: reply.browserId });
      }
      return true;
    }
  } catch {
    // No host registered — expected on Chromium dev-load. Fall through.
  }
  return false;
}

/**
 * Ask the Zoltraak engine for the API token over HTTP.
 *
 * The engine issues the token only during a pairing window, which opens when the user
 * clicks "Reload extension" (or Install / Repair) in the app's settings and closes after
 * one pairing. Knowing the extension's public ID is not enough; native messaging remains
 * the path that needs no click.
 *
 * @returns {Promise<boolean>} whether a token was obtained and stored.
 */
async function pairViaHttp() {
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 2500);
    const response = await fetch(`${ENGINE_ORIGIN}/pair`, {
      method: "POST",
      signal: controller.signal,
    });
    clearTimeout(timer);
    if (response.ok) {
      const data = await response.json();
      if (data && data.token) {
        await setToken(data.token);
        return true;
      }
    }
  } catch {
    // Engine unreachable or /pair rejected. Fall through.
  }
  return false;
}

/**
 * The token we should send, pairing on demand if we don't already have a
 * working one.
 *
 * @returns {Promise<string>} a token, or "" if pairing isn't possible.
 */
async function requireToken() {
  const token = await getToken();
  if (token) return token;
  if (await pairViaHttp()) return getToken();
  if (await pairViaNativeHost()) return getToken();
  return "";
}

/**
 * Work through every way this build can get a token the app accepts, and keep
 * the first that works.
 *
 * Ladder:
 * 1. Stored token (skipped if forceFresh is true)
 * 2. HTTP pairing via /pair (fastest, direct with running engine)
 * 3. Native messaging host (talks directly to the app executable on disk)
 * 4. Baked token in api.js (updated on disk when extension files are written)
 *
 * @param {{ forceFresh?: boolean }} [options]
 * @returns {Promise<{paired: boolean, reason: string | null}>}
 */
export async function reconnect({ forceFresh = false } = {}) {
  const previous = await storedToken();

  if (!forceFresh && previous) {
    if (await verify(previous)) {
      return { paired: true, reason: null };
    }
  }

  // Try HTTP pairing first
  if (await pairViaHttp()) {
    const token = await storedToken();
    if (await verify(token)) {
      return { paired: true, reason: null };
    }
  }

  // Try native messaging
  if (await pairViaNativeHost()) {
    const token = await storedToken();
    if (await verify(token)) {
      return { paired: true, reason: null };
    }
  }

  // Try baked-in token
  if (BAKED_TOKEN && (forceFresh || BAKED_TOKEN !== previous)) {
    if (await verify(BAKED_TOKEN)) {
      await setToken(BAKED_TOKEN);
      return { paired: true, reason: null };
    }
  }

  if (!(await health())) {
    return { paired: false, reason: "The Zoltraak app is not running." };
  }

  return {
    paired: false,
    reason: previous
      ? "The app rejected this token. Click 'Reload extension' in Zoltraak settings or copy a fresh token."
      : "Not paired yet. Copy the token from the app's Extension card.",
  };
}

/**
 * Proactively ensure we hold a token the app actually accepts — used on
 * startup, periodic background alarms, and popup-open.
 * If token is rejected or missing, automatically triggers re-pairing.
 */
export async function ensurePaired() {
  const token = await storedToken();
  if (token && (await verify(token))) {
    return true;
  }
  const result = await reconnect({ forceFresh: true });
  return result.paired;
}

/** Unauthenticated: true only tells us a process is bound to the port. */
export async function health() {
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 2000);
    const response = await fetch(`${ENGINE_ORIGIN}/health`, { method: "GET", signal: controller.signal });
    clearTimeout(timer);
    return response.ok;
  } catch {
    return false;
  }
}

/**
 * Whether the candidate token is actually accepted by the running engine.
 * Tests without auto-repair so caller knows if this exact token works.
 */
export async function verify(token) {
  if (!token) return false;
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 2500);
    const response = await fetch(`${ENGINE_ORIGIN}/downloads`, {
      headers: { Authorization: `Bearer ${token}` },
      signal: controller.signal,
    });
    clearTimeout(timer);
    return response.ok;
  } catch {
    return false;
  }
}

let repairPromise = null;

/**
 * Deduplicated auto-repair when a token mismatch is detected.
 * Coordinates concurrent calls so only one re-pairing handshake runs at a time.
 *
 * @param {string} [badToken]
 * @returns {Promise<string | null>} fresh working token, or null if repair failed.
 */
export async function autoRepairToken(badToken = "") {
  if (repairPromise) return repairPromise;

  repairPromise = (async () => {
    try {
      // If another concurrent call already updated the token to something working, use it
      const current = await storedToken();
      if (current && current !== badToken && (await verify(current))) {
        return current;
      }

      const result = await reconnect({ forceFresh: true });
      if (result.paired) {
        const fresh = await getToken();
        if (fresh && (await verify(fresh))) {
          return fresh;
        }
      }
      return null;
    } finally {
      repairPromise = null;
    }
  })();

  return repairPromise;
}

/**
 * Authenticated fetch to the engine with automatic token mismatch detection and self-healing.
 * If the engine returns 401 Unauthorized, automatically re-pairs and retries once.
 *
 * @param {string} path
 * @param {RequestInit} [init]
 * @returns {Promise<Response | null>} response, or null if unpaired.
 */
export async function apiFetch(path, init = {}) {
  let token = await requireToken();
  if (!token) {
    token = await autoRepairToken("");
    if (!token) return null;
  }

  const doFetch = async (authToken) => {
    const headers = new Headers(init.headers || {});
    headers.set("Authorization", `Bearer ${authToken}`);
    const identity = await extensionIdentityHeaders();
    for (const [name, value] of Object.entries(identity)) headers.set(name, value);
    return fetch(`${ENGINE_ORIGIN}${path}`, {
      ...init,
      headers,
    });
  };

  let response = await doFetch(token);

  // If token mismatch detected (401 Unauthorized), auto-repair and retry once
  if (response && response.status === 401) {
    const freshToken = await autoRepairToken(token);
    if (freshToken) {
      response = await doFetch(freshToken);
    }
  }

  return response;
}

/**
 * @param {{url: string, filename?: string, referrer?: string, headers?: [string, string][]}} download
 * @returns {Promise<{ok: true, id?: string} | {ok: false, status: number, error: string}>}
 */
export async function enqueue(download) {
  try {
    const response = await apiFetch("/download", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(download),
    });

    if (!response) {
      return { ok: false, status: 0, error: "Not paired with the Zoltraak app yet." };
    }

    if (response.ok) {
      const body = await response.json().catch(() => ({}));
      return { ok: true, id: body.id };
    }

    const body = await response.json().catch(() => ({}));
    return {
      ok: false,
      status: response.status,
      error: body.error ?? `request failed (${response.status})`,
    };
  } catch {
    return { ok: false, status: 0, error: "Could not reach the Zoltraak app. Is it running?" };
  }
}

/**
 * @param {string} url
 * @param {string} [referrer]
 * @param {[string, string][]} [headers]
 * @returns {Promise<{ok: true, metadata: object} | {ok: false, error: string}>}
 */
export async function analyzeVideo(url, referrer, headers, sourceKind, mediaType) {
  try {
    const response = await apiFetch("/video/analyze", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url, referrer, headers: headers ?? [], sourceKind, mediaType }),
    });

    if (!response) {
      return { ok: false, error: "Not paired with the Zoltraak app yet." };
    }

    if (response.ok) return { ok: true, metadata: await response.json() };

    const body = await response.json().catch(() => ({}));
    return { ok: false, error: body.error ?? `request failed (${response.status})` };
  } catch {
    return { ok: false, error: "Could not reach the Zoltraak app. Is it running?" };
  }
}

/**
 * @param {{title: string, webpageUrl: string, format: object, referrer?: string, headers?: [string, string][], autoDiscord?: boolean}} job
 * @returns {Promise<{ok: true, id?: string} | {ok: false, error: string}>}
 */
export async function startVideoDownload({ title, webpageUrl, format, referrer, headers, autoDiscord }) {
  try {
    const response = await apiFetch("/video/download", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        title,
        webpageUrl,
        format,
        referrer,
        headers: headers ?? [],
        autoDiscord: Boolean(autoDiscord),
      }),
    });

    if (!response) {
      return { ok: false, error: "Not paired with the Zoltraak app yet." };
    }

    if (response.ok) {
      const body = await response.json().catch(() => ({}));
      return { ok: true, id: body.id };
    }

    const body = await response.json().catch(() => ({}));
    return { ok: false, error: body.error ?? `request failed (${response.status})` };
  } catch {
    return { ok: false, error: "Could not reach the Zoltraak app. Is it running?" };
  }
}

/**
 * Hand a URL to the desktop app's downloads text input (and bring its window forward)
 * @param {string} url
 * @param {[string, string][]} [headers]
 * @param {string} [referrer]
 * @returns {Promise<{ok: boolean, error?: string}>}
 */
export async function prefillDownload(url, headers, referrer) {
  try {
    const response = await apiFetch("/download/prefill", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url, headers: headers ?? [], referrer }),
    });
    if (!response) return { ok: false, error: "Not paired with the Zoltraak app yet." };
    if (response.ok) return { ok: true };
    const body = await response.json().catch(() => ({}));
    return { ok: false, error: body.error ?? `request failed (${response.status})` };
  } catch {
    return { ok: false, error: "Could not reach the Zoltraak app. Is it running?" };
  }
}

/**
 * Hand a URL to the desktop app's Video view (and bring its window forward)
 * @param {string} url
 * @param {[string, string][]} [headers]
 * @param {string} [referrer]
 * @param {boolean} [autoDownload]
 * @param {string} [title]
 * @returns {Promise<{ok: boolean, error?: string}>}
 */
export async function prefillVideo(url, headers, referrer, autoDownload, title) {
  try {
    const response = await apiFetch("/video/prefill", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        url,
        headers: headers ?? [],
        referrer,
        autoDownload: Boolean(autoDownload),
        title: title || undefined,
      }),
    });
    if (!response) return { ok: false, error: "Not paired with the Zoltraak app yet." };
    if (response.ok) return { ok: true };
    const body = await response.json().catch(() => ({}));
    return { ok: false, error: body.error ?? `request failed (${response.status})` };
  } catch {
    return { ok: false, error: "Could not reach the Zoltraak app. Is it running?" };
  }
}

/**
 * Hand a post URL to the desktop app's Images view
 * @param {string} url
 * @param {[string, string][]} [headers]
 * @param {string} [referrer]
 * @returns {Promise<{ok: boolean, error?: string}>}
 */
export async function prefillImages(url, headers, referrer, browserDocuments) {
  try {
    const response = await apiFetch("/image/prefill", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url, headers: headers ?? [], referrer, browserDocuments }),
    });
    if (!response) return { ok: false, error: "Not paired with the Zoltraak app yet." };
    if (response.ok) return { ok: true };
    const body = await response.json().catch(() => ({}));
    return { ok: false, error: body.error ?? `request failed (${response.status})` };
  } catch {
    return { ok: false, error: "Could not reach the Zoltraak app. Is it running?" };
  }
}

/**
 * Hand a music link to the desktop app's Music view
 * @param {string} url
 * @param {[string, string][]} [headers]
 * @param {string} [referrer]
 * @returns {Promise<{ok: boolean, error?: string}>}
 */
export async function prefillMusic(url, headers, referrer) {
  try {
    const response = await apiFetch("/music/prefill", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url, headers: headers ?? [], referrer }),
    });
    if (!response) return { ok: false, error: "Not paired with the Zoltraak app yet." };
    if (response.ok) return { ok: true };
    const body = await response.json().catch(() => ({}));
    return { ok: false, error: body.error ?? `request failed (${response.status})` };
  } catch {
    return { ok: false, error: "Could not reach the Zoltraak app. Is it running?" };
  }
}

/**
 * Share the browser's session for a login-gated platform with the app's session vault
 * @param {string} domain
 * @param {{cookie?: string|null, poToken?: string|null, visitorData?: string|null}} session
 * @returns {Promise<{ok: boolean}>}
 */
export async function pushSession(domain, session) {
  try {
    const response = await apiFetch("/sessions", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        domain,
        cookie: session.cookie || undefined,
        poToken: session.poToken || undefined,
        visitorData: session.visitorData || undefined,
        poTokenVideoId: session.poTokenVideoId || undefined,
      }),
    });
    return { ok: Boolean(response?.ok) };
  } catch {
    return { ok: false };
  }
}

/** Older shape of `pushSession`: the session as a header list. */
export async function pushSiteCookies(domain, headers) {
  const cookie = (headers || []).find(([name]) => name.toLowerCase() === "cookie")?.[1];
  if (!cookie) return { ok: false };
  return pushSession(domain, { cookie });
}

export async function getSnapshot() {
  try {
    const [resSnap, resJobs] = await Promise.all([
      apiFetch("/downloads").then((r) => (r && r.ok ? r.json() : [])).catch(() => []),
      apiFetch("/video/jobs").then((r) => (r && r.ok ? r.json() : [])).catch(() => []),
    ]);
    return { ok: true, items: Array.isArray(resSnap) ? resSnap : [], videoJobs: Array.isArray(resJobs) ? resJobs : [] };
  } catch {
    return { ok: false, items: [], videoJobs: [] };
  }
}

/**
 * @param {string} id
 * @param {"pause"|"resume"} action
 * @returns {Promise<{ok: boolean}>}
 */
export async function setDownloadState(id, action) {
  try {
    const response = await apiFetch(`/downloads/${encodeURIComponent(id)}/${action}`, {
      method: "POST",
    });
    return { ok: Boolean(response?.ok) };
  } catch {
    return { ok: false };
  }
}

/**
 * Removes a download from the queue/history, or a video job if the id
 * belongs to one — tried in that order, same as reveal.
 * @param {string} id
 * @returns {Promise<{ok: boolean}>}
 */
export async function removeDownload(id) {
  try {
    const response = await apiFetch(`/downloads/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
    if (response?.ok) return { ok: true };
    const video = await apiFetch(`/video/jobs/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
    return { ok: Boolean(video?.ok) };
  } catch {
    return { ok: false };
  }
}

export async function revealTarget(id, path) {
  try {
    if (id) {
      await apiFetch("/download/reveal", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id }),
      });
    } else if (path) {
      await apiFetch("/folder/reveal", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ path }),
      });
    }
    return { ok: true };
  } catch {
    return { ok: false };
  }
}

export async function openTarget(id, path) {
  try {
    const res = await apiFetch("/download/open", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: id || undefined, path: path || undefined }),
    });
    return { ok: Boolean(res?.ok) };
  } catch {
    return { ok: false };
  }
}

/** Whether picking this format yields a file with sound — the engine's
 * `VideoFormat::plays_with_audio`. A separate audio stream counts because the
 * job muxes it in; an unknown `hasAudio` is assumed fine, since most
 * progressive files do carry audio and nothing has looked at them. */
export function playsWithAudio(format) {
  return Boolean(format.requiresMerge) || format.hasAudio !== false;
}

/**
 * The format a one-click flow should take, where nobody gets to see a picker.
 *
 * Returns the best *complete* rendition — highest resolution among those that
 * play with sound — and only when there is none at all falls back to the best
 * silent one, flagged so the caller can say so. Height decides first because
 * it is what the user perceives; bitrate breaks ties.
 *
 * @returns {{format: object, complete: boolean} | null}
 */
export function bestCompleteFormat(formats) {
  if (!Array.isArray(formats) || formats.length === 0) return null;
  const score = (f) => (f.height ?? 0) * 1_000_000 + (f.bitrate ?? 0);
  const audioRank = (f) => f.requiresMerge || f.hasAudio === true ? 2 : f.hasAudio == null ? 1 : 0;
  const ranked = [...formats].sort((a, b) => audioRank(b) - audioRank(a) || score(b) - score(a));
  // An audio-only track plays with sound, trivially; it is still not a video.
  const complete = ranked.find((f) => playsWithAudio(f) && !globalThis.ZoltraakVideoIdentity.isAudioOnly(f));
  if (complete) return { format: complete, complete: true };
  return { format: ranked[0], complete: false };
}

/** Kept for callers that only need *a* format; prefers a complete one. */
export function bestFormat(formats) {
  return bestCompleteFormat(formats)?.format ?? null;
}

/**
 * The cookie names YouTube's sign-in actually reads. The browser holds many
 * more for youtube.com — tracking, experiments, ad state — and none of those
 * leave the browser: only these are ever shared with the app, which applies
 * the same allowlist again on its side.
 */
const YOUTUBE_SESSION_COOKIES = new Set([
  "SID", "HSID", "SSID", "APISID", "SAPISID",
  "__Secure-1PSID", "__Secure-3PSID", "__Secure-1PAPISID", "__Secure-3PAPISID",
  "__Secure-1PSIDTS", "__Secure-3PSIDTS", "__Secure-1PSIDCC", "__Secure-3PSIDCC", "SIDCC",
  "LOGIN_INFO", "PREF", "VISITOR_INFO1_LIVE", "VISITOR_PRIVACY_METADATA", "YSC",
  "__Secure-YEC", "__Secure-ROLLOUT_TOKEN", "CONSENT", "SOCS",
]);

/**
 * YouTube's session, reduced to the allowlist above, as one `Cookie` header —
 * or null when the browser holds nothing useful. Read from the youtube.com
 * cookie store rather than any particular page URL so an embed, a Short or a
 * youtu.be link all get the same session.
 */
export async function youtubeSessionCookie() {
  if (!(await canUseBrowserCookies())) return null;
  try {
    const cookies = await chrome.cookies.getAll({ domain: "youtube.com" }).catch(() => []);
    // One value per name; the widest-scoped (".youtube.com") entry wins.
    const byName = new Map();
    for (const c of cookies) {
      if (!YOUTUBE_SESSION_COOKIES.has(c.name)) continue;
      const existing = byName.get(c.name);
      if (!existing || (c.domain || "").startsWith(".")) byName.set(c.name, c);
    }
    if (byName.size === 0) return null;
    return [...byName.values()].map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return null;
  }
}

/**
 * The session cookies for `url`, collapsed into the single header a browser
 * would have sent — `Cookie` is one header with `;`-joined pairs, never many.
 * Falls back to root domain cookies (.facebook.com, .instagram.com, etc.) if path cookies are empty.
 * YouTube goes through `youtubeSessionCookie` instead, which applies the allowlist.
 */
export async function cookieHeader(url, storeId) {
  if (!(await canUseBrowserCookies())) return null;
  if (/^https?:\/\/(?:[a-z0-9-]+\.)*(youtube\.com|youtu\.be|youtube-nocookie\.com)\//i.test(url || "")) {
    return youtubeSessionCookie();
  }
  try {
    const store = storeId ? { storeId } : {};
    // URL matching includes domain, path and secure-cookie rules. A broad
    // domain fallback could replay unrelated cookies (especially on co.uk).
    const cookies = await chrome.cookies.getAll({ url, ...store }).catch(() => []);
    if (cookies.length === 0) return null;
    return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return null;
  }
}

