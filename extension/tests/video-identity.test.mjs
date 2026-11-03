import assert from 'node:assert/strict';
import { test } from 'node:test';
import '../video-identity.js';

const { sameMedia, itemIdentity, verifyAndScoreCandidate, createPlayerTracker,
  filterFormatsByVideoContext } = globalThis.ZoltraakVideoIdentity;
const metadata = (url, duration = 10) => ({ duration,
  formats: [{ videoUrl: url, width: 1280, height: 720, hasAudio: true }] });

test('extensionless and signed generic URLs retain all content query fields', () => {
  assert.equal(sameMedia('https://cdn.test/play?id=A', 'https://cdn.test/play?id=B'), false);
  assert.equal(sameMedia('https://cdn.test/play?token=A', 'https://cdn.test/play?token=B'), false);
  assert.equal(sameMedia('https://cdn.test/play?id=A&quality=720', 'https://cdn.test/play?quality=720&id=A'), true);
});

test('YouTube normalization preserves content and rendition identity', () => {
  const url = 'https://r1.googlevideo.com/videoplayback?id=A&itag=137&expire=1&sig=old';
  assert.equal(sameMedia(url, url.replace('expire=1&sig=old', 'expire=2&sig=new')), true);
  assert.equal(sameMedia(url, url.replace('id=A', 'id=B')), false);
  assert.equal(sameMedia(url, url.replace('itag=137', 'itag=140')), false);
});

test('item IDs are namespaced and recognize query-based watch URLs', () => {
  assert.equal(itemIdentity('https://www.youtube.com/watch?v=clipA'), 'youtube:clipA');
  assert.equal(itemIdentity('https://youtu.be/clipA'), 'youtube:clipA');
  assert.equal(itemIdentity('https://m.facebook.com/watch/?v=123'), 'facebook:123');
  assert.equal(itemIdentity('https://x.com/user/status/123'), 'x:123');
  assert.equal(itemIdentity('https://cdn.test/video/123'), null);
});

test('Instagram audio and collection routes are not post identities', () => {
  assert.equal(itemIdentity('https://www.instagram.com/reels/audio/28686481800976139/'), null);
  assert.equal(itemIdentity('https://www.instagram.com/reels/liked/'), null);
  assert.equal(itemIdentity('https://www.instagram.com/reel/DdKrjQHNY86/?utm_source=ig_web_copy_link'), 'instagram:DdKrjQHNY86');
  assert.equal(itemIdentity('https://www.instagram.com/p/DdKrjQHNY86/c/123/'), 'instagram:DdKrjQHNY86');
});

test('identified reels keep selections through looping, seeking, and same-item reloads', () => {
  const tracker = createPlayerTracker();
  const player = { currentTime: 8 };
  const binding = tracker.observe(player, 'clip-A', false, 'instagram:A');
  player.currentTime = 0;
  assert.equal(tracker.matches(player, 'clip-A', binding, 'instagram:A'), true);
  tracker.observe(player, 'clip-A', true, 'instagram:A');
  assert.equal(tracker.matches(player, 'clip-A', binding, 'instagram:A'), true);
  assert.equal(tracker.matches(player, 'clip-B', binding, 'instagram:B'), false);
});

test('equal-duration adjacent clips and document-wide timings never establish identity', () => {
  const url = 'https://cdn.test/play?id=B';
  const result = verifyAndScoreCandidate(metadata(url), url, {
    duration: 10, width: 1280, height: 720, recentMediaUrls: [url], currentSrc: 'blob:player',
  });
  assert.equal(result.valid, true);
  assert.equal(result.strong, false);
  assert.deepEqual(result.evidence, []);
});

test('exact player source establishes identity, CSS cropping does not reject it', () => {
  const url = 'https://cdn.test/play?id=A';
  const result = verifyAndScoreCandidate(metadata(url), url, { currentSrc: url, aspectRatio: 0.5 });
  assert.equal(result.valid, true);
  assert.equal(result.strong, true);
});

test('adjacent permalink is rejected even when duration and source hints match', () => {
  const url = 'https://youtube.com/watch?v=B';
  assert.equal(verifyAndScoreCandidate(metadata(url), url, {
    permalink: 'https://youtube.com/watch?v=A', currentSrc: url, duration: 10,
  }).valid, false);
});

test('same soundtrack does not pull neighboring video into matched formats', () => {
  const formats = [
    { videoUrl: 'https://cdn.test/a.mp4', audioUrl: 'https://cdn.test/shared.m4a' },
    { videoUrl: 'https://cdn.test/b.mp4', audioUrl: 'https://cdn.test/shared.m4a' },
  ];
  assert.deepEqual(filterFormatsByVideoContext(formats, { currentSrc: formats[0].videoUrl }), [formats[0]]);
});

test('reused player invalidates stale results on clip changes and reloads', () => {
  const tracker = createPlayerTracker();
  const player = {};
  const first = tracker.observe(player, 'clip-A');
  assert.equal(tracker.matches(player, 'clip-A', first), true);
  const second = tracker.observe(player, 'clip-B');
  assert.equal(first.playerInstanceId, second.playerInstanceId);
  assert.equal(tracker.matches(player, 'clip-B', first), false);
  assert.equal(tracker.matches(player, 'clip-B', second), true);
  tracker.observe(player, 'clip-B', true);
  assert.equal(tracker.matches(player, 'clip-B', second), false);
  assert.notEqual(tracker.observe({}, 'clip-B').playerInstanceId, second.playerInstanceId);
});

test('MSE playback reset invalidates a selection even when the blob and metadata stay the same', () => {
  const tracker = createPlayerTracker();
  const player = { currentTime: 8 };
  const binding = tracker.observe(player, 'same-blob');
  player.currentTime = 0;
  assert.equal(tracker.matches(player, 'same-blob', binding), false);
});


test('social post recovery accepts supported post routes only', () => {
  const { socialPostUrl } = globalThis.ZoltraakVideoIdentity;
  assert.equal(socialPostUrl('https://www.instagram.com/reels/ABC123/?utm_source=copy'), 'https://www.instagram.com/reel/ABC123/');
  assert.equal(socialPostUrl('https://twitter.com/user/status/123/video/1'), 'https://x.com/user/status/123');
  for (const url of ['https://www.facebook.com/reel/123', 'https://www.facebook.com/user/posts/123',
    'https://m.facebook.com/story.php?story_fbid=123&id=456', 'https://www.reddit.com/r/test/comments/abc123/title/',
    'https://www.tiktok.com/@user/video/123']) assert.ok(socialPostUrl(url), url);
  for (const url of ['https://www.instagram.com/', 'https://www.instagram.com/reels/audio/123/',
    'https://instagram.com.evil.test/p/ABC123/', 'https://x.com/home', 'https://facebook.com/user',
    'https://youtube.com/watch?v=123', 'https://cdn.test/clip.mp4', 'javascript:alert(1)',
    'https://secret@instagram.com/p/ABC123/']) assert.equal(socialPostUrl(url), null, url);
});

test('Facebook posts and share tokens identify the requested item without guessing a video ID', () => {
  const { socialPostUrl, contextPostTarget } = globalThis.ZoltraakVideoIdentity;
  const post = 'https://www.facebook.com/juliarusziell/posts/pfbid0umUguDQtAMzFeybyTtq4P4Sgno5Nwgfrwj5hkaHx9ZikgRoj81D8mQhTwq5v51hml';
  const share = 'https://www.facebook.com/share/p/1DmzsRpa8N/';
  assert.equal(socialPostUrl(share), share);
  assert.equal(itemIdentity(post), 'facebook:post:pfbid0umUguDQtAMzFeybyTtq4P4Sgno5Nwgfrwj5hkaHx9ZikgRoj81D8mQhTwq5v51hml');
  assert.equal(itemIdentity(share), 'facebook:share:p:1DmzsRpa8N');
  for (const link of ['https://facebook.com/juliarusziell', 'https://cdn.test/photo.jpg', 'https://facebook.com/photo/?fbid=123']) {
    assert.equal(contextPostTarget(link, post, 'https://facebook.com/'), post);
  }
  assert.equal(contextPostTarget(share, post, 'https://facebook.com/'), share);
  assert.equal(contextPostTarget(null, null, 'https://facebook.com/'), null);
  assert.equal(verifyAndScoreCandidate({ ...metadata('https://video.fbcdn.net/v.mp4'), webpageUrl: post }, post,
    { permalink: post }).strong, true);
  assert.equal(verifyAndScoreCandidate({ ...metadata('https://video.fbcdn.net/v.mp4'), webpageUrl: share }, share,
    { permalink: post }).valid, false);
});

test('Meta byte slices resolve to full tracks without rewriting signed queries', () => {
  const { completeMediaUrl } = globalThis.ZoltraakVideoIdentity;
  const full = 'https://video.xx.fbcdn.net/v.mp4?oh=a%2Fb+z&_nc_cat=1&oe=123';
  const slice = `${full}&bytestart=0&byteend=131071`;
  assert.equal(completeMediaUrl(slice), full);
  assert.equal(sameMedia(slice, full), true);
  assert.equal(sameMedia(full, full.replace('v.mp4', 'audio.mp4')), false);
  const unknown = slice.replace('video.xx.fbcdn.net', 'cdn.test');
  assert.equal(completeMediaUrl(unknown), unknown);
});


test('TikTok identities match video, photo, embed and shared query routes', () => {
  const { itemIdentity, contextPostTarget } = globalThis.ZoltraakVideoIdentity;
  for (const url of [
    'https://www.tiktok.com/@creator/video/7671362008486251790?is_from_webapp=1',
    'https://www.tiktok.com/@creator/photo/7671362008486251790',
    'https://www.tiktok.com/embed/v2/7671362008486251790',
    'https://www.tiktok.com/player/v1/7671362008486251790',
    'https://www.tiktok.com/?itemId=7671362008486251790',
  ]) assert.equal(itemIdentity(url), 'tiktok:7671362008486251790', url);
  assert.equal(contextPostTarget('https://www.tiktok.com/@creator',
    'https://www.tiktok.com/@creator/video/7671362008486251790', 'https://www.tiktok.com/foryou'),
    'https://www.tiktok.com/@creator/video/7671362008486251790');
});


test('TikTok short share links remain usable post targets without invented IDs', () => {
  const { socialPostUrl, itemIdentity, contextPostTarget } = globalThis.ZoltraakVideoIdentity;
  for (const url of ['https://vm.tiktok.com/ZMAbc123/', 'https://vt.tiktok.com/ZMAbc123/',
    'https://www.tiktok.com/t/ZMAbc123/']) {
    assert.equal(socialPostUrl(url + '?sender_device=pc'), url);
    assert.equal(contextPostTarget(url, null, 'https://www.tiktok.com/foryou'), url);
    assert.equal(itemIdentity(url), null);
  }
  assert.equal(socialPostUrl('https://vm.tiktok.com/'), null);
  assert.equal(socialPostUrl('https://vm.tiktok.com.evil.test/ZMAbc123/'), null);
});
