// Dev-only static server for previewing the showcase build.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
const root = process.argv[2] ?? ".";
const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".png": "image/png", ".svg": "image/svg+xml", ".woff2": "font/woff2" };
createServer(async (req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^[\\/]+/, "");
  try { const data = await readFile(join(root, path || "index.html")); res.setHeader("Content-Type", types[extname(path)] ?? "application/octet-stream"); res.setHeader("Cache-Control", "no-store"); res.end(data); }
  catch { res.writeHead(404); res.end("not found"); }
}).listen(1460, "127.0.0.1");
