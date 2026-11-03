const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const vm = require('node:vm');
const { test } = require('node:test');

test('YouTube harvest keeps stream tokens scoped to their video and visitor', async () => {
  const messages = [];
  let onMessage;
  const window = {
    location: { href: 'https://www.youtube.com/watch?v=jNQXAC9IVRw' },
    ytcfg: { get: () => 'visitor-a' },
    postMessage: (message) => messages.push(message),
    addEventListener: (_, listener) => { onMessage = listener; },
    fetch: async () => 'original response',
  };
  function XMLHttpRequest() {}
  XMLHttpRequest.prototype.open = function () {};
  XMLHttpRequest.prototype.send = function () {};
  vm.runInNewContext(readFileSync(require.resolve('../yt-inject.js'), 'utf8'), {
    window, XMLHttpRequest, URL, ArrayBuffer, Uint8Array, TextEncoder, TextDecoder,
    Blob, Response, Date, btoa, setInterval: () => 1, clearInterval() {},
  });
  await window.fetch('/youtubei/v1/player', { body: JSON.stringify({
    videoId: 'jNQXAC9IVRw', context: { client: { visitorData: 'visitor-a' } },
    serviceIntegrityDimensions: { poToken: 'player-only' },
  }) });
  await new Promise(setImmediate);
  assert.equal(messages.at(-1).playerToken, 'player-only');
  assert.equal(messages.at(-1).poToken, null);

  assert.equal(await window.fetch('https://r1.googlevideo.com/videoplayback?pot=gvs-a'), 'original response');
  assert.equal(messages.at(-1).poToken, 'gvs-a');
  assert.equal(messages.at(-1).videoId, 'jNQXAC9IVRw');
  const capturedAt = messages.at(-1).ts;
  onMessage({ source: window, data: { source: 'zoltraak-yt-auth-request' } });
  assert.equal(messages.at(-1).poToken, 'gvs-a');
  assert.equal(messages.at(-1).ts, capturedAt);
  await window.fetch('/youtubei/v1/player', { body: JSON.stringify({
    videoId: 'jNQXAC9IVRw', serviceIntegrityDimensions: { poToken: 'another-player-token' },
  }) });
  await new Promise(setImmediate);
  assert.equal(messages.at(-1).ts, capturedAt);

  window.location.href = 'https://www.youtube.com/watch?v=9FtpzAqIPIU';
  await window.fetch('/youtubei/v1/player', { body: JSON.stringify({
    videoId: '9FtpzAqIPIU', context: { client: { visitorData: 'visitor-b' } },
  }) });
  await new Promise(setImmediate);
  assert.equal(messages.at(-1).poToken, null);
  assert.equal(messages.at(-1).ts, 0);
  assert.equal(messages.at(-1).videoId, '9FtpzAqIPIU');

  const token = new Uint8Array(16).fill(42);
  const body = new Uint8Array([154, 1, 18, 18, 16, ...token]);
  await window.fetch('https://r1.googlevideo.com/videoplayback', { method: 'POST', body });
  await new Promise(setImmediate);
  assert.equal(messages.at(-1).poToken, Buffer.from(token).toString('base64url') + '==');
  assert.equal(messages.at(-1).visitorData, 'visitor-b');
});
