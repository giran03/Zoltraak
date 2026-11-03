import assert from "node:assert/strict";
import { test } from "node:test";
import { imageCrop, messagePreview, profileDraft, profileImage } from "../src/lib/discordProfile.ts";

test("profile drafts distinguish missing fields, removals, and reverts", () => {
  const base = { username: "Bot", avatar: "old-image", banner: null };
  let draft = profileDraft(base, {}, "avatar", null);
  assert.deepEqual(draft, { avatar: null });
  draft = profileDraft(base, draft, "username", "New bot");
  draft = profileDraft(base, draft, "username", "Bot");
  assert.deepEqual(draft, { avatar: null });
  assert.deepEqual(profileDraft(base, draft, "banner", null), { avatar: null });
  assert.deepEqual(profileDraft({ tags: ["download"] }, {}, "tags", ["download"]), {});
});

test("profile images use the correct global, application, and member CDN paths", () => {
  assert.equal(profileImage("global", { id: "42", avatar: "a_hash" }, "avatar"), "https://cdn.discordapp.com/avatars/42/a_hash.gif?size=1024");
  assert.equal(profileImage("application", { id: "42", icon: "hash" }, "icon"), "https://cdn.discordapp.com/app-icons/42/hash.png?size=1024");
  assert.equal(profileImage("server", { id: "42", banner: "hash" }, "banner", "7"), "https://cdn.discordapp.com/guilds/7/users/42/banners/hash.png?size=1024");
  assert.equal(profileImage("global", { avatar: null }, "avatar"), null);
});

test("crop positions stay in source bounds and use the intended aspect ratio", () => {
  for (const [width, height] of [[1600, 900], [900, 1600], [512, 512]]) {
    for (const ratio of [1, 2.5]) for (const zoom of [1, 3]) {
      const crop = imageCrop(width, height, ratio, zoom, 100, 100);
      assert.ok(crop.x >= 0 && crop.y >= 0);
      assert.ok(crop.x + crop.width <= width + 0.001);
      assert.ok(crop.y + crop.height <= height + 0.001);
      assert.ok(Math.abs(crop.width / crop.height - ratio) < 0.001);
    }
  }
});

test("message preview includes only configured mentions and source links", () => {
  const settings = { messageTemplate: "{filename} · {size}", includeSourceUrl: false, pingSender: false };
  assert.equal(messagePreview(settings as Parameters<typeof messagePreview>[0]), "sample-video.mp4 · 8.0 MiB");
  const withPing = { ...settings, pingSender: true, discordUserId: "123", pingMessageTemplate: "sent by {discord user id}", includeSourceUrl: true };
  assert.equal(messagePreview(withPing as Parameters<typeof messagePreview>[0]), "sent by <@123>\nsample-video.mp4 · 8.0 MiB\nhttps://example.com/downloads/sample-video.mp4");
});
