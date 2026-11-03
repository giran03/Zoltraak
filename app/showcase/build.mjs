// Builds the real app against the mock engine and folds it, with the director shell, into one HTML file.
const BS = String.fromCharCode(92);
const jsLiteral = (s) => JSON.stringify(s).split("<").join(BS + "u003c").split(String.fromCharCode(0x2028)).join(BS + "u2028").split(String.fromCharCode(0x2029)).join(BS + "u2029");
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync, readdirSync, mkdirSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
const app = resolve(import.meta.dirname, "..");
execSync("npx vite build -c vite.showcase.config.ts", { cwd: app, stdio: "inherit" });
const dist = resolve(app, "../scratch/showcase-build/assets");
const css = readFileSync(join(dist, readdirSync(dist).find(f => f.endsWith(".css"))), "utf8");
const js = readFileSync(join(dist, readdirSync(dist).find(f => f.endsWith(".js"))), "utf8");
const appHtml = `<!doctype html><html lang="en"><head><meta charset="UTF-8"><meta name="color-scheme" content="light dark"><meta name="viewport" content="width=device-width, initial-scale=1.0"><title>Zoltraak</title><style>${css}</style></head><body><div id="root"></div><script>${js.replace(/<\/script/gi, "<\/script")}</script></body></html>`;
// Fonts for the shell itself are the app's own latin subsets, taken from the bundled CSS.
const fontFaces = (css.match(/@font-face\{[^}]*\}/g) || []).filter(f => /latin-wdth-normal|latin-wdth|latin(?!-ext)/.test(f) && !/latin-ext/.test(f) && !/italic/.test(f)).join("\n");
// The browser page: a generic video site running the extension's real content script against a stubbed chrome.* API.
const ext = resolve(app, "../extension");
const b64 = (f) => "data:font/woff2;base64," + readFileSync(join(ext, f)).toString("base64");
const safe = (t) => t.split("</script").join("<" + BS + "/script");
const poster = "data:image/svg+xml;utf8," + encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="900" height="506" viewBox="0 0 900 506"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#27465f"/><stop offset="1" stop-color="#3a2a4a"/></linearGradient></defs><rect width="900" height="506" fill="url(#g)"/><circle cx="640" cy="150" r="80" fill="#0b0e0d" opacity=".3"/><path d="M0 390L270 280L500 360L720 250L900 340V506H0Z" fill="#0b0e0d" opacity=".45"/><circle cx="450" cy="253" r="38" fill="#fff" opacity=".85"/><path d="M438 232L472 253L438 274Z" fill="#27465f"/></svg>');
const webHtml = readFileSync(join(import.meta.dirname, "web.template.html"), "utf8")
  .replace("__POSTER__", () => poster)
  .replace("__FONTS__", () => JSON.stringify({ "fonts/archivo-subset.woff2": b64("fonts/archivo-subset.woff2"), "fonts/martian-mono-subset.woff2": b64("fonts/martian-mono-subset.woff2") }))
  .replace("__GENERIC__", () => safe(readFileSync(join(ext, "generic-media.js"), "utf8")))
  .replace("__IDENTITY__", () => safe(readFileSync(join(ext, "video-identity.js"), "utf8")))
  // The only adaptation: the page runs in an iframe, so "am I the top frame" is answered as yes.
  .replace("__CONTENT__", () => safe(readFileSync(join(ext, "content.js"), "utf8").split("window.self === window.top").join("true").split("window.self !== window.top").join("false").split("window.top === window.self").join("true")));
const shell = readFileSync(join(import.meta.dirname, "shell.html"), "utf8")
  .replace("/*__FONTS__*/", fontFaces)
  .replace('/*__APP_HTML__*/""', () => jsLiteral(appHtml))
  .replace('/*__WEB_HTML__*/""', () => jsLiteral(webHtml));
const out = resolve(app, "../docs/showcase.html");
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, shell);
console.log("wrote", out, (shell.length / 1e6).toFixed(2), "MB");
