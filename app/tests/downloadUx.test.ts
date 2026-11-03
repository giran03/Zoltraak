import test from "node:test";
import assert from "node:assert/strict";
import { addLinks, reviewLinks, recoveryFor, normalizeView, moveColumn, sortDownloads, groupDownloads } from "../src/lib/downloadUx.ts";
import type { QueueItem } from "../src/lib/snapshots.ts";

const row = (id: string, extra: Partial<QueueItem> = {}): QueueItem => ({ id, filename: id, url: `https://example.test/${id}`, directory: "/downloads", state: "downloading", downloadedBytes: 0, totalBytes: null, connections: 1, bytesPerSecond: 0, etaSeconds: null, priority: 0, error: null, ...extra });

test("batch review distinguishes canonical duplicates, existing downloads, comments and invalid URLs", () => {
  const result = reviewLinks('# ignored comment\nHTTPS://example.test/a https://example.test/a https://example.test/b broken ftp://example.test/file', ['https://example.test/b']);
  assert.deepEqual(result.map(entry => entry.status), ['ready', 'duplicate', 'existing', 'invalid', 'invalid']);
  assert.equal(result[0]?.url, 'https://example.test/a');
  assert.equal(reviewLinks('https:example.test/a')[0]?.status, 'invalid');
});

test("batch addition continues after a rejection and preserves each actual failure", async () => {
  const calls: string[] = [];
  const result = await addLinks(['a', 'b', 'a', 'c'], async url => { calls.push(url); if (url === 'b') throw new Error('Directory is read-only'); return 'id'; });
  assert.deepEqual(calls, ['a', 'b', 'c']);
  assert.deepEqual(result, { added: ['a', 'c'], failed: [{ url: 'b', error: 'Directory is read-only' }] });
});

test("recovery selects file, sign-in, extraction and network actions without guessing music extraction", () => {
  assert.equal(recoveryFor(row('file', { error: 'Access is denied (os error 5)' })).kind, 'folder');
  assert.equal(recoveryFor(row('signin', { error: 'server returned 403' })).kind, 'signin');
  assert.equal(recoveryFor(row('video', { isVideo: true, error: 'No supported video formats' })).kind, 'extract');
  assert.equal(recoveryFor(row('music', { isMusic: true, error: 'No supported audio formats' })).kind, 'retry');
  assert.equal(recoveryFor(row('network', { error: 'Connection timed out' })).kind, 'retry');
});

test("saved views validate stale data and preserve a unique column order", () => {
  const view = normalizeView({ query: 'clip', filter: 'videos', sort: 'name', direction: 'asc', density: 'comfortable', columnOrder: ['rate', 'size', 'rate', 'unknown'] });
  assert.deepEqual(view.columnOrder.slice(0, 2), ['rate', 'size']);
  assert.equal(new Set(view.columnOrder).size, view.columnOrder.length);
  assert.equal(view.filter, 'videos');
  assert.equal(normalizeView({ filter: 'obsolete', sort: 'nonsense', query: 99 }).sort, 'date');
  assert.equal(normalizeView(null).query, '');
  assert.equal(normalizeView(null).groupBy, 'none');
  assert.equal(normalizeView({ groupBy: 'obsolete' }).groupBy, 'none');
  assert.equal(normalizeView({ groupBy: 'source' }).groupBy, 'source');
  assert.deepEqual(moveColumn(['aspect', 'rate', 'size'], 'rate', -1), ['rate', 'aspect', 'size']);
  assert.deepEqual(moveColumn(['rate', 'size'], 'rate', -1), ['rate', 'size']);
});

test("file type groups use actual output paths, case-insensitive extensions, and preserve row sorting", () => {
  const items = [row('video-title', { isVideo: true, path: 'C:/Downloads/video.MP4' }), row('clip2.mp4'), row('archive.zip'), row('stream', { isVideo: true }), row('unknown')];
  const groups = groupDownloads(items, 'type');
  assert.deepEqual(groups.map(group => group.label), ['MP4', 'No extension', 'Video', 'ZIP']);
  assert.deepEqual(groups[0]?.items.map(item => item.id), ['video-title', 'clip2.mp4']);
  assert.deepEqual(groupDownloads(items, 'none')[0]?.items, items);
  assert.deepEqual(groupDownloads([], 'status'), []);
  assert.equal(items[0]?.id, 'video-title', 'grouping must not mutate its input');
});

test("status groups prioritize active work, then waiting, scheduled, paused, failures and finished", () => {
  const states = ['finished', 'failed', 'paused', 'scheduled', 'waiting', 'downloading'] as const;
  const groups = groupDownloads(states.map(state => row(state, { state })), 'status');
  assert.deepEqual(groups.map(group => group.label), ['Downloading', 'Waiting', 'Scheduled', 'Paused', 'Failed', 'Finished']);
});

test("source and destination groups tolerate missing data and normalize Windows folder paths", () => {
  const items = [row('a', { url: 'https://EXAMPLE.test/a', directory: 'C:\\Downloads\\' }), row('b', { url: 'https://example.test/b', directory: 'c:/downloads' }), row('c', { url: 'not a URL', directory: '', path: 'D:/Music/song.mp3' }), row('d', { url: '', directory: '' })];
  const source = groupDownloads(items, 'source');
  assert.deepEqual(source.map(group => group.items.map(item => item.id)), [['a', 'b'], ['c', 'd']]);
  const folders = groupDownloads(items, 'folder');
  assert.deepEqual(folders.map(group => group.items.map(item => item.id)), [['a', 'b'], ['c'], ['d']]);
  assert.equal(folders[1]?.label, 'D:/Music/');
  assert.equal(folders[2]?.label, 'No destination');
  assert.equal(groupDownloads([row('a', { directory: '/Music' }), row('b', { directory: '/music' })], 'folder').length, 2, 'POSIX paths remain case sensitive');
});

test("sorts use numeric names, real dates and progress, leaving unknown sizes last in either direction", () => {
  const items = [row('clip10', { totalBytes: 10, createdAt: '2026-10-01T00:00:00Z', downloadedBytes: 5 }), row('clip2', { totalBytes: 20, createdAt: '2026-09-30T00:00:00Z', downloadedBytes: 15 }), row('unknown')];
  assert.deepEqual(sortDownloads(items, { sort: 'name', direction: 'asc' }).map(item => item.id), ['clip2', 'clip10', 'unknown']);
  assert.deepEqual(sortDownloads(items, { sort: 'size', direction: 'desc' }).map(item => item.id), ['clip2', 'clip10', 'unknown']);
  assert.deepEqual(sortDownloads(items, { sort: 'size', direction: 'asc' }).map(item => item.id), ['clip10', 'clip2', 'unknown']);
  assert.deepEqual(sortDownloads(items, { sort: 'date', direction: 'desc' }).map(item => item.id), ['clip10', 'clip2', 'unknown']);
  assert.deepEqual(sortDownloads(items, { sort: 'progress', direction: 'desc' }).map(item => item.id), ['clip2', 'clip10', 'unknown']);
  assert.equal(items[0]?.id, 'clip10', 'sorting must not mutate the queue');
});
