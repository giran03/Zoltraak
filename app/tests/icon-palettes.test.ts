import assert from "node:assert/strict";
import { test } from "node:test";
import { DEFAULT_APPEARANCE, DEFAULT_DARK_COLORS, DEFAULT_LIGHT_COLORS, normalizeIconPalette, readAppearance, resolvedIconPalette } from "../src/lib/useAppearance.ts";

test("icon palettes follow resolved theme and keep per-theme overrides independent", () => {
  const settings = { ...DEFAULT_APPEARANCE, iconPalettes: { dark: { useAppColors: false, background: "#112233", foreground: "#ffeecc" }, "preset:amber-crt": { useAppColors: false, background: "#332211", foreground: "#ffddaa" } } };
  assert.equal(resolvedIconPalette(settings, true).foreground, "#ffeecc");
  assert.equal(resolvedIconPalette(settings, false).foreground, DEFAULT_LIGHT_COLORS.route);
  assert.equal('background' in resolvedIconPalette(settings, true), false);
  assert.equal(resolvedIconPalette({ ...settings, themeMode: "dark" }, false).foreground, "#ffeecc");
  assert.equal(resolvedIconPalette({ ...settings, themeMode: "preset", activePresetId: "amber-crt" }, false).foreground, "#ffddaa");
  const linked = { ...settings, iconPalettes: { dark: { ...settings.iconPalettes.dark, useAppColors: true } } };
  assert.equal(resolvedIconPalette(linked, true).foreground, DEFAULT_DARK_COLORS.route);
  const glass = resolvedIconPalette({ ...settings, effects: { ...settings.effects, glass: true } }, false);
  assert.equal(glass.key, "liquid-glass");
  const preset = { ...settings, themeMode: "preset" as const, activePresetId: "user-test", userPresets: [{ id: "user-test", name: "Test", createdAt: 0, colors: DEFAULT_LIGHT_COLORS, iconPalette: settings.iconPalettes.dark }] };
  assert.equal(resolvedIconPalette(preset, false).foreground, "#ffeecc");
});
test("stored icon settings migrate old appearances and discard malformed palettes", () => {
  for (const value of [null, {}, { useAppColors: false, foreground: "red" }]) assert.equal(normalizeIconPalette(value), undefined);
  assert.deepEqual(normalizeIconPalette({ useAppColors: false, background: "#123456", foreground: "#FFFFFF" }), { useAppColors: false, foreground: "#ffffff" });
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  try {
    Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => JSON.stringify({ themeMode: "dark", iconPalettes: { dark: { useAppColors: false, background: "#ABCDEF", foreground: "#123456" }, broken: {} } }) } });
    const saved = readAppearance();
    assert.equal(saved.syncAppIcon, true);
    assert.deepEqual(Object.keys(saved.iconPalettes), ["dark"]);
    assert.deepEqual(saved.iconPalettes.dark, { useAppColors: false, foreground: "#123456" });
  } finally { if (original) Object.defineProperty(globalThis, "localStorage", original); else Reflect.deleteProperty(globalThis, "localStorage"); }
});
