// Real Chromium extension worlds, network observation, frame snapshots and popup.
// PLAYWRIGHT_MODULE points to the existing playwright-core/index.mjs installation.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { cp, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const { chromium } = await import(pathToFileURL(process.env.PLAYWRIGHT_MODULE).href);
const scratch = await mkdtemp(path.join(tmpdir(), "zoltraak-generic-media-"));
const extension = path.join(scratch, "extension");
await cp("extension", extension, { recursive: true, filter: (name) => !name.includes(`${path.sep}tests`) });
const mp4 = await readFile("engine/core/tests/fixtures/verified-av.mp4");
const server = createServer((req, res) => {
  const url = new URL(req.url, "http://localhost");
  if (url.pathname === "/movie.mp4" || url.pathname === "/segment.m4s" || url.pathname === "/opaque" || url.pathname.startsWith("/dash/segment-")) {
    res.writeHead(200, { "content-type": "video/mp4", "content-length": mp4.length }); res.end(mp4); return;
  }
  if (url.pathname === "/master") { res.writeHead(200, { "content-type": "application/vnd.apple.mpegurl" }); res.end("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=300000\n/playlist\n"); return; }
  if (url.pathname === "/playlist") { res.writeHead(200, { "content-type": "application/vnd.apple.mpegurl" }); res.end("#EXTM3U\n#EXTINF:10,\n/segment.m4s\n#EXT-X-ENDLIST\n"); return; }
  if (url.pathname === "/dash-master") { res.writeHead(200, { "content-type": "application/dash+xml" }); res.end('<MPD><BaseURL>/dash/</BaseURL><Period><AdaptationSet><SegmentTemplate media="segment-$Number%05d$.m4s"/><Representation id="v"/></AdaptationSet></Period></MPD>'); return; }
  res.writeHead(200, { "content-type": "text/html" });
  res.end(url.pathname === "/frame" ? '<!doctype html><title>Embedded</title><audio id="embedded" src="/movie.mp4?frame=1"></audio>'
    : '<!doctype html><title>Generic playback fixture</title><body style="background:#181818;color:white"><video id="one" muted loop src="/movie.mp4?clip=one" style="width:300px"></video><video id="two" muted loop src="/movie.mp4?clip=two" style="width:300px"></video><audio id="audio" src="/movie.mp4?clip=audio"></audio><div id="shadow"></div><div id="late"></div><iframe src="http://localhost:PORT/frame"></iframe></body>'.replace("PORT", server.address().port));
});
await new Promise((resolve) => server.listen(0, "0.0.0.0", resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const manifest = JSON.parse(await readFile(path.join(extension, "manifest.json"), "utf8"));
delete manifest.browser_specific_settings;
manifest.permissions = manifest.permissions.filter((p) => p !== "webRequestBlocking");
manifest.background = { service_worker: "background.js", type: "module" };
manifest.content_scripts.push({ matches: ["http://*/*"], js: ["fixture-probe.js"], run_at: "document_idle" });
await writeFile(path.join(extension, "manifest.json"), JSON.stringify(manifest));
const bg = await readFile(path.join(extension, "background.js"), "utf8");
const names = bg.match(/import\s*\{([\s\S]+?)\}\s*from "\.\/api\.js"/)[1].split(",").map((name) => name.trim()).filter(Boolean);
names.push("setToken", "reconnect");
await writeFile(path.join(extension, "api.js"), names.map((name) => {
  if (name === "ENGINE_ORIGIN") return 'export const ENGINE_ORIGIN = "http://127.0.0.1:1450";';
  const body = ({ health: "false", ensurePaired: "true", verify: "true", canUseBrowserCookies: "false",
    getToken: '"fixture"', cookieHeader: "null", youtubeSessionCookie: "null",
    getSnapshot: "({ok:true,items:[],videoJobs:[]})", extensionStatusCursor: "({reloadVersion:0,sourceRevision:0})",
    apiFetch: 'args[0].startsWith("/extension/status") ? new Promise(() => {}) : new Response(JSON.stringify([]))',
    analyzeVideo: '({ok:true,metadata:{title:"Selected media",webpageUrl:args[0],formats:[{id:"direct",container:"mp4",videoUrl:args[0],hasAudio:true}]}})',
    bestCompleteFormat: '({format:args[0][0],complete:true})',
    startVideoDownload: '(await chrome.storage.local.set({fixtureDownload:args[0]}),{ok:true,id:"fixture-job"})',
  })[name] || "({ok:true})";
  return `export const ${name} = async (...args) => ${body};`;
}).join("\n"));
// bestCompleteFormat is synchronous in production.
await writeFile(path.join(extension, "api.js"), (await readFile(path.join(extension, "api.js"), "utf8")).replace("const bestCompleteFormat = async", "const bestCompleteFormat ="));
await writeFile(path.join(extension, "fixture-probe.js"), `
document.documentElement.dataset.fixtureReady = 'yes';
document.addEventListener('fixture-snapshot', () => {
  document.documentElement.dataset.fixtureSnapshot = JSON.stringify(ZoltraakGenericMedia.snapshot());
});
document.addEventListener('fixture-encrypted', () => document.querySelector('#one').dispatchEvent(new Event('encrypted')));
`);
const context = await chromium.launchPersistentContext(path.join(scratch, "profile"), { channel: "chromium", headless: true,
  args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`] });
const errors = [];
context.setDefaultTimeout(12000);
try {
  const worker = context.serviceWorkers()[0] || await context.waitForEvent("serviceworker");
  const extensionId = new URL(worker.url()).hostname;
  const page = await context.newPage(); page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(base); await page.waitForFunction(() => document.documentElement.dataset.fixtureReady === "yes");
  await page.evaluate(async () => {
    document.querySelector("#shadow").attachShadow({ mode: "open" }).innerHTML = '<video src="/movie.mp4?clip=shadow"></video>';
    await Promise.all([document.querySelector("#one").play(), document.querySelector("#two").play()]);
    document.querySelector("#audio").muted = true; await document.querySelector("#audio").play();
    await fetch("/opaque");
  });
  const snapshot = async () => {
    await page.evaluate(() => document.dispatchEvent(new Event("fixture-snapshot")));
    return JSON.parse(await page.locator("html").getAttribute("data-fixture-snapshot"));
  };
  await page.waitForFunction(() => { document.dispatchEvent(new Event("fixture-snapshot")); return JSON.parse(document.documentElement.dataset.fixtureSnapshot).players.length === 4; });
  const first = await snapshot();
  assert.equal(first.players.filter((p) => p.state === "playing").length, 3);
  assert.equal(first.players.filter((p) => p.type === "audio").length, 1);
  assert.equal(new Set(first.players.map((p) => p.sources[0].url)).size, 4);
  console.log("PASS two simultaneous videos, invisible audio and an open shadow root");
  await page.evaluate(() => document.querySelector("#late").attachShadow({ mode: "open" }).innerHTML = '<audio src="/movie.mp4?clip=late"></audio>');
  await page.waitForFunction(() => { document.dispatchEvent(new Event("fixture-snapshot")); return JSON.parse(document.documentElement.dataset.fixtureSnapshot).players.length === 5; });
  await page.evaluate(async () => {
    await fetch("/master"); await fetch("/playlist");
    const media = document.createElement("video"), source = new MediaSource();
    media.id = "mse"; media.src = URL.createObjectURL(source); document.body.append(media);
    await new Promise((resolve) => source.addEventListener("sourceopen", resolve, { once: true }));
    source.addSourceBuffer('video/mp4; codecs="avc1.42E01E,mp4a.40.2"').appendBuffer(await (await fetch("/segment.m4s")).arrayBuffer());
  });
  await page.waitForFunction(() => { document.dispatchEvent(new Event("fixture-snapshot")); return JSON.parse(document.documentElement.dataset.fixtureSnapshot).players.some((p) => p.sources.some((s) => s.evidence === "mse-manifest")); });
  const mse = (await snapshot()).players.find((p) => p.sources.some((s) => s.evidence === "mse-manifest"));
  assert.deepEqual(mse.sources.map((s) => s.url), [`${base}/master`]);
  console.log("PASS MSE buffers map through rendition to master playlist without offering segments");
  await page.evaluate(async () => {
    await fetch("/dash-master");
    const media = document.createElement("video"), source = new MediaSource();
    media.id = "dash"; media.src = URL.createObjectURL(source); document.body.append(media);
    await new Promise((resolve) => source.addEventListener("sourceopen", resolve, { once: true }));
    source.addSourceBuffer('video/mp4; codecs="avc1.42E01E,mp4a.40.2"').appendBuffer(await (await fetch("/dash/segment-00001.m4s")).arrayBuffer());
    const file = document.createElement("video"); file.id = "blob-file";
    file.src = URL.createObjectURL(await (await fetch("/movie.mp4?clip=blob-file")).blob()); document.body.append(file);
    const frame = document.createElement("iframe"); frame.srcdoc = '<title>Related blank frame</title><audio src="/movie.mp4?clip=srcdoc"></audio>'; document.body.append(frame);
  });
  await page.waitForFunction(() => { document.dispatchEvent(new Event("fixture-snapshot")); const list = JSON.parse(document.documentElement.dataset.fixtureSnapshot).players;
    return list.some((p) => p.sources.some((s) => s.kind === "dash" && s.evidence === "mse-manifest")) && list.some((p) => p.sources.some((s) => s.evidence === "mse-buffer")); });
  const extended = await snapshot();
  assert.ok(extended.players.some((p) => p.sources.some((s) => s.url === `${base}/dash-master`)));
  assert.ok(extended.players.some((p) => p.sources.some((s) => s.url === `${base}/movie.mp4?clip=blob-file`)));
  console.log("PASS DASH numbered-template ownership and complete response blobs");
  await page.evaluate(() => document.dispatchEvent(new Event("fixture-encrypted")));
  assert.equal((await snapshot()).players.find((p) => p.sources.some((s) => s.url.includes("clip=one"))).status, "protected");
  await page.evaluate(() => { const media = document.querySelector("#two"); media.src = "/movie.mp4?clip=replacement"; media.load(); });
  const replaced = (await snapshot()).players.find((p) => p.id === first.players.find((p) => p.sources[0].url.includes("clip=two")).id);
  assert.ok(replaced.generation > first.players.find((p) => p.id === replaced.id).generation);
  assert.ok(replaced.sources.every((s) => !s.url.includes("clip=two")));
  console.log("PASS encrypted playback and source replacement invalidate old selections");
  const tabId = await worker.evaluate(async (url) => (await chrome.tabs.query({})).find((t) => t.url === url).id, `${base}/`);
  const popup = await context.newPage(); popup.on("pageerror", (error) => errors.push(error.message));
  await popup.addInitScript(({ tabId, base }) => { if (location.protocol === "chrome-extension:") chrome.tabs.query = async () => [{ id: tabId, url: base, title: "Generic playback fixture" }]; }, { tabId, base });
  await popup.goto(`chrome-extension://${extensionId}/popup.html`);
  console.log("Popup registry:", await popup.locator("#mediaStatus").textContent());
  await popup.waitForFunction(() => document.querySelectorAll(".media-player").length >= 7);
  assert.ok(await popup.getByText("Embedded", { exact: false }).count());
  await popup.waitForFunction(() => document.getElementById("mediaList").textContent.includes("Related blank frame"));
  assert.ok(await popup.getByText("Other detected streams", { exact: false }).count());
  const row = popup.locator(`.media-player[data-key="0:${replaced.id}"]`);
  await row.evaluate((element) => { element.open = true; });
  await row.getByRole("button", { name: "Download source" }).click();
  await popup.waitForFunction(() => document.getElementById("mediaStatus").textContent.startsWith("Queued"));
  const queued = await worker.evaluate(async () => (await chrome.storage.local.get("fixtureDownload")).fixtureDownload);
  assert.equal(queued.format.videoUrl, `${base}/movie.mp4?clip=replacement`);
  await popup.screenshot({ path: path.join(scratch, "popup-media.png") });
  assert.deepEqual(errors, []);
  console.log(`PASS cross-origin frame aggregation, unassigned sources and exact popup download\nArtifacts: ${scratch}`);
} finally { await context.close(); await new Promise((resolve) => server.close(resolve)); }
