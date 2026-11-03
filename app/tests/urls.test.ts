import assert from "node:assert/strict";
import { test } from "node:test";
import { parseHtmlUrls, parseUrls } from "../src/lib/urls.ts";

test("URL intake accepts only complete HTTP links and removes duplicates", () => {
  assert.deepEqual(
    parseUrls("https://example.com/a ftp://example.com/no javascript:alert(1) https://example.com/a"),
    ["https://example.com/a"],
  );
  assert.deepEqual(parseUrls("example.com/no-scheme not-a-url"), []);
  assert.deepEqual(parseUrls("HTTPS://EXAMPLE.COM/file.zip"), ["https://example.com/file.zip"]);
});

test("browser image and anchor drags yield their remote targets", () => {
  assert.deepEqual(
    parseHtmlUrls(
      '<a href="https://example.com/page"><img src="https://cdn.example.com/photo.jpg"></a>',
    ),
    ["https://cdn.example.com/photo.jpg"],
  );
});
