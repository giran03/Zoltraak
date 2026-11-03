import assert from "node:assert/strict";
import { test } from "node:test";
import { albumStatus } from "../src/lib/albumStatus.ts";
import type { ImageAlbum } from "../src/lib/engine.ts";

test("album status distinguishes complete, partial, and older engine results", () => {
  const album: ImageAlbum = { title: "Post", sourceUrl: "https://facebook.com/posts/123", uploader: null, items: [] };
  assert.equal(albumStatus(album), "Found 0 · completeness unverified");
  assert.equal(albumStatus({ ...album, completeness: "complete" }), "Found 0 media");
  assert.equal(albumStatus({ ...album, completeness: "partial", expectedCount: 27 }), "Found 0 of 27");
  assert.equal(albumStatus({ ...album, completeness: "partial" }), "Found 0 · incomplete");
  // A known count mismatch must never be hidden by a stale complete flag.
  assert.equal(albumStatus({ ...album, completeness: "complete", expectedCount: 27 }), "Found 0 of 27");
});
