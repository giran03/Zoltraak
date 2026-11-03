import assert from "node:assert/strict";
import { test } from "node:test";
import { classifyLink, routeMessage } from "../src/lib/linkKind.ts";

const section = (url: string) => classifyLink(url)?.section ?? null;

test("music services go to Music", () => {
  assert.equal(section("https://open.spotify.com/track/abc"), "Music");
  assert.equal(section("https://music.apple.com/us/album/x/1"), "Music");
  assert.equal(section("https://music.youtube.com/watch?v=abcdefghijk"), "Music");
  assert.equal(section("https://artist.bandcamp.com/album/x"), "Music");
  assert.equal(section("https://soundcloud.com/"), null);
});

test("social post permalinks go to Posts", () => {
  assert.equal(section("https://x.com/user/status/123"), "Images");
  assert.equal(section("https://twitter.com/user/status/123?s=20"), "Images");
  assert.equal(section("https://www.instagram.com/reel/Cabc_d/"), "Images");
  assert.equal(section("https://www.reddit.com/r/pics/comments/abc123/title/"), "Images");
  assert.equal(section("https://www.tiktok.com/@u/video/7123"), "Images");
  assert.equal(section("https://www.facebook.com/watch/?v=123"), "Images");
  assert.equal(section("https://x.com/user"), null);
  assert.equal(section("https://www.instagram.com/someone/"), null);
});

test("single video pages go to Video", () => {
  assert.equal(section("https://www.youtube.com/watch?v=abcdefghijk"), "Video");
  assert.equal(section("https://youtu.be/abcdefghijk"), "Video");
  assert.equal(section("https://www.youtube.com/shorts/abcdefghijk"), "Video");
  assert.equal(section("https://vimeo.com/123456"), "Video");
  assert.equal(section("https://www.youtube.com/playlist?list=PL1"), null);
  assert.equal(section("https://www.youtube.com/"), null);
});

test("plain files and unknown sites stay regular downloads", () => {
  assert.equal(section("https://example.com/file.zip"), null);
  assert.equal(section("https://example.com/watch?v=abcdefghijk"), null);
  assert.equal(section("not a url"), null);
  assert.equal(section("ftp://x.com/user/status/1"), null);
  assert.match(routeMessage(classifyLink("https://x.com/u/status/1")!), /Posts tab/);
});
