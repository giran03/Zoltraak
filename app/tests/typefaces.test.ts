import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { TYPEFACE_PRESETS } from "../src/lib/typefaces.ts";
import { checkFontFile, fontDisplayName, normalizeCustomFont } from "../src/lib/customFont.ts";
import { DEFAULT_APPEARANCE, readAppearance } from "../src/lib/useAppearance.ts";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8").replace(/\r\n/g, "\n");

/** The two font variables a `:root` block sets, by the data-font mode it targets. */
function stacksFor(mode: string) {
  const header = mode === "default" ? ":root {\n  --ui-font-sans" : `:root[data-font="${mode}"] {`;
  const start = css.indexOf(header);
  assert.ok(start >= 0, `no stylesheet block for ${mode}`);
  const block = css.slice(start, css.indexOf("}", start));
  return {
    sans: block.match(/--ui-font-sans:\s*([^;]+);/)?.[1],
    mono: block.match(/--ui-font-mono:\s*([^;]+);/)?.[1],
  };
}

test("every typeface preset previews the exact stacks the stylesheet applies", () => {
  const base = stacksFor("default");
  for (const preset of TYPEFACE_PRESETS) {
    const applied = stacksFor(preset.id);
    assert.equal(applied.sans ?? base.sans, preset.sans, `${preset.id} text`);
    assert.equal(applied.mono ?? base.mono, preset.mono, `${preset.id} figures`);
  }
});

test("the utilities read the variables, so the setting reaches font-sans and font-mono", () => {
  assert.match(css, /--font-sans:\s*var\(--ui-font-sans\)/);
  assert.match(css, /--font-mono:\s*var\(--ui-font-mono\)/);
  assert.match(css, /@import "@fontsource-variable\/inter\/index\.css"/);
});

test("font files are checked by name, size and signature", () => {
  const woff2 = new Uint8Array([0x77, 0x4f, 0x46, 0x32]);
  assert.equal(checkFontFile("Face.woff2", 1000, woff2), null);
  assert.equal(checkFontFile("Face.ttf", 1000, new Uint8Array([0, 1, 0, 0])), null);
  assert.equal(checkFontFile("Face.otf", 1000, new Uint8Array([0x4f, 0x54, 0x54, 0x4f])), null);
  assert.match(checkFontFile("Face.png", 1000, woff2) ?? "", /Choose a/);
  assert.match(checkFontFile("Face.woff2", 0, woff2) ?? "", /empty/);
  assert.match(checkFontFile("Face.woff2", 9 * 1024 * 1024, woff2) ?? "", /up to 8 MB/);
  assert.match(checkFontFile("Face.woff2", 1000, new Uint8Array([1, 2, 3, 4])) ?? "", /does not look like a font/);
});

test("a family name is made readable from the file name", () => {
  assert.equal(fontDisplayName("Fira-Sans_Medium.woff2"), "Fira Sans Medium");
  assert.equal(fontDisplayName(String.raw`C:\fonts\Inter.ttf`), "Inter");
  assert.equal(fontDisplayName(".woff2"), "Custom typeface");
});

test("stored custom font settings are restricted to what the app understands", () => {
  assert.equal(normalizeCustomFont(null), null);
  assert.equal(normalizeCustomFont({ name: "   " }), null);
  assert.deepEqual(normalizeCustomFont({ name: " Face ", fileName: "f.woff2", bytes: 12.4, applyToMono: true, extra: 1 }),
    { name: "Face", fileName: "f.woff2", bytes: 12, applyToMono: true });
});

test("an unusable saved typeface falls back to the default", () => {
  const stored = new Map<string, string>();
  (globalThis as unknown as { localStorage: Storage }).localStorage = {
    getItem: (key: string) => stored.get(key) ?? null,
    setItem: (key: string, value: string) => void stored.set(key, value),
    removeItem: (key: string) => void stored.delete(key),
  } as Storage;
  const key = "zoltraak.appearance.v1";
  stored.set(key, JSON.stringify({ fontMode: "custom" }));
  assert.equal(readAppearance().fontMode, "default");
  stored.set(key, JSON.stringify({ fontMode: "custom", customFont: { name: "Face", fileName: "f.ttf", bytes: 5 } }));
  assert.equal(readAppearance().fontMode, "custom");
  stored.set(key, JSON.stringify({ fontMode: "papyrus" }));
  assert.equal(readAppearance().fontMode, DEFAULT_APPEARANCE.fontMode);
});
