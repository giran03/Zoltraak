// Runs in the isolated world, independently of the visible-video overlay.
(() => {
  if (globalThis.ZoltraakGenericMedia) return;
  const players = new Map();
  const roots = new WeakSet();
  const observers = [];
  const sourceMap = new Map();
  const manifestMap = new Map();
  const manifestPatterns = new Map();
  const manifestFlags = new Map();
  let nextId = 0, timer, lastSnapshot = "", stopped = false;
  const documentStartedAt = performance.timeOrigin;
  const http = (value) => {
    if (typeof value !== "string" || !value.trim() || value.length > 8192) return null;
    try { const u = new URL(value, document.baseURI); return /^https?:$/.test(u.protocol) ? u.href : null; }
    catch { return null; }
  };
  const kind = (url) => /\.m3u8(?:[?#]|$)/i.test(url) ? "hls" : /\.mpd(?:[?#]|$)/i.test(url) ? "dash" : "file";
  function sourcesFor(media) {
    const sources = [], seen = new Set();
    const add = (value, evidence, sourceKind, flags) => {
      const url = http(value);
      flags ||= manifestFlags.get(url);
      const existing = sources.find((s) => s.url === url);
      if (existing && flags) { existing.live ||= flags.live; existing.protected ||= flags.protected; }
      if (url && !seen.has(url) && sources.length < 12) {
        seen.add(url); sources.push({ url, kind: sourceKind || kind(url), evidence,
          live: flags?.live === true, protected: flags?.protected === true });
      }
    };
    const src = media.getAttribute("src") ? media.src : media.currentSrc || media.src;
    // A pending replacement must not inherit the previous buffered source.
    if (!media.getAttribute("src") || !media.currentSrc || http(media.src) === http(media.currentSrc)) add(src, "element");
    add(media.src, "element");
    for (const node of media.querySelectorAll("source[src]")) add(node.src, "element");
    if (typeof manifestFromAttributes === "function") add(manifestFromAttributes(media), "element-manifest");
    const mapped = sourceMap.get(src);
    if (mapped) for (const value of mapped.urls) {
      const manifests = [...(manifestMap.get(value) || []), ...[...manifestPatterns.values()].filter((entry) => entry.patterns.some((pattern) => pattern.test(value)))];
      if (manifests.length) for (const entry of manifests) {
        const parents = manifestMap.get(entry.url);
        if (parents?.length) for (const parent of parents) add(parent.url, "mse-manifest", parent.kind, manifestFlags.get(entry.url));
        else add(entry.url, "mse-manifest", entry.kind);
      }
      // Segments never become standalone downloads.
      else if (mapped.file) add(value, "mse-buffer");
    }
    if (typeof videoPermalink === "function" && media.tagName === "VIDEO") {
      const permalink = videoPermalink(media);
      if (globalThis.ZoltraakVideoIdentity?.itemIdentity(permalink)) add(permalink, "social-item", "page");
    }
    return sources;
  }
  function describe(media, entry) {
    const currentSrc = media.currentSrc || media.src || "";
    const sources = sourcesFor(media);
    const fingerprint = JSON.stringify([currentSrc, media.getAttribute("src"),
      [...media.querySelectorAll("source[src]")].map((n) => n.src), media.poster || "",
      sources.map((s) => s.url)]);
    if (entry.fingerprint !== fingerprint) {
      entry.fingerprint = fingerprint; entry.generation++; entry.startedAt = Date.now();
    }
    const protectedMedia = entry.protected || Boolean(media.mediaKeys) || sources.some((s) => s.protected);
    const live = media.duration === Infinity || sources.some((s) => s.live);
    return { id: entry.id, generation: entry.generation, fingerprint, startedAt: entry.startedAt,
      type: media.tagName.toLowerCase(), title: (media.getAttribute("aria-label") || media.title || document.title || "Media").slice(0, 180),
      state: media.ended ? "ended" : media.paused ? "paused" : media.readyState < 3 ? "buffering" : "playing",
      currentSrc, duration: Number.isFinite(media.duration) ? media.duration : null,
      width: media.videoWidth || null, height: media.videoHeight || null,
      protected: protectedMedia, live, sources,
      status: protectedMedia ? "protected" : live ? "live" : media.srcObject ? "unsupported" : sources.length ? "source-found" : "unresolved" };
  }
  function snapshot() {
    const list = [];
    for (const [media, entry] of players) {
      if (!media.isConnected) { players.delete(media); continue; }
      if (list.length < 64) list.push(describe(media, entry));
    }
    return { type: "zoltraak-media-snapshot", documentStartedAt, pageUrl: http(location.href) || http(document.referrer), players: list };
  }
  async function publish(force = false, restored = false) {
    if (stopped) return;
    const payload = snapshot();
    if (restored) payload.restored = true;
    const serialized = JSON.stringify(payload);
    if (!force && serialized === lastSnapshot) return;
    try { await chrome.runtime.sendMessage(payload); lastSnapshot = serialized; } catch { /* Extension reload. */ }
  }
  function schedule() {
    clearTimeout(timer);
    timer = setTimeout(() => { void publish(); }, 120);
  }
  function track(media) {
    if (players.has(media) || players.size >= 128) return;
    const entry = { id: `media-${++nextId}`, generation: 0, startedAt: Date.now(), protected: false };
    players.set(media, entry);
    describe(media, entry);
    for (const event of ["play", "playing", "pause", "waiting", "ended", "loadedmetadata", "durationchange", "emptied", "loadstart", "encrypted", "timeupdate"]) {
      media.addEventListener(event, () => {
        if (event === "loadstart" || event === "emptied") { entry.generation++; entry.protected = false; entry.startedAt = Date.now(); }
        if (event === "encrypted") entry.protected = true;
        if (event === "timeupdate") {
          const rewound = entry.lastTime > 2 && media.currentTime < .5;
          entry.lastTime = media.currentTime;
          if (!rewound) return;
          entry.generation++; entry.startedAt = Date.now();
        }
        schedule();
      });
    }
  }
  function scan(node) {
    if (node.nodeType !== 1 && node.nodeType !== 9 && node.nodeType !== 11) return;
    if (node.matches?.("video,audio")) track(node);
    if (node.shadowRoot) observe(node.shadowRoot);
    for (const element of node.querySelectorAll("*")) {
      if (element.matches("video,audio")) track(element);
      if (element.shadowRoot) observe(element.shadowRoot);
    }
  }
  function observe(root) {
    if (roots.has(root)) return;
    roots.add(root);
    const observer = new MutationObserver((changes) => {
      for (const change of changes) for (const node of change.addedNodes) scan(node);
      schedule();
    });
    observer.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ["src", "poster", "data-hls-src", "data-dash-src", "data-manifest-url"] });
    observers.push(observer); scan(root);
  }
  // MAIN-world reports are hints, never permission to read credentials or queue a download.
  window.addEventListener("message", (event) => {
    if (event.source !== window || event.data?.channel !== "zoltraak-media-probe") return;
    const data = event.data;
    if (["mse", "blob"].includes(data.type) && typeof data.blob === "string" && data.blob.startsWith("blob:") && data.blob.length < 8192 && Array.isArray(data.urls)) {
      const urls = data.urls.slice(0, 64).map(http).filter(Boolean);
      sourceMap.set(data.blob, { urls, file: data.type === "blob" });
      while (sourceMap.size > 64) sourceMap.delete(sourceMap.keys().next().value);
    } else if (data.type === "manifest" && http(data.url) && ["hls", "dash"].includes(data.kind) && Array.isArray(data.children)) {
      manifestFlags.set(http(data.url), { live: data.live === true, protected: data.protected === true });
      while (manifestFlags.size > 64) manifestFlags.delete(manifestFlags.keys().next().value);
      for (const child of data.children.slice(0, 256).map(http).filter(Boolean)) {
        const entries = manifestMap.get(child) || [];
        if (!entries.some((e) => e.url === data.url)) entries.push({ url: http(data.url), kind: data.kind });
        manifestMap.set(child, entries.slice(-4));
      }
      while (manifestMap.size > 2048) manifestMap.delete(manifestMap.keys().next().value);
      const patterns = (Array.isArray(data.templates) ? data.templates : []).slice(0, 64).filter((template) => typeof template === "string" && template.length < 8192 && http(template))
        .map((template) => {
          const tokens = template.split(/(\$(?:Number|Time)(?:%0\d{1,2}d)?\$)/);
          return new RegExp(`^${tokens.map((token) => /^\$(?:Number|Time)/.test(token) ? "[0-9]{1,20}" : token.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("")}$`);
        });
      manifestPatterns.set(data.url, { url: http(data.url), kind: data.kind, patterns });
      while (manifestPatterns.size > 64) manifestPatterns.delete(manifestPatterns.keys().next().value);
    } else return;
    schedule();
  });
  document.addEventListener("zoltraak-shadow-root", (event) => { scan(event.target); schedule(); }, true);
  chrome.runtime.onMessage.addListener((message, _sender, reply) => {
    if (message?.type === "zoltraak-media-refresh") {
      if (stopped) { reply({ ok: false }); return false; }
      publish(true).then(() => reply({ ok: true })).catch(() => reply({ ok: false })); return true;
    }
    if (message?.type === "zoltraak-media-check") {
      const record = snapshot().players.find((p) => p.id === message.id);
      reply({ ok: Boolean(!stopped && record && record.generation === message.generation && record.fingerprint === message.fingerprint
        && documentStartedAt === message.documentStartedAt && !record.protected && !record.live), record });
    }
    return false;
  });
  window.addEventListener("pagehide", () => { stopped = true; clearTimeout(timer); });
  window.addEventListener("pageshow", (event) => { stopped = false; void publish(true, event.persisted); });
  window.addEventListener("popstate", schedule);
  window.addEventListener("hashchange", schedule);
  globalThis.ZoltraakGenericMedia = { snapshot, sourcesFor,
    recordFor: (media) => players.has(media) ? describe(media, players.get(media)) : null };
  observe(document);
  void publish(true);
})();
