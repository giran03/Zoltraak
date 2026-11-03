import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import vm from 'node:vm';

const context = vm.createContext({ URL, location: { href: 'https://www.facebook.com/' } });
vm.runInContext(await readFile(new URL('../facebook-post.js', import.meta.url), 'utf8'), context);
const fb = context.ZoltraakFacebook;
const url = 'https://www.facebook.com/user/posts/pfbidExample';
const photo = (id) => ({ __typename: 'Photo', id: String(id), url: `https://www.facebook.com/photo/?fbid=${id}&set=pcb.12345`, image: { uri: `https://scontent.fbcdn.net/${id}.jpg`, width: 100, height: 100 } });
const story = (count = 7) => ({ __typename: 'Story', post_id: '12345', permalink_url: url, attachments: [{ all_subattachments: { count, nodes: [1,2,3,4,5].map(photo) } }] });
const plain = (v) => JSON.parse(JSON.stringify(v));

test('five previews merge with post-scoped viewer responses in gallery order', () => {
  const docs = [story(), { mediaSetToken: 'pcb.12345', media: photo(5), nextMediaAfterNodeId: { id: '6' } },
    { mediaSetToken: 'pcb.12345', media: photo(7) },
    { mediaSetToken: 'pcb.12345', media: { ...photo(6), viewer_image: { uri: 'https://scontent.fbcdn.net/original6.jpg', width: 2000, height: 2000 } }, nextMediaAfterNodeId: { id: '7' } }];
  const result = fb.select(docs, url);
  assert.equal(result.expected, 7);
  assert.deepEqual(plain(result.ids), ['1','2','3','4','5','6','7']);
  assert.equal(result.documents[0].attachments[0].all_subattachments.nodes[5].viewer_image.width, 2000);
});

test('comments, neighboring stories, unrelated galleries and arbitrary CDN hosts are excluded', () => {
  const post = story(); post.comments = [photo(99)];
  post.attachments.push({ nested: { __typename: 'Story', post_id: '99999', attachments: [{ media: photo(88) }] } });
  const result = fb.select([fb.compact(post), { mediaSetToken: 'pcb.99999', media: { ...photo(77), url: 'https://www.facebook.com/photo/?fbid=77&set=pcb.99999' } }], url);
  assert.deepEqual(plain(result.ids), ['1','2','3','4','5']);
  assert.equal(fb.select([{ data: { node_v2: { ...story(), permalink_url: 'https://www.facebook.com/user/posts/pfbidOther' } } }], url), null);
  const bad = story(1); bad.attachments[0].all_subattachments.nodes = [{ ...photo(1), image: { uri: 'https://fbcdn.net.evil.test/private.jpg' } }];
  assert.equal(fb.select([bad], url), null);
});

test('Relay envelopes parse without retaining tokens or raw request data', () => {
  const raw = { data: story(), token: 'secret', cookie: 'secret', request: { body: 'secret' } };
  for (const text of [JSON.stringify(raw), `for (;;);${JSON.stringify(raw)}`, `${JSON.stringify(raw)}\n${JSON.stringify({ data: { mediaSetToken: 'pcb.12345', media: photo(6) } })}`]) {
    const docs = fb.parse(text);
    assert.ok(docs.length);
    assert.ok(!JSON.stringify(docs).includes('secret'));
  }
  assert.deepEqual(plain(fb.parse('x'.repeat(2 * 1024 * 1024 + 1))), []);
});

test('conflicting counts and an absent count never claim a complete gallery', () => {
  assert.equal(fb.select([story(7), story(8)], url).expected, null);
  const missing = story(); delete missing.attachments[0].all_subattachments.count;
  assert.equal(fb.select([missing], url).expected, null);
});

test('pfbid aliases require the exact open permalink and its requested Story', () => {
  const doc = { data: { node_v2: story() } }; delete doc.data.node_v2.permalink_url;
  assert.equal(fb.select([doc], url, [doc]), null, 'a feed cannot infer the alias');
  context.location.href = url;
  try {
    const selected = fb.select([doc], url, [doc]);
    assert.equal(selected.documents[0].post_id, '12345');
    assert.equal(fb.select([doc], url), null, 'arbitrary cached envelopes are not alias evidence');
    assert.equal(fb.select([doc, { data: { node_v2: { ...doc.data.node_v2, post_id: '99999' } } }], url, [doc, { data: { node_v2: { ...doc.data.node_v2, post_id: '99999' } } }]), null, 'ambiguous requested Stories are rejected');
  } finally { context.location.href = 'https://www.facebook.com/'; }
});
