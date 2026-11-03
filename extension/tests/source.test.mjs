import assert from "node:assert/strict";
import { test } from "node:test";

const values = {};
let cookieReads = 0;
globalThis.chrome = {
  runtime: { getManifest: () => ({ version: "1.2.3" }) },
  storage: {
    local: {
      get: async (keys) => {
        if (typeof keys === "string") return { [keys]: values[keys] };
        return Object.fromEntries((keys ?? []).map((key) => [key, values[key]]));
      },
      set: async (next) => Object.assign(values, next),
    },
  },
  cookies: {
    getAll: async () => {
      cookieReads += 1;
      return [{ name: "SAPISID", value: "secret", domain: ".youtube.com" }];
    },
  },
};

const api = await import("../api.js");

test("extension client identity persists in profile-local storage", async () => {
  delete values.zoltraakClientId;
  const first = await api.extensionClientId();
  const second = await api.extensionClientId();
  assert.equal(first, second);
  assert.match(first, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
});

test("inactive and stale profiles never read browser cookies", async () => {
  cookieReads = 0;
  await api.setCookieSourceState(false, 1);
  assert.equal(await api.cookieHeader("https://youtube.com/watch?v=1"), null);
  assert.equal(cookieReads, 0);

  values.zoltraakCookieSource = { active: true, revision: 2, confirmedAt: Date.now() - 66_000 };
  assert.equal(await api.youtubeSessionCookie(), null);
  assert.equal(cookieReads, 0);
});

test("the confirmed active profile may read its cookie store", async () => {
  cookieReads = 0;
  await api.setCookieSourceState(true, 3);
  assert.equal(await api.youtubeSessionCookie(), "SAPISID=secret");
  assert.equal(cookieReads, 1);
});

test("authenticated extension requests carry the profile identity", async () => {
  values.token = "paired-token";
  values.zoltraakBrowserId = "chrome";
  let request;
  globalThis.fetch = async (_url, init) => {
    request = init;
    return new Response("[]", { status: 200 });
  };

  await api.apiFetch("/downloads");

  assert.equal(request.headers.get("Authorization"), "Bearer paired-token");
  assert.equal(request.headers.get("X-Zoltraak-Client-Id"), values.zoltraakClientId);
  assert.equal(request.headers.get("X-Zoltraak-Browser-Id"), "chrome");
  assert.equal(request.headers.get("X-Zoltraak-Extension-Version"), "1.2.3");
});

test("reload acknowledgement survives a service-worker restart", async () => {
  await api.setExtensionStatusCursor(7, 11);
  assert.deepEqual(await api.extensionStatusCursor(), {
    reloadVersion: 7,
    sourceRevision: 11,
  });
});

test("known audio outranks a higher-resolution stream whose audio is unknown", () => {
  const result = api.bestCompleteFormat([
    { id: "unknown", height: 1080, container: "mp4" },
    { id: "known", height: 720, container: "mp4", hasAudio: true },
  ]);
  assert.equal(result.format.id, "known");
});

test("audio-only files cannot replace a complete video in the video picker", () => {
  for (const container of ["mp3", "m4a", "ogg", "wav", "flac", "opus"]) {
    const result = api.bestCompleteFormat([
      { id: "audio", container, hasAudio: true, bitrate: 900000 },
      { id: "video", container: "mp4", height: 720, hasAudio: true },
    ]);
    assert.equal(result.format.id, "video");
  }
});


test("download cookies are scoped to the target URL and its container", async () => {
  await api.setCookieSourceState(true, 4);
  const calls = [];
  const original = chrome.cookies.getAll;
  chrome.cookies.getAll = async (query) => {
    calls.push(query);
    return query.domain ? [{ name: 'unrelated', value: 'secret' }] : [];
  };
  try {
    assert.equal(await api.cookieHeader('https://download.example.co.uk/private/file', 'container-2'), null);
    assert.deepEqual(calls, [{ url: 'https://download.example.co.uk/private/file', storeId: 'container-2' }]);
  } finally { chrome.cookies.getAll = original; }
});
