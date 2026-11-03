// Runs in the PAGE's own JS context (world: MAIN) at document_start on
// youtube.com — before YouTube's player bundle loads — so it can observe the
// player's own requests.
//
// Its job is to harvest the proof-of-origin tokens the real player mints, and
// the **visitorData** they are bound to, so the engine can present them the
// way the player does:
//
// - the **player token**, sent in plain JSON under
//   `serviceIntegrityDimensions.poToken` in the body of the player's
//   `/youtubei/v1/player` call — or, on today's pages, inside the
//   `playerRequest` of `/youtubei/v1/get_watch`, gzip-compressed and passed
//   as a `Request` object rather than a string;
// - the **stream (GVS) token**, which the player puts in the protobuf body of
//   its POST `videoplayback` requests (streamer context, field 19, sub-field
//   2) and which the CDN checks as `pot=` on stream URLs.
//
// Whatever is found is posted to the extension's content script (isolated
// world) via window.postMessage; the content script caches it so the engine
// can attach it to its own requests. This world cannot use chrome.* APIs, so
// postMessage is the only bridge. Nothing here ever changes a request — every
// hook forwards to the original function untouched.

(function () {
  const FLAG = "__zoltraak_yt_harvest__";
  if (window[FLAG]) return;
  window[FLAG] = true;

  const last = { poToken: null, playerToken: null, visitorData: null, videoId: null, ts: 0 };

  function currentVideoId() {
    const url = new URL(window.location.href);
    const id = url.searchParams.get("v") || url.pathname.match(/^\/(?:shorts|live|embed)\/([\w-]{11})/)?.[1];
    return /^[\w-]{11}$/.test(id || "") ? id : null;
  }

  function emit(update) {
    const videoId = update.videoId || currentVideoId();
    if (!videoId || videoId !== currentVideoId()) return;
    let changed = false;
    if (last.videoId !== videoId || (update.visitorData && last.visitorData !== update.visitorData)) {
      last.poToken = null;
      last.playerToken = null;
      last.ts = 0;
      last.videoId = videoId;
      changed = true;
    }
    for (const key of ["poToken", "playerToken", "visitorData"]) {
      if (update[key] && update[key] !== last[key]) {
        last[key] = update[key];
        changed = true;
        if (key === "poToken") last.ts = Date.now();
      }
    }
    if (!changed) return;
    try {
      window.postMessage(
        {
          source: "zoltraak-yt-auth",
          poToken: last.poToken,
          playerToken: last.playerToken,
          visitorData: last.visitorData,
          videoId: last.videoId,
          ts: last.ts,
        },
        "*",
      );
    } catch {
      /* postMessage can throw during teardown; ignore */
    }
  }

  // visitorData is available straight off ytcfg well before any request — a
  // useful backup for the (rarer) pages whose tokens we don't catch, and it
  // lets the engine at least send a matching visitorData.
  function fromYtcfg() {
    try {
      const vd =
        (window.ytcfg && typeof window.ytcfg.get === "function" && window.ytcfg.get("VISITOR_DATA")) ||
        window.ytcfg?.data_?.VISITOR_DATA;
      if (vd) emit({ visitorData: vd });
    } catch {
      /* ytcfg not ready yet */
    }
  }

  // ── Player / get_watch bodies (JSON, possibly gzip-compressed) ──────────
  function isPlayerUrl(url) {
    return typeof url === "string" && /\/youtubei\/v1\/(player|get_watch)(\?|$)/.test(url);
  }

  function isStreamPost(url, method) {
    return typeof url === "string" && /googlevideo\.com\/videoplayback/.test(url) && String(method || "GET").toUpperCase() === "POST";
  }

  function harvestStreamUrl(value) {
    try {
      const url = new URL(value, window.location.href);
      if (!url.hostname.endsWith(".googlevideo.com") || url.pathname !== "/videoplayback") return;
      const poToken = url.searchParams.get("pot");
      if (poToken) emit({ poToken });
    } catch { /* not a URL */ }
  }

  function harvestPlayerJson(text) {
    if (!text) return;
    try {
      const j = JSON.parse(text);
      const pr = j?.playerRequest || j;
      const playerToken = pr?.serviceIntegrityDimensions?.poToken || null;
      const visitorData = j?.context?.client?.visitorData || null;
      if (playerToken || visitorData) emit({ playerToken, visitorData, videoId: pr.videoId });
    } catch {
      /* not JSON / partial body — nothing to harvest */
    }
  }

  async function bytesToText(u8) {
    if (u8.length > 1 && u8[0] === 0x1f && u8[1] === 0x8b && typeof DecompressionStream === "function") {
      return new Response(new Blob([u8]).stream().pipeThrough(new DecompressionStream("gzip"))).text();
    }
    return new TextDecoder().decode(u8);
  }

  // Turn whatever a body is (string, ArrayBuffer, view, Blob, or the body of
  // a Request) into bytes, without consuming the original.
  async function bodyBytes(body, request) {
    try {
      if (typeof body === "string") return new TextEncoder().encode(body);
      if (body instanceof ArrayBuffer) return new Uint8Array(body);
      if (body && ArrayBuffer.isView(body)) return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
      if (typeof Blob !== "undefined" && body instanceof Blob) return new Uint8Array(await body.arrayBuffer());
      if (request && typeof request.clone === "function") return new Uint8Array(await request.clone().arrayBuffer());
    } catch {
      /* a body that cannot be read is a body we do not need */
    }
    return null;
  }

  async function harvestPlayerBody(body, request) {
    const bytes = await bodyBytes(body, request);
    if (!bytes) return;
    harvestPlayerJson(await bytesToText(bytes));
  }

  // ── Stream request bodies (protobuf) ──────────────────────────────────────
  // Minimal protobuf walk: field 19 (streamer context) → field 2 (po_token).
  function readVarint(u8, pos) {
    let value = 0;
    let shift = 0;
    let byte;
    do {
      if (pos.i >= u8.length) return null;
      byte = u8[pos.i++];
      value += (byte & 0x7f) * Math.pow(2, shift);
      shift += 7;
    } while (byte & 0x80);
    return value;
  }

  function findField(u8, wanted) {
    const pos = { i: 0 };
    while (pos.i < u8.length) {
      const key = readVarint(u8, pos);
      if (key === null) return null;
      const field = Math.floor(key / 8);
      const wire = key % 8;
      if (wire === 0) {
        if (readVarint(u8, pos) === null) return null;
      } else if (wire === 2) {
        const len = readVarint(u8, pos);
        if (len === null || pos.i + len > u8.length) return null;
        const slice = u8.subarray(pos.i, pos.i + len);
        pos.i += len;
        if (field === wanted) return slice;
      } else if (wire === 1) {
        pos.i += 8;
      } else if (wire === 5) {
        pos.i += 4;
      } else {
        return null;
      }
    }
    return null;
  }

  function base64Url(u8) {
    let s = "";
    for (let i = 0; i < u8.length; i++) s += String.fromCharCode(u8[i]);
    return btoa(s).replace(/\+/g, "-").replace(/\//g, "_");
  }

  async function harvestStreamBody(body, request, videoId) {
    const bytes = await bodyBytes(body, request);
    if (!bytes || bytes.length < 16) return;
    try {
      const context = findField(bytes, 19);
      const token = context && findField(context, 2);
      if (token && token.length >= 16) emit({ poToken: base64Url(token), videoId });
    } catch {
      /* malformed or unexpected body — nothing to harvest */
    }
  }

  // ── Patch fetch (the path the modern web player uses) ────────────────────
  const origFetch = window.fetch;
  if (typeof origFetch === "function") {
    window.fetch = function (input, init) {
      try {
        const url = typeof input === "string" ? input : input && input.url;
        const method = (init && init.method) || (input && typeof input === "object" && input.method);
        const body = init && init.body != null ? init.body : null;
        harvestStreamUrl(url);
        if (isPlayerUrl(url)) {
          void harvestPlayerBody(body, typeof input === "object" ? input : null);
        } else if (isStreamPost(url, method)) {
          void harvestStreamBody(body, typeof input === "object" ? input : null, currentVideoId());
        }
      } catch {
        /* never let harvesting break a real request */
      }
      return origFetch.apply(this, arguments);
    };
  }

  // ── Patch XHR (belt-and-suspenders for any fallback path) ────────────────
  const origOpen = XMLHttpRequest.prototype.open;
  const origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url) {
    harvestStreamUrl(url);
    this.__zoltraak_kind = isPlayerUrl(url) ? "player" : isStreamPost(url, method) ? "stream" : null;
    return origOpen.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function (body) {
    try {
      if (this.__zoltraak_kind === "player") void harvestPlayerBody(body, null);
      else if (this.__zoltraak_kind === "stream") void harvestStreamBody(body, null, currentVideoId());
    } catch {
      /* ignore */
    }
    return origSend.apply(this, arguments);
  };

  fromYtcfg();
  // The isolated content script starts at document_idle and can miss the
  // initial capture. Replay the snapshot when its listener is ready.
  window.addEventListener("message", (event) => {
    if (event.source !== window || event.data?.source !== "zoltraak-yt-auth-request") return;
    if (last.videoId !== currentVideoId()) return;
    window.postMessage({ source: "zoltraak-yt-auth", ...last }, "*");
  });
  // ytcfg/token can arrive slightly after document_start; poll briefly then stop.
  let ticks = 0;
  const iv = setInterval(() => {
    fromYtcfg();
    if (++ticks > 15) clearInterval(iv);
  }, 1000);
})();
