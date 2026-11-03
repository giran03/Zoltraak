import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8").replace(/\r\n/g, "\n");
const read = (file: string) => readFileSync(new URL(`../src/components/${file}`, import.meta.url), "utf8");

test("the highlight slides on the animation tokens, so Interface Motion governs it", () => {
  const rule = css.slice(css.indexOf('.slide-indicator[data-ready="true"]'));
  const block = rule.slice(0, rule.indexOf("}"));
  for (const property of ["transform", "width", "height"]) {
    assert.ok(block.includes(property + " var(--duration-fast) var(--ease-smooth-out)"), property);
  }
  // Subtle shortens the token, and reduced zeroes every transition.
  assert.match(css, /:root\[data-motion="subtle"\] \{[^}]*--duration-fast: 140ms/);
  assert.match(css, /:root\[data-motion="reduced"\] \*,[^{]*\{[^}]*transition-duration: 0\.01ms/);
});

test("the first placement is a cut: transitions only arm after it", () => {
  const unarmed = css.slice(css.indexOf(".slide-indicator {"), css.indexOf(".slide-indicator[data-visible"));
  assert.doesNotMatch(unarmed, /transition/);
});

test("every tab-like group in the app uses the sliding highlight", () => {
  const expected: Record<string, string[]> = {
    "DownloadsView.tsx": ['aria-label="Filter by state"'],
    "SettingsShell.tsx": ['aria-label="Settings sections"', "section navigation"],
    "PanelHeader.tsx": ['data-tauri-drag-region aria-label="Primary"'],
    "MusicView.tsx": ['aria-label="Music job state"'],
    "VideoView.tsx": ['aria-label="Video job state"'],
    "AppearanceView.tsx": ['aria-label="Panel lighting mode"'],
  };
  for (const [file, labels] of Object.entries(expected)) {
    const source = read(file);
    for (const label of labels) {
      const at = source.indexOf(label);
      assert.ok(at >= 0, `${file} lost "${label}"`);
      assert.match(source.slice(Math.max(0, at - 200), at), /<SlideGroup/, `${file}: "${label}" is not a SlideGroup`);
    }
  }
});

test("a chosen item draws no fill of its own, or it would show through the sliding one", () => {
  for (const file of ["DownloadsView.tsx", "SettingsShell.tsx", "MusicView.tsx", "VideoView.tsx"]) {
    assert.doesNotMatch(read(file), /\? "bg-route text-on-signal"/, file);
  }
});
