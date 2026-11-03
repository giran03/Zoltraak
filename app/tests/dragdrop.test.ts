import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

test("the Tauri window passes browser image and link drops to HTML", () => {
  const config = JSON.parse(
    readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
  );
  assert.equal(config.app.windows[0].dragDropEnabled, false);
});
