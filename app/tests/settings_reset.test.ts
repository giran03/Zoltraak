import test from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_ENGINE_SETTINGS } from "../src/lib/engine.ts";
import { DEFAULT_APPEARANCE, readAppearance } from "../src/lib/useAppearance.ts";
import { TAB_OUTLINES } from "../src/lib/settingsOutlines.ts";

test("DEFAULT_ENGINE_SETTINGS matches zoltraak_core factory specifications", () => {
  assert.equal(DEFAULT_ENGINE_SETTINGS.bandwidthLimitBytesPerSec, null);
  assert.equal(DEFAULT_ENGINE_SETTINGS.defaultConnections, null);
  assert.equal(DEFAULT_ENGINE_SETTINGS.defaultDirectory, null);
  assert.equal(DEFAULT_ENGINE_SETTINGS.desktopNotifications, true);
  assert.equal(DEFAULT_ENGINE_SETTINGS.playSoundOnComplete, true);
  assert.equal(DEFAULT_ENGINE_SETTINGS.startOnLaunch, false);
  assert.equal(DEFAULT_ENGINE_SETTINGS.closeToTray, true);
  assert.equal(DEFAULT_ENGINE_SETTINGS.minimizeToTray, false);
  assert.equal(DEFAULT_ENGINE_SETTINGS.deleteFilesOnRemove, true);
  assert.deepEqual(DEFAULT_ENGINE_SETTINGS.schedule, {
    enabled: false,
    startMinutes: 0,
    stopMinutes: 0,
  });
  assert.deepEqual(DEFAULT_ENGINE_SETTINGS.onComplete, { type: "none" });
});

test("DEFAULT_APPEARANCE has confetti disabled and notification limit set to 25", () => {
  assert.equal(DEFAULT_APPEARANCE.confettiOnComplete, false);
  assert.equal(DEFAULT_APPEARANCE.confettiSfx, false);
  assert.equal(DEFAULT_APPEARANCE.notificationLimit, 25);
  assert.equal(DEFAULT_APPEARANCE.themeMode, "system");
  assert.equal(DEFAULT_APPEARANCE.activePresetId, "classic-dispatch");
});

test("readAppearance applies false confetti defaults in empty environment", () => {
  const settings = readAppearance();
  assert.equal(settings.confettiOnComplete, false);
  assert.equal(settings.confettiSfx, false);
  assert.equal(settings.notificationLimit, 25);
});

test("TAB_OUTLINES contains factory reset in General and reset appearance in Appearance", () => {
  const generalIds = TAB_OUTLINES.General.map((item) => item.id);
  assert.ok(generalIds.includes("section-factory-reset"), "General outline missing section-factory-reset");
  assert.equal(
    TAB_OUTLINES.General.find((item) => item.id === "section-factory-reset")?.label,
    "Factory Reset",
  );

  const appearanceIds = TAB_OUTLINES.Appearance.map((item) => item.id);
  assert.ok(appearanceIds.includes("section-reset-appearance"), "Appearance outline missing section-reset-appearance");
  assert.equal(
    TAB_OUTLINES.Appearance.find((item) => item.id === "section-reset-appearance")?.label,
    "Reset Appearance",
  );
  assert.ok(
    !appearanceIds.includes("section-factory-reset"),
    "Appearance outline should not contain section-factory-reset after rename",
  );
});
