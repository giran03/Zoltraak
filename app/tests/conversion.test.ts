import assert from "node:assert/strict";
import { test } from "node:test";
import { CONVERSION_FORMATS, CONVERSION_PRESET_LIBRARY, adaptSettingsToFormat, compareJob, conversionPreset, defaultConversionSettings, describeSelectOption, estimateFullBytes, groupBySettings, groupSelectOptions, nameTemplateError, outputFieldSupport, parseTimestamp, presetSupportsFormat, previewOutputName, resultingSize, supportsSampleRate, supportsTargetSize } from "../src/lib/conversion.ts";
import { FULL_CROP, MIN_CROP_SIDE, MIN_TRIM_SECONDS, aspectCrop, cropFromStage, cropToStage, frameTransform, isFullCrop, moveCrop, moveTrimHandle, relativeOrientation, resizeCrop, stageDimensions } from "../src/lib/cropMath.ts";

test("timestamps accept precise seconds and timecodes and reject invalid segments", () => {
  assert.equal(parseTimestamp("01:23.450"), 83.45);
  assert.equal(parseTimestamp("01:02:03.125"), 3723.125);
  assert.equal(parseTimestamp(" 2.5 "), 2.5);
  assert.equal(parseTimestamp(""), null);
  for (const value of ["1:60", "-1", "NaN", "1:2:3:4", "01::03", "x"]) assert.ok(Number.isNaN(parseTimestamp(value)), value);
});

test("presets specify encoding when quality or bitrate is intentional", () => {
  assert.equal(defaultConversionSettings().mode, "auto");
  assert.equal(conversionPreset("quality", "mp4").mode, "encode");
  const gif = conversionPreset("gif", "mp4");
  assert.equal(gif.format, "gif"); assert.equal(gif.durationSeconds, 5); assert.equal(gif.endSeconds, null);
  const voice = conversionPreset("voice", "mp3");
  assert.equal(voice.format, "opus"); assert.equal(voice.channels, 1); assert.equal(voice.normalize, true);
});

test("AAC and Apple Lossless have distinct format choices", () => {
  assert.ok(CONVERSION_FORMATS.some((format) => format.value === "m4a" && format.label.includes("AAC")));
  assert.ok(CONVERSION_FORMATS.some((format) => format.value === "alac" && format.label.includes("Lossless")));
  assert.equal(new Set(CONVERSION_FORMATS.map((format) => format.value)).size, CONVERSION_FORMATS.length);
});

test("preset library entries are unique, encode, and carry usable settings", () => {
  const ids = CONVERSION_PRESET_LIBRARY.map((entry) => entry.id);
  assert.equal(new Set(ids).size, ids.length);
  for (const entry of CONVERSION_PRESET_LIBRARY) {
    const settings = conversionPreset(entry.id, "mp4");
    assert.equal(settings.mode, "encode", entry.id);
    assert.equal(settings.format, entry.settings.format, entry.id);
    assert.ok(CONVERSION_FORMATS.some((format) => format.value === settings.format), entry.id);
    if (settings.targetSizeMb !== null) assert.ok(supportsTargetSize(settings.format), entry.id);
  }
  assert.equal(conversionPreset("discord-10", "mkv").targetSizeMb, 10);
  assert.equal(conversionPreset("unknown", "mkv").format, "mkv");
});

test("filename patterns validate and preview the way the engine names files", () => {
  assert.equal(nameTemplateError("{name}-{format}"), null);
  for (const bad of ["", "   ", "{nam}", "{name", "name}", "{}", "x".repeat(121)]) assert.notEqual(nameTemplateError(bad), null, bad);
  const now = new Date(2026, 0, 2, 3, 4, 5);
  assert.equal(previewOutputName("{name}-small", "clip.final.mov", "mp4"), "clip.final-small.mp4");
  assert.equal(previewOutputName("{date}_{time}", "a.mp4", "gif", now), "2026-01-02_03-04-05.gif");
  assert.equal(previewOutputName("a/b:c", "x.mp4", "mp3"), "a_b_c.mp3");
  assert.equal(previewOutputName("con", "x.mp4", "mp3"), "_con.mp3");
  assert.equal(previewOutputName("..", "keep.mp4", "mp3"), "keep.mp3");
});

test("crop geometry keeps boxes inside the picture and above the minimum size", () => {
  const box = { x: 0.2, y: 0.2, width: 0.4, height: 0.4 };
  assert.deepEqual(moveCrop(box, 5, -5), { x: 0.6, y: 0, width: 0.4, height: 0.4 });
  const grown = resizeCrop(box, "se", 2, 2);
  assert.deepEqual(grown, { x: 0.2, y: 0.2, width: 0.8, height: 0.8 });
  const flipped = resizeCrop(box, "nw", 0.9, 0.9);
  assert.ok(flipped.x >= 0.6 - 1e-9 && flipped.width >= MIN_CROP_SIDE - 1e-9, "dragging past the anchor flips the box");
  const tiny = resizeCrop(box, "se", 0.2, 0.2);
  assert.ok(tiny.width >= MIN_CROP_SIDE - 1e-9 && tiny.height >= MIN_CROP_SIDE - 1e-9);
  const corner = resizeCrop({ x: 0.95, y: 0.95, width: 0.05, height: 0.05 }, "se", 1, 1);
  assert.ok(corner.x + corner.width <= 1 + 1e-9 && corner.width >= MIN_CROP_SIDE - 1e-9);
  assert.ok(isFullCrop(null) && isFullCrop(FULL_CROP) && !isFullCrop(box));
});

test("aspect crops are centered, as large as fit, and measured in pixels", () => {
  const square = aspectCrop(1920, 1080, 1);
  assert.ok(Math.abs(square.width * 1920 - square.height * 1080) < 1e-6);
  assert.equal(square.height, 1);
  assert.ok(Math.abs(square.x - (1 - square.width) / 2) < 1e-9);
  const wide = aspectCrop(1000, 1000, 2);
  assert.equal(wide.width, 1);
  assert.ok(Math.abs(wide.height - 0.5) < 1e-9);
  assert.deepEqual(aspectCrop(0, 10, 1), FULL_CROP);
});

test("trim handles cannot cross each other or leave the file", () => {
  const range = { start: 2, end: 8 };
  assert.deepEqual(moveTrimHandle(range, "start", -3, 10), { start: 0, end: 8 });
  assert.equal(moveTrimHandle(range, "start", 9, 10).start, 8 - MIN_TRIM_SECONDS);
  assert.equal(moveTrimHandle(range, "end", 99, 10).end, 10);
  assert.equal(moveTrimHandle(range, "end", 0, 10).end, 2 + MIN_TRIM_SECONDS);
});

test("outputs are compared, grouped and sized from their recipes", () => {
  const base = defaultConversionSettings();
  const groups = groupBySettings(["a", "b", "c"], (path) => path === "b" ? { ...base, quality: 40 } : base);
  assert.deepEqual(groups.map((group) => group.paths), [["a", "c"], ["b"]]);
  const job = { state: "finished", inputBytes: 1000, outputBytes: 250, source: { videoCodec: "h264", audioCodec: "aac", width: 1920, height: 1080 }, outputInfo: { videoCodec: "vp9", audioCodec: "opus", width: 1280, height: 720 } };
  const change = compareJob(job as never);
  assert.equal(change?.savedPercent, 75);
  assert.equal(change?.from, "h264 1920×1080");
  assert.equal(change?.to, "vp9 1280×720");
  assert.equal(compareJob({ ...job, state: "failed" } as never), null);
  assert.equal(compareJob({ ...job, outputBytes: 2000 } as never)?.savedPercent, -100);
  assert.equal(estimateFullBytes(500, 2, 60), 15000);
  assert.equal(estimateFullBytes(500, 2, null), null);
  const size = resultingSize({ width: 1920, height: 1080 }, { ...base, crop: { x: 0, y: 0, width: 0.5, height: 1 }, rotate: 90, width: 400, height: null });
  assert.deepEqual(size, { width: 400, height: 356 }); // 960×1080 after the crop, 1080×960 after the turn
  assert.equal(resultingSize({ width: null, height: null }, base), null);
});

test("output field support and format adaptation disable unsupported options cleanly", () => {
  const mp4 = outputFieldSupport(defaultConversionSettings("mp4"));
  assert.equal(mp4.streamCopy, true);
  assert.equal(mp4.quality, true);
  assert.equal(mp4.targetSize, true);
  assert.equal(mp4.background, true);
  assert.equal(mp4.dither, false);
  assert.equal(mp4.loopCount, false);
  assert.equal(mp4.losslessWebp, false);

  const gif = outputFieldSupport(defaultConversionSettings("gif"));
  assert.equal(gif.streamCopy, false);
  assert.equal(gif.quality, false);
  assert.equal(gif.targetSize, false);
  assert.equal(gif.background, false);
  assert.equal(gif.dither, true);
  assert.equal(gif.loopCount, true);
  assert.equal(gif.audioBitrate, false);
  assert.equal(gif.audioTrack, false);
  assert.equal(gif.audioProcessing, false);
  assert.equal(gif.mute, false);

  const mp3 = outputFieldSupport(defaultConversionSettings("mp3"));
  assert.equal(mp3.streamCopy, true);
  assert.equal(mp3.quality, false);
  assert.equal(mp3.dimensions, false);
  assert.equal(mp3.fps, false);
  assert.equal(mp3.audioBitrate, true);
  assert.equal(mp3.cropAndOrientation, false);

  const mutedMp4 = outputFieldSupport({ ...defaultConversionSettings("mp4"), mute: true });
  assert.equal(mutedMp4.audioBitrate, false);
  assert.equal(mutedMp4.audioTrack, false);
  assert.equal(mutedMp4.audioProcessing, false);

  const copyMp4 = outputFieldSupport({ ...defaultConversionSettings("mp4"), mode: "copy" });
  assert.equal(copyMp4.quality, false);
  assert.equal(copyMp4.dimensions, false);
  assert.equal(copyMp4.cropAndOrientation, false);

  assert.equal(presetSupportsFormat("gif", "gif"), true);
  assert.equal(presetSupportsFormat("gif", "mp4"), false);
  assert.equal(presetSupportsFormat("discord-10", "mp4"), true);
  assert.equal(presetSupportsFormat("discord-10", "gif"), false);
  assert.equal(supportsSampleRate("opus", 48000), true);
  assert.equal(supportsSampleRate("opus", 44100), false);

  const adaptedAudio = adaptSettingsToFormat(
    { ...defaultConversionSettings("mp4"), mute: true, rotate: 90, flipHorizontal: true, crop: { x: 0.1, y: 0.1, width: 0.5, height: 0.5 }, targetSizeMb: 10 },
    "flac",
  );
  assert.equal(adaptedAudio.mute, false);
  assert.equal(adaptedAudio.rotate, 0);
  assert.equal(adaptedAudio.flipHorizontal, false);
  assert.equal(adaptedAudio.crop, null);
  assert.equal(adaptedAudio.targetSizeMb, null);
});

test("stage crop and frame orientation transforms round-trip across rotations and flips", () => {
  const source: typeof FULL_CROP = { x: 0.1, y: 0.2, width: 0.3, height: 0.4 };
  for (const rotate of [0, 90, 180, 270] as const) {
    for (const flipHorizontal of [false, true]) {
      for (const flipVertical of [false, true]) {
        const stage = cropToStage(source, rotate, flipHorizontal, flipVertical);
        const restored = cropFromStage(stage, rotate, flipHorizontal, flipVertical);
        assert.ok(Math.abs(restored.x - source.x) < 1e-9);
        assert.ok(Math.abs(restored.y - source.y) < 1e-9);
        assert.ok(Math.abs(restored.width - source.width) < 1e-9);
        assert.ok(Math.abs(restored.height - source.height) < 1e-9);
      }
    }
  }
  assert.deepEqual(stageDimensions(160, 120, 90), { width: 120, height: 160 });
  assert.equal(frameTransform(0, false, false, 160, 120), undefined);
  assert.match(frameTransform(90, true, false, 160, 120) ?? "", /scale\(-1, 1\) rotate\(90deg\)/);
  const delta = relativeOrientation({ rotate: 90, flipHorizontal: false, flipVertical: false }, { rotate: 180, flipHorizontal: false, flipVertical: false });
  assert.deepEqual(delta, { rotate: 90, flipHorizontal: false, flipVertical: false });
});

test("dropdown options parse groups, file extensions and secondary details cleanly", () => {
  const mp4 = describeSelectOption({ value: "mp4", label: "Video · MP4 · H.264 / AAC", group: "Video", badge: ".mp4" });
  assert.equal(mp4.primary, "MP4");
  assert.equal(mp4.badge, "(.mp4)");
  assert.equal(mp4.detail, "H.264 / AAC");
  assert.equal(mp4.group, "Video");

  const aac = describeSelectOption({ value: "m4a", label: "AAC (.m4a)", group: "Lossy" });
  assert.equal(aac.primary, "AAC");
  assert.equal(aac.badge, "(.m4a)");
  assert.equal(aac.detail, undefined);

  const grouped = groupSelectOptions([mp4, aac]);
  assert.equal(grouped.length, 2);
  assert.equal(grouped[0].group, "Video");
  assert.equal(grouped[1].group, "Lossy");
});
