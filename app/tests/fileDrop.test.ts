import assert from "node:assert/strict";
import { test } from "node:test";
import { canResolveDroppedPaths, judgeDrop, resolveDroppedPaths, SUPPORTED_FORMATS } from "../src/lib/fileDrop.ts";

test("supported media and folders are accepted; everything else is listed as unsupported", () => {
  const verdict = judgeDrop([
    { name: "clip.MP4", isDirectory: false },
    { name: "song.flac", isDirectory: false },
    { name: "photo.png", isDirectory: false },
    { name: "notes.txt", isDirectory: false },
    { name: "archive.zip", isDirectory: false },
    { name: "Holiday 2024", isDirectory: true },
    { name: "noextension", isDirectory: false },
  ]);
  assert.deepEqual(verdict.accepted.map((entry) => entry.name), ["clip.MP4", "song.flac", "photo.png", "Holiday 2024"]);
  assert.deepEqual(verdict.unsupported, ["notes.txt", "archive.zip", "noextension"]);
});

test("a folder is never judged by its name, since the engine filters what is inside", () => {
  assert.equal(judgeDrop([{ name: "backup.zip", isDirectory: true }]).unsupported.length, 0);
});

test("the caller's own objects come back, so a file can be matched to its verdict", () => {
  const file = { name: "a.mkv", isDirectory: false, marker: 7 };
  assert.equal(judgeDrop([file]).accepted[0], file);
});

test("the unsupported-file message describes what can be converted", () => {
  assert.deepEqual(SUPPORTED_FORMATS.map((group) => group.kind), ["Video", "Audio", "Images and animations"]);
});

test("outside the desktop window there is no way to read dropped paths", async () => {
  assert.equal(canResolveDroppedPaths(), false);
  assert.equal(await resolveDroppedPaths([{ name: "a.mp4" } as File]), null);
});

test("a file that already carries its path needs no round trip", async () => {
  const files = [{ name: "a.mp4", path: "C:/media/a.mp4" }, { name: "b.mp3", path: "C:/media/b.mp3" }] as unknown as File[];
  assert.deepEqual(await resolveDroppedPaths(files), ["C:/media/a.mp4", "C:/media/b.mp3"]);
});
