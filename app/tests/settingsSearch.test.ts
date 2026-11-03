import test from "node:test";
import assert from "node:assert/strict";
import { searchSettings } from "../src/lib/settingsSearch.ts";
import { TAB_OUTLINES } from "../src/lib/settingsOutlines.ts";

test("settings search finds controls across unopened tabs and accepts synonyms", () => {
  assert.equal(searchSettings("delete")[0]?.control, "Delete local file with trash button");
  assert.equal(searchSettings("sign-ins")[0]?.tab, "Extension");
  assert.ok(searchSettings("folder").some(result => result.id === "section-engine-defaults"));
  assert.ok(searchSettings("sound").some(result => result.control === "Completion sound"));
  assert.ok(searchSettings("  BROWSER token ").some(result => result.label === "Extension pairing"));
  assert.deepEqual(searchSettings("unmatched-query"), []);
  assert.deepEqual(searchSettings("   "), []);
});
test("each search destination exists in its tab's outline", () => {
  for (const query of ["delete", "sound", "folder", "extension", "install", "columns", "health", "density", "confetti", "history", "dark"]) {
    for (const match of searchSettings(query)) assert.ok(TAB_OUTLINES[match.tab].some(section => section.id === match.id), match.id);
  }
});
