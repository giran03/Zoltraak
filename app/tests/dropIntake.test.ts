import assert from "node:assert/strict";
import { test } from "node:test";
import { htmlConfirmMessage, screenDroppedLinks } from "../src/lib/dropIntake.ts";

const inspect = async (url: string) => url.includes("page")
  ? { filename: "page.html", contentType: "text/html", isHtml: true }
  : { filename: "cat.png", contentType: "image/png", isHtml: false };

test("dropped file links queue without asking", async () => {
  let asked = false;
  const urls = await screenDroppedLinks(["https://h/cat"], inspect, () => { asked = true; return false; });
  assert.deepEqual(urls, ["https://h/cat"]);
  assert.equal(asked, false);
});

test("a dropped web page is confirmed before it is queued", async () => {
  const links = ["https://h/cat", "https://h/page"];
  let message = "";
  assert.deepEqual(await screenDroppedLinks(links, inspect, text => { message = text; return true; }), links);
  assert.match(message, /HTML page/);
  assert.match(message, /page\.html/);
  assert.deepEqual(await screenDroppedLinks(links, inspect, () => false), ["https://h/cat"]);
});

test("a link that cannot be inspected is let through", async () => {
  const failing = async () => { throw new Error("offline"); };
  assert.deepEqual(await screenDroppedLinks(["https://h/x"], failing, () => false), ["https://h/x"]);
});

test("several pages are asked about together", () => {
  const message = htmlConfirmMessage([{ url: "a", filename: "a.html" }, { url: "b", filename: "b.html" }]);
  assert.match(message, /These 2 links will download HTML pages/);
});
