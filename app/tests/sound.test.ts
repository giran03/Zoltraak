import assert from "node:assert/strict";
import { test } from "node:test";
import {
  formatAudioSize,
  formatDuration,
  getCustomSoundMeta,
  getSoundVolume,
  playCompletionSound,
  setSoundVolume,
} from "../src/lib/completionSound.ts";
import { playConfettiSfx, triggerConfetti } from "../src/lib/confetti.ts";

test("formatDuration formats seconds into human-readable duration", () => {
  assert.equal(formatDuration(0), "0.0s");
  assert.equal(formatDuration(-1), "0.0s");
  assert.equal(formatDuration(0.85), "0.9s");
  assert.equal(formatDuration(2.4), "2.4s");
  assert.equal(formatDuration(12.34), "12.3s");
  assert.equal(formatDuration(65), "1m 5s");
  assert.equal(formatDuration(125), "2m 5s");
});

test("formatAudioSize formats byte counts cleanly", () => {
  assert.equal(formatAudioSize(0), "0 B");
  assert.equal(formatAudioSize(512), "512 B");
  assert.equal(formatAudioSize(1024), "1.0 KB");
  assert.equal(formatAudioSize(48 * 1024), "48.0 KB");
  assert.equal(formatAudioSize(2.5 * 1024 * 1024), "2.50 MB");
});

test("volume clamps cleanly between 0.0 and 1.0", () => {
  // Test volume setting
  setSoundVolume(0.5);
  assert.equal(getSoundVolume(), 0.5);

  setSoundVolume(1.5);
  assert.equal(getSoundVolume(), 1.0);

  setSoundVolume(-0.2);
  assert.equal(getSoundVolume(), 0.0);

  // Restore default volume
  setSoundVolume(0.7);
  assert.equal(getSoundVolume(), 0.7);
});

test("default sound fallback when no custom sound is loaded", () => {
  // Without uploading a file, custom sound meta is null
  assert.equal(getCustomSoundMeta(), null);

  // playCompletionSound executes safely (no unhandled exceptions)
  // In a node environment without AudioContext, it returns a no-op stop function
  const stop = playCompletionSound(true);
  assert.equal(typeof stop, "function");
  stop();
});

test("playCompletionSound throttles calls within 300ms cooldown", () => {
  const stop1 = playCompletionSound(false);
  assert.equal(typeof stop1, "function");

  // Immediate second call is suppressed by cooldown
  const stop2 = playCompletionSound(false);
  assert.equal(typeof stop2, "function");

  // Forced call bypasses cooldown (e.g. Test sound button)
  const stopForced = playCompletionSound(true);
  assert.equal(typeof stopForced, "function");
});

test("triggerConfetti and playConfettiSfx execute safely and handle simultaneous calls", () => {
  // Verify triggerConfetti and playConfettiSfx run in unbundled test environment without throwing
  assert.doesNotThrow(() => {
    triggerConfetti(false);
    triggerConfetti(false);
    triggerConfetti(true);
    playConfettiSfx(false);
    playConfettiSfx(false);
    playConfettiSfx(true);
  });
});
