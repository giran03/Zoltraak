import assert from "node:assert/strict";
import { test } from "node:test";
import { cleanSnapshot, combineMedia, registerGenericMedia, validUrl } from "../media-registry.js";

const now = Date.now();
const sender = { tab: { id: 4 }, frameId: 0, documentId: "doc-a", url: "https://page.test/watch" };
const player = (id, url, type = "video") => ({ id, type, generation: 1, fingerprint: url, currentSrc: url,
  startedAt: now, state: "playing", sources: [{ url, kind: "file", evidence: "element" }] });
const snapshot = (players, started = now - 1000) => ({ players, documentStartedAt: started, pageUrl: sender.url });

test("each player keeps its own sources, including invisible audio", () => {
  const frame = cleanSnapshot(snapshot([player("media-1", "https://cdn.test/one.mp4"),
    player("media-2", "https://cdn.test/two.mp4"), player("media-3", "https://cdn.test/audio.mp3", "audio")]), sender, now);
  const result = combineMedia([frame], [], now);
  assert.equal(result.players.length, 3);
  assert.equal(result.players[2].type, "audio");
  assert.deepEqual(result.players.map((p) => p.sources[0].url), ["https://cdn.test/one.mp4", "https://cdn.test/two.mp4", "https://cdn.test/audio.mp3"]);
});
test("frame traffic stays unassigned even with one playing player", () => {
  const frame = cleanSnapshot(snapshot([player("media-1", "https://cdn.test/one.mp4")]), sender, now);
  const capture = { url: "https://cdn.test/neighbor.m3u8", kind: "hls", frameId: 0, documentId: "doc-a", ts: now };
  const result = combineMedia([frame], [capture], now);
  assert.equal(result.unassigned.length, 1);
  assert.equal(result.players[0].sources.length, 1);
});
test("old documents, detached frames, expired captures and assigned URLs are excluded", () => {
  const frame = cleanSnapshot(snapshot([player("media-1", "https://cdn.test/one.mp4")]), sender, now);
  const capture = { url: "https://cdn.test/one.mp4", kind: "file", frameId: 0, documentId: "doc-a", ts: now };
  assert.equal(combineMedia([frame], [capture, { ...capture, url: "https://cdn.test/old.mp4", documentId: "old" },
    { ...capture, url: "https://cdn.test/frame.mp4", frameId: 1 }, { ...capture, ts: now - 2000 }], now).unassigned.length, 0);
  assert.equal(combineMedia([{ ...frame, updatedAt: now - 31 * 60_000 }], [capture], now).players.length, 0);
});
test("boundaries reject credentials, local schemes, oversized reports and invalid players", () => {
  for (const url of ["blob:test", "javascript:alert(1)", "https://user:pass@cdn.test/x", "https://cdn.test/" + "x".repeat(8192)]) assert.equal(validUrl(url), null);
  assert.equal(cleanSnapshot(snapshot([]), { ...sender, tab: null }, now), null);
  assert.equal(cleanSnapshot(snapshot([], now + 60_000), sender, now), null);
  const frame = cleanSnapshot(snapshot([player("invalid", "https://cdn.test/one.mp4"),
    player("media-2", "file:///secret")]), sender, now);
  assert.equal(frame.players.length, 1);
  assert.equal(frame.players[0].sources.length, 0);
});
test("signed query strings are retained byte for byte", () => {
  const url = "https://cdn.test/media?sig=a%2Fb+z&range=100&bytestart=0&byteend=100";
  assert.equal(cleanSnapshot(snapshot([player("media-1", url)]), sender, now).players[0].sources[0].url, url);
});
test("related blank frames retain an HTTP referrer but privileged pages are rejected", () => {
  const frame = cleanSnapshot({ ...snapshot([]), pageUrl: sender.url }, { ...sender, url: "about:srcdoc", origin: "https://page.test" }, now);
  assert.equal(frame.pageUrl, sender.url);
  assert.equal(cleanSnapshot(snapshot([]), { ...sender, url: "chrome://settings", origin: "null" }, now), null);
});

function fixture(store = {}) {
  const listeners = [], calls = [];
  let checkResult = true, failAfterAnalysis = false;
  const chrome = {
    runtime: { getURL: (path) => `chrome-extension://fixture/${path}`, onMessage: { addListener: (listener) => listeners.push(listener) } },
    tabs: { get: async (id) => ({ id, url: sender.url }),
      sendMessage: async (_id, message) => message.type === "zoltraak-media-check"
        ? { ok: checkResult, record: report.players[0] } : { ok: true } },
  };
  const report = snapshot([player("media-1", "https://cdn.test/one.mp4")]);
  const dependencies = { store: { get: async (key) => ({ [key]: store[key] }), set: async (values) => Object.assign(store, values) },
    getCaptures: async () => [], prefill: async () => ({ ok: true }),
    download: async (options) => { if (failAfterAnalysis) checkResult = false; await options.genericGuard(); calls.push(options); return { ok: true }; } };
  const registry = registerGenericMedia(chrome, dependencies);
  const send = (body, from = { url: "chrome-extension://fixture/popup.html" }) => new Promise((resolve) => listeners[0](body, from, resolve));
  const choose = { type: "zoltraak-media-download", tabId: 4, frameId: 0, id: "media-1", generation: 1, documentStartedAt: report.documentStartedAt, url: report.players[0].sources[0].url };
  return { store, calls, report, registry, send, choose, dependencies,
    expireDuringAnalysis() { failAfterAnalysis = true; } };
}
test("snapshot persistence survives a restarted service worker", async () => {
  const f = fixture();
  assert.equal((await f.send({ type: "zoltraak-media-snapshot", ...f.report }, sender)).ok, true);
  const restarted = fixture(f.store);
  const result = await restarted.send({ type: "zoltraak-media-list", tabId: 4 });
  assert.equal(result.players.length, 1);
});
test("page scripts cannot initiate registry downloads or list other tabs", async () => {
  const f = fixture();
  for (const type of ["zoltraak-media-list", "zoltraak-media-download", "zoltraak-media-prefill"]) {
    assert.equal((await f.send({ ...f.choose, type }, sender)).ok, false);
  }
  assert.equal(f.calls.length, 0);
});
test("a replacement source is rejected after analysis without starting a download", async () => {
  const f = fixture();
  await f.send({ type: "zoltraak-media-snapshot", ...f.report }, sender);
  f.expireDuringAnalysis();
  const result = await f.send(f.choose);
  assert.equal(result.ok, false);
  assert.match(result.error, /changed/);
  assert.equal(f.calls.length, 0);
});
test("a verified selection forwards only its own source and type", async () => {
  const f = fixture();
  await f.send({ type: "zoltraak-media-snapshot", ...f.report }, sender);
  assert.equal((await f.send(f.choose)).ok, true);
  assert.deepEqual(f.calls[0].candidates, [f.choose.url]);
  assert.equal(f.calls[0].sourceKinds[f.choose.url], "file");
  assert.equal(f.calls[0].videoContext.mediaType, "video");
});
test("newer frame documents cannot be overwritten by late reports", async () => {
  const f = fixture();
  await f.send({ type: "zoltraak-media-snapshot", ...f.report }, sender);
  await f.send({ type: "zoltraak-media-snapshot", ...snapshot([], f.report.documentStartedAt - 100) }, sender);
  assert.equal((await f.registry.frames(4))[0].players.length, 1);
  await f.registry.clear(4, 0);
  assert.equal((await f.registry.frames(4)).length, 0);
});
test("an active restored document replaces cached navigation state", async () => {
  const f = fixture();
  await f.send({ type: "zoltraak-media-snapshot", ...f.report }, sender);
  const restored = { ...snapshot([], f.report.documentStartedAt - 100), type: "zoltraak-media-snapshot" };
  assert.equal((await f.send(restored, { ...sender, documentId: "restored", documentLifecycle: "active" })).ok, true);
  assert.equal((await f.registry.frames(4))[0].documentId, "restored");
  assert.equal(cleanSnapshot(f.report, { ...sender, documentLifecycle: "cached" }), null);
  const firefox = fixture();
  await firefox.send({ type: "zoltraak-media-snapshot", ...firefox.report }, { ...sender, documentId: undefined });
  await firefox.send({ ...restored, restored: true }, { ...sender, documentId: undefined });
  assert.equal((await firefox.registry.frames(4))[0].documentStartedAt, restored.documentStartedAt);
});
test("protected and live selections cannot queue ordinary downloads", async () => {
  for (const field of ["protected", "live"]) {
    const f = fixture(); f.report.players[0][field] = true;
    await f.send({ type: "zoltraak-media-snapshot", ...f.report }, sender);
    assert.equal((await f.send(f.choose)).ok, false);
    assert.equal(f.calls.length, 0);
    if (field === "live") assert.equal((await f.send({ ...f.choose, type: "zoltraak-media-prefill" })).ok, true);
  }
});
