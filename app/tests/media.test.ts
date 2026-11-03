import assert from "node:assert/strict";
import { test } from "node:test";
import type { MediaInfo } from "../src/lib/engine.ts";
import {
  alreadyIs,
  basename,
  describePlan,
  dirname,
  extensionIsContainer,
  extensionOf,
  isLossless,
  isMediaPath,
  isVideoPath,
  planFor,
} from "../src/lib/media.ts";

function info(video: string | null, audio: string | null, extension: string): MediaInfo {
  return {
    path: `C:\\clips\\holiday.${extension}`,
    extension,
    videoCodec: video,
    audioCodec: audio,
    width: null,
    height: null,
    durationSeconds: null,
    bytes: null,
    unprobed: false,
  };
}

test("the staged row's verdict matches what the engine will actually do", () => {
  // These four cases are the ones the screen exists to tell apart, and each
  // mirrors an assertion in the engine's own `remux::tests`.
  const vp9 = planFor(info("vp9", "opus", "webm"), "mp4");
  assert.deepEqual(vp9, { video: "encode", audio: "encode" });
  assert.equal(isLossless(vp9), false);

  const h264 = planFor(info("h264", "aac", "mkv"), "mp4");
  assert.deepEqual(h264, { video: "copy", audio: "copy" });
  assert.equal(isLossless(h264), true);

  // Matroska takes anything as it is.
  assert.equal(isLossless(planFor(info("vp9", "opus", "webm"), "mkv")), true);

  // A stream that is not there is not an encode.
  const silent = planFor(info("h264", null, "mp4"), "mov");
  assert.deepEqual(silent, { video: "copy", audio: "absent" });
  assert.equal(isLossless(silent), true);
});

test("an unprobed file is previewed as a transcode, the way the engine plans it", () => {
  // Unknown streams are not absent streams: promising a copy here would
  // promise a file that does not play.
  const unknown = { ...info(null, null, "ts"), unprobed: true };
  assert.deepEqual(planFor(unknown, "mp4"), { video: "encode", audio: "encode" });
});

test("QuickTime's uncompressed audio family is recognised as copyable", () => {
  assert.equal(isLossless(planFor(info("h264", "pcm_s16le", "avi"), "mov")), true);
  // Nowhere else, though — MP4 cannot hold it.
  assert.equal(isLossless(planFor(info("h264", "pcm_s16le", "avi"), "mp4")), false);
});

test("the verdict is phrased for whichever stream is being re-encoded", () => {
  assert.match(describePlan({ video: "copy", audio: "copy" }, "mp4"), /copies the streams/);
  assert.match(describePlan({ video: "encode", audio: "copy" }, "mp4"), /picture/);
  assert.match(describePlan({ video: "copy", audio: "encode" }, "webm"), /sound/);
  assert.match(describePlan({ video: "encode", audio: "encode" }, "mp4"), /both streams/);
});

test("a file already in the target container is recognised, .m4v included", () => {
  assert.equal(alreadyIs(info(null, null, "m4v"), "mp4"), true);
  assert.equal(alreadyIs(info(null, null, "mp4"), "mp4"), true);
  assert.equal(alreadyIs(info(null, null, "mkv"), "mp4"), false);
  assert.equal(extensionIsContainer("MOV", "mov"), true, "extensions may arrive in any case");
});

test("paths are split the same way on both separators", () => {
  assert.equal(basename("C:\\Users\\me\\Videos\\clip.webm"), "clip.webm");
  assert.equal(basename("/home/me/videos/clip.webm"), "clip.webm");
  assert.equal(dirname("C:\\Users\\me\\clip.webm"), "C:\\Users\\me");
  assert.equal(dirname("/home/me/clip.webm"), "/home/me");
  assert.equal(extensionOf("C:\\a\\Holiday.MKV"), "mkv");
  assert.equal(extensionOf("no-extension"), "");
  // A dotfile is not an extension.
  assert.equal(extensionOf(".bashrc"), "");
});

test("only media is offered, and only video counts as a clip", () => {
  assert.equal(isMediaPath("C:\\a\\song.flac"), true);
  assert.equal(isMediaPath("C:\\a\\clip.mkv"), true);
  assert.equal(isMediaPath("C:\\a\\notes.txt"), false);

  // The register's "Remux to MP4" entry hangs off this: a downloaded track
  // must not be offered a video container.
  assert.equal(isVideoPath("C:\\a\\clip.webm"), true);
  assert.equal(isVideoPath("C:\\a\\song.flac"), false);
  assert.equal(isVideoPath("C:\\a\\song.mp3"), false);
});
