// Generic playback registry: persisted frame snapshots survive MV3 worker idle.
const MAX_AGE = 30 * 60 * 1000;
const keyFor = (tabId) => `players:${tabId}`;
export function validUrl(value) {
  if (typeof value !== "string" || value.length > 8192) return null;
  try { const u = new URL(value); return /^https?:$/.test(u.protocol) && !u.username && !u.password ? value : null; }
  catch { return null; }
}
export function cleanSnapshot(message, sender, now = Date.now()) {
  const frameAllowed = validUrl(sender?.url) || (/^(?:about:(?:blank|srcdoc)|blob:https?:\/\/)/.test(sender?.url || "")
    && (validUrl(sender.origin) || validUrl(sender.tab?.url)));
  if (!Number.isInteger(sender?.tab?.id) || sender.tab.id < 0 || !frameAllowed
      || (sender.documentLifecycle && sender.documentLifecycle !== "active")
      || !Number.isFinite(message.documentStartedAt) || message.documentStartedAt <= 0 || message.documentStartedAt > now + 1000
      || !Array.isArray(message.players)) return null;
  const players = message.players.slice(0, 64).filter((p) => p && /^media-\d+$/.test(p.id)
    && Number.isInteger(p.generation) && p.generation >= 0 && ["audio", "video"].includes(p.type)
    && typeof p.fingerprint === "string" && p.fingerprint.length < 32768).map((p) => {
    const sources = (Array.isArray(p.sources) ? p.sources : []).slice(0, 12)
      .filter((s) => s && validUrl(s.url) && ["file", "hls", "dash", "page"].includes(s.kind)
        && ["element", "element-manifest", "mse-manifest", "mse-buffer", "social-item"].includes(s.evidence))
      .map(({ url, kind, evidence }) => ({ url, kind, evidence }));
    return { id: p.id, generation: p.generation, fingerprint: p.fingerprint, type: p.type,
      title: typeof p.title === "string" ? p.title.slice(0, 180) : "Media",
      currentSrc: validUrl(p.currentSrc), startedAt: Number.isFinite(p.startedAt) ? p.startedAt : now,
      duration: Number.isFinite(p.duration) && p.duration > 0 ? p.duration : null,
      width: Number.isFinite(p.width) && p.width > 0 ? p.width : null,
      height: Number.isFinite(p.height) && p.height > 0 ? p.height : null,
      state: ["playing", "paused", "buffering", "ended"].includes(p.state) ? p.state : "paused",
      protected: p.protected === true, live: p.live === true, sources,
      status: p.protected ? "protected" : p.live ? "live" : p.status === "unsupported" ? "unsupported" : sources.length ? "source-found" : "unresolved" };
  });
  return { frameId: sender.frameId ?? 0, documentId: sender.documentId, url: sender.url,
    pageUrl: validUrl(message.pageUrl) || sender.url, documentStartedAt: message.documentStartedAt, updatedAt: now, players };
}
export function combineMedia(frames, captures, now = Date.now()) {
  const active = frames.filter((frame) => now - frame.updatedAt < MAX_AGE);
  const players = active.flatMap((frame) => frame.players.map((player) => ({ ...player,
    frameId: frame.frameId, documentId: frame.documentId, pageUrl: frame.pageUrl,
    documentStartedAt: frame.documentStartedAt })));
  const assigned = new Set(players.flatMap((p) => p.sources.map((s) => `${p.frameId}:${s.url}`)));
  const unassigned = captures.filter((c) => validUrl(c.url) && now - c.ts < MAX_AGE
    && !assigned.has(`${c.frameId}:${c.url}`) && active.some((frame) => frame.frameId === c.frameId
      && (!c.documentId || !frame.documentId || c.documentId === frame.documentId)
      && (!c.documentUrl || c.documentUrl === frame.url.split("#")[0]) && c.ts >= frame.documentStartedAt))
    .map(({ url, kind, frameId, documentId, ts }) => ({ url, kind, frameId, documentId, ts, status: "unassigned" }));
  return { players, unassigned };
}
export function registerGenericMedia(chrome, { store, getCaptures, download, prefill }) {
  const writes = new Map();
  async function frames(tabId) {
    await writes.get(tabId);
    const key = keyFor(tabId), values = await store.get(key);
    return Array.isArray(values[key]) ? values[key] : [];
  }
  function edit(tabId, operation) {
    const next = (writes.get(tabId) || Promise.resolve()).catch(() => {}).then(async () => {
      const key = keyFor(tabId), values = await store.get(key);
      const next = operation(Array.isArray(values[key]) ? values[key] : []);
      if (next === null && store.remove) await store.remove(key);
      else await store.set({ [key]: next || [] });
    });
    writes.set(tabId, next);
    void next.catch(() => {}).finally(() => { if (writes.get(tabId) === next) writes.delete(tabId); });
    return next;
  }
  async function clear(tabId, frameId) {
    await edit(tabId, (items) => frameId == null ? null : items.filter((f) => f.frameId !== frameId));
  }
  async function refresh(tabId) {
    // Broadcasting asks newly inserted frames to publish, even if not yet registered.
    const broadcast = await chrome.tabs.sendMessage(tabId, { type: "zoltraak-media-refresh" }).catch(() => null);
    const registered = await frames(tabId);
    await Promise.all(registered.map(async (f) => {
      const reply = await chrome.tabs.sendMessage(tabId, { type: "zoltraak-media-refresh" },
        f.documentId ? { documentId: f.documentId } : { frameId: f.frameId }).catch(() => null);
      if (!reply?.ok) await edit(tabId, (items) => items.filter((item) => item.frameId !== f.frameId || item.documentStartedAt !== f.documentStartedAt));
    }));
    const currentFrames = await frames(tabId);
    return { ...combineMedia(currentFrames, await getCaptures(tabId)), accessible: broadcast?.ok === true || currentFrames.length > 0 };
  }
  const trustedPage = (sender) => typeof sender.url === "string" && sender.url.startsWith(chrome.runtime.getURL(""))
    && (!chrome.runtime.id || sender.id === chrome.runtime.id);
  async function act(message) {
    if (!Number.isInteger(message.tabId) || message.tabId < 0 || !validUrl(message.url)) throw new Error("Invalid media selection.");
    const tab = await chrome.tabs.get(message.tabId);
    const list = await refresh(message.tabId);
    const player = message.id ? list.players.find((p) => p.id === message.id && p.frameId === message.frameId
      && p.generation === message.generation && p.documentStartedAt === message.documentStartedAt) : null;
    const source = player ? player.sources.find((s) => s.url === message.url)
      : !message.id && list.unassigned.find((s) => s.url === message.url && s.frameId === message.frameId);
    if (!source) throw new Error("The media changed. Refresh the list and select it again.");
    if (player?.protected) throw new Error("This media is protected.");
    if (player?.live && message.type !== "zoltraak-media-prefill") throw new Error("Live media requires a recording duration. Open it in the app.");
    if (player?.status === "unsupported") throw new Error("This player has no downloadable network source.");
    const frame = (await frames(message.tabId)).find((f) => f.frameId === message.frameId);
    const scope = player?.documentId || frame?.documentId ? { documentId: player?.documentId || frame.documentId } : { frameId: message.frameId };
    const check = async () => {
      if (!player) {
        const currentFrame = (await frames(message.tabId)).find((f) => f.frameId === message.frameId);
        if (!frame || currentFrame?.documentStartedAt !== frame.documentStartedAt
          || !(await getCaptures(message.tabId)).some((c) => c.url === message.url && c.frameId === message.frameId
            && (!c.documentId || !frame.documentId || c.documentId === frame.documentId))) throw new Error("This source is no longer available.");
        return;
      }
      const response = await chrome.tabs.sendMessage(message.tabId, { type: "zoltraak-media-check", id: player.id,
        generation: player.generation, fingerprint: player.fingerprint, documentStartedAt: player.documentStartedAt }, scope);
      if (!response?.ok || !response.record?.sources?.some((s) => s.url === source.url)) throw new Error("The media changed. Select it again before downloading.");
    };
    const referrer = player?.pageUrl || frame?.pageUrl || tab.url;
    if (message.type === "zoltraak-media-prefill") return prefill(source.url, referrer, tab);
    await check();
    const context = player ? { currentSrc: source.evidence === "element" || source.evidence === "mse-buffer" ? source.url : player.currentSrc,
      elementManifest: ["element-manifest", "mse-manifest"].includes(source.evidence) ? source.url : null,
      permalink: source.evidence === "social-item" ? source.url : null,
      duration: player.duration, width: player.width, height: player.height, mediaType: player.type } : undefined;
    return download({ candidates: [source.url], sourceKinds: { [source.url]: source.kind }, referrer,
      title: player?.title || tab.title, videoContext: context, genericGuard: check },
      { tab, frameId: message.frameId, documentId: player?.documentId || frame?.documentId, url: referrer });
  }
  chrome.runtime.onMessage.addListener((message, sender, reply) => {
    if (message?.type === "zoltraak-media-snapshot") {
      const snapshot = cleanSnapshot(message, sender);
      if (!snapshot) { reply({ ok: false }); return false; }
      edit(sender.tab.id, (items) => {
        const old = items.find((f) => f.frameId === snapshot.frameId);
        const restored = message.restored === true || (snapshot.documentId && snapshot.documentId !== old?.documentId && sender.documentLifecycle === "active");
        if (old && old.documentStartedAt > snapshot.documentStartedAt && !restored) return items;
        return [...items.filter((f) => f.frameId !== snapshot.frameId && Date.now() - f.updatedAt < MAX_AGE), snapshot].slice(-64);
      }).then(() => reply({ ok: true })).catch(() => reply({ ok: false }));
      return true;
    }
    if (!["zoltraak-media-list", "zoltraak-media-download", "zoltraak-media-prefill"].includes(message?.type)) return false;
    if (!trustedPage(sender)) { reply({ ok: false, error: "Open the extension popup to select media." }); return false; }
    const operation = message.type === "zoltraak-media-list"
      ? Number.isInteger(message.tabId) && message.tabId >= 0 ? refresh(message.tabId).then((list) => ({ ok: true, ...list })) : Promise.reject(new Error("Invalid tab."))
      : act(message);
    operation.then(reply).catch((error) => reply({ ok: false, error: error.message || "The page is no longer available." }));
    return true;
  });
  chrome.tabs.onRemoved?.addListener((tabId) => { void clear(tabId).catch(() => {}); });
  return { clear, frames, refresh };
}
