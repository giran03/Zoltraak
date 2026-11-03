import assert from "node:assert/strict";
import { test } from "node:test";
import {
  OVERLAY_PRESETS,
  DEFAULT_POSITION,
  previewPlacementStyles,
  sanitizePosition,
} from "../placement-controller.js";

test("OVERLAY_PRESETS contains all required presets", () => {
  assert.equal(OVERLAY_PRESETS.length, 10);
  const ids = OVERLAY_PRESETS.map((p) => p.id);

  // Inside Playback
  assert.ok(ids.includes("top-right"));
  assert.ok(ids.includes("top-left"));
  assert.ok(ids.includes("top-center"));
  assert.ok(ids.includes("bottom-right"));
  assert.ok(ids.includes("bottom-left"));
  assert.ok(ids.includes("bottom-center"));

  // Outside Playback
  assert.ok(ids.includes("outside-top-right"));
  assert.ok(ids.includes("outside-top-left"));
  assert.ok(ids.includes("outside-bottom-right"));
  assert.ok(ids.includes("outside-bottom-left"));

  for (const preset of OVERLAY_PRESETS) {
    assert.ok(preset.label.length > 0);
    assert.ok(preset.desc.length > 0);
    assert.ok(preset.group === "inside" || preset.group === "outside");
  }
});

test("preview placement keeps offsets independent from scale and pins the selected anchor", () => {
  const smallTopRight = previewPlacementStyles({ preset: "top-right", offsetX: 7, offsetY: -3, scale: 50 });
  const largeTopRight = previewPlacementStyles({ preset: "top-right", offsetX: 7, offsetY: -3, scale: 150 });
  assert.equal(smallTopRight.right, "calc(12px - 7px)");
  assert.equal(smallTopRight.top, "calc(12px + -3px)");
  assert.equal(smallTopRight.transformOrigin, "right top");
  assert.equal(smallTopRight.scale, "0.5");
  assert.deepEqual({ ...smallTopRight, scale: largeTopRight.scale }, largeTopRight);

  const bottomRight = previewPlacementStyles({ preset: "bottom-right", scale: 150 });
  assert.equal(bottomRight.bottom, "calc(48px - 0px)");
  assert.equal(bottomRight.transformOrigin, "right bottom");

  const outsideTop = previewPlacementStyles({ preset: "outside-top-left", scale: 150 });
  assert.equal(outsideTop.bottom, "calc(100% + 8px - 0px)");
  assert.equal(outsideTop.transformOrigin, "left bottom");
});

test("sanitizePosition clamps and sanitizes coordinates", () => {
  // Default fallback
  const def = sanitizePosition(null);
  assert.deepEqual(def, DEFAULT_POSITION);

  // Valid inputs
  const valid = sanitizePosition({ preset: "outside-top-right", offsetX: 25, offsetY: -12 });
  assert.equal(valid.preset, "outside-top-right");
  assert.equal(valid.offsetX, 25);
  assert.equal(valid.offsetY, -12);
  assert.equal(valid.scale, 100);

  const scaled = sanitizePosition({ preset: "top-right", scale: 135 });
  assert.equal(scaled.scale, 135);
  assert.equal(sanitizePosition({ scale: 20 }).scale, 50);
  assert.equal(sanitizePosition({ scale: 0 }).scale, 50);
  assert.equal(sanitizePosition({ scale: 500 }).scale, 200);

  // Clamping outside bounds [-150, 150]
  const clampedMax = sanitizePosition({ preset: "bottom-left", offsetX: 300, offsetY: 999 });
  assert.equal(clampedMax.offsetX, 150);
  assert.equal(clampedMax.offsetY, 150);

  const clampedMin = sanitizePosition({ preset: "top-left", offsetX: -500, offsetY: -200 });
  assert.equal(clampedMin.offsetX, -150);
  assert.equal(clampedMin.offsetY, -150);

  // Invalid preset falls back to default preset
  const invalidPreset = sanitizePosition({ preset: "invalid-corner", offsetX: 10, offsetY: 10 });
  assert.equal(invalidPreset.preset, "top-right");
  assert.equal(invalidPreset.offsetX, 10);
  assert.equal(invalidPreset.offsetY, 10);

  // Non-numeric offsets fall back to 0
  const nanOffset = sanitizePosition({ preset: "top-right", offsetX: "abc", offsetY: null });
  assert.equal(nanOffset.offsetX, 0);
  assert.equal(nanOffset.offsetY, 0);
});
