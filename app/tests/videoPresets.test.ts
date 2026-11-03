import assert from "node:assert/strict";
import { test } from "node:test";
import { choosePreset } from "../src/lib/videoPresets.ts";
import type { VideoFormat } from "../src/lib/engine.ts";
const f = (id: string, height: number, options: Partial<VideoFormat> = {}) => ({ id, height, container: "mp4", videoCodec: "avc1.64001f", audioCodec: "mp4a.40.2", hasAudio: true, ...options } as VideoFormat);
test("presets select actual matching formats and prefer known audio", () => {
  const formats = [f("unknown", 2160, {hasAudio: null}), f("known",1080), f("balanced",720), f("silent",1440,{hasAudio:false})];
  assert.equal(choosePreset(formats,"best")?.id,"known");
  assert.equal(choosePreset(formats,"balanced")?.id,"balanced");
  assert.equal(choosePreset(formats,"compatible")?.id,"known");
  assert.equal(choosePreset([f("vp9",1080,{videoCodec:"vp9",container:"webm"})],"compatible"),undefined);
  assert.equal(choosePreset([f("hd",1080)],"balanced"),undefined);
  assert.equal(choosePreset([f("silent",1080,{hasAudio:false,audioCodec:null})],"compatible"),undefined);
});

test("presets require a clip choice for multi-video posts", () => {
  const formats = [f("clip-2-high", 1080), f("clip-2-low", 720), f("clip-3-high", 1440)];
  assert.equal(choosePreset(formats, "best"), undefined);
  assert.equal(choosePreset(formats, "balanced"), undefined);
  assert.equal(choosePreset(formats, "compatible"), undefined);
  assert.equal(choosePreset(formats.slice(0, 2), "best")?.id, "clip-2-high");
});
