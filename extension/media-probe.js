// Bundled MAIN-world probe. Observe URLs and buffer ownership; never store media bytes.
(() => {
  globalThis.ZoltraakFacebook?.installProbe();
  const key = Symbol.for("zoltraak.media.probe");
  if (window[key]) return;
  window[key] = true;
  const buffers = new WeakMap(), owners = new WeakMap(), blobs = new WeakMap(), urls = new WeakMap();
  const pending = new Set();
  const send = (payload) => window.postMessage({ channel: "zoltraak-media-probe", ...payload }, "*");
  const http = (value, base = location.href) => {
    if (typeof value !== "string" || !value.trim() || value.length > 8192) return null;
    try { const url = new URL(value, base); return /^https?:$/.test(url.protocol) ? url.href : null; } catch { return null; }
  };
  function report(source) {
    if (!blobs.has(source) || pending.has(source)) return;
    pending.add(source);
    setTimeout(() => { pending.delete(source); send({ type: "mse", blob: blobs.get(source), urls: [...(urls.get(source) || [])] }); }, 100);
  }
  function bind(buffer, url) {
    if (buffer && typeof buffer === "object" && http(url)) buffers.set(buffer, url);
  }
  function manifest(text, url, type) {
    if (typeof text !== "string" || text.length > 512 * 1024) return;
    let children = [], templates = [], kind;
    if (text.trimStart().startsWith("#EXTM3U")) {
      kind = "hls";
      for (const line of text.split(/\r?\n/)) {
        if (line.trim() && !line.startsWith("#")) children.push(http(line.trim(), url));
        for (const match of line.matchAll(/URI="([^"]+)"/g)) children.push(http(match[1], url));
      }
    } else if (/dash\+xml/i.test(type) || /<MPD[\s>]/.test(text)) {
      kind = "dash";
      const xml = new DOMParser().parseFromString(text, "application/xml");
      for (const node of xml.querySelectorAll("BaseURL,SegmentURL,Initialization")) {
        children.push(http(node.getAttribute("media") || node.getAttribute("sourceURL") || node.textContent.trim(), url));
      }
      for (const rep of [...xml.querySelectorAll("Representation")].slice(0, 64)) {
        let base = url, segmentTemplate;
        const ancestors = []; for (let node = rep; node?.nodeType === 1; node = node.parentElement) ancestors.unshift(node);
        for (const node of ancestors) {
          const child = [...node.children].find((c) => c.localName === "BaseURL");
          if (child) base = http(child.textContent.trim(), base) || base;
          segmentTemplate = [...node.children].find((c) => c.localName === "SegmentTemplate") || segmentTemplate;
        }
        const raw = segmentTemplate?.getAttribute("media");
        if (!raw) continue;
        const expanded = raw.replace(/\$RepresentationID\$/g, rep.getAttribute("id") || "")
          .replace(/\$Bandwidth\$/g, rep.getAttribute("bandwidth") || "");
        const template = http(expanded, base);
        if (template && /\$(?:Number|Time)(?:%0\d{1,2}d)?\$/.test(template)) templates.push(template);
      }
    }
    if (kind) send({ type: "manifest", url, kind, children: children.filter(Boolean).slice(0, 256), templates,
      live: kind === "hls" ? !text.includes("#EXT-X-STREAM-INF") && !text.includes("#EXT-X-ENDLIST") : /<MPD\b[^>]*\btype\s*=\s*["']dynamic["']/i.test(text),
      protected: kind === "hls" ? text.split(/\r?\n/).some((line) => /^#EXT-X-(?:SESSION-)?KEY:/.test(line) && !/METHOD=NONE(?:,|$)/.test(line)) : /<(?:[\w-]+:)?ContentProtection\b/.test(text) });
  }
  const arrayBuffer = Response.prototype.arrayBuffer;
  Response.prototype.arrayBuffer = function (...args) {
    const promise = Reflect.apply(arrayBuffer, this, args);
    promise.then((buffer) => bind(buffer, this.url), () => {});
    return promise;
  };
  const responseBlob = Response.prototype.blob;
  Response.prototype.blob = function (...args) {
    const promise = Reflect.apply(responseBlob, this, args);
    promise.then((blob) => bind(blob, this.url), () => {});
    return promise;
  };
  const originalFetch = window.fetch;
  let reads = 0;
  window.fetch = function (...args) {
    const promise = Reflect.apply(originalFetch, this, args);
    promise.then((response) => {
      const type = response.headers.get("content-type") || "";
      if (reads >= 4 || !/mpegurl|dash\+xml/i.test(type) || Number(response.headers.get("content-length")) > 512 * 1024) return;
      const copy = response.clone(), controller = new AbortController();
      reads++;
      const timeout = setTimeout(() => controller.abort(), 2500);
      (async () => {
        const reader = copy.body?.getReader();
        if (!reader) return;
        const cancel = () => { void reader.cancel().catch(() => {}); };
        controller.signal.addEventListener("abort", cancel, { once: true });
        let text = "", size = 0;
        const decoder = new TextDecoder();
        try {
          while (!controller.signal.aborted) {
            const { done, value } = await reader.read();
            if (done) { text += decoder.decode(); manifest(text, response.url, type); break; }
            size += value.byteLength;
            if (size > 512 * 1024) break;
            text += decoder.decode(value, { stream: true });
          }
        } finally { cancel(); controller.signal.removeEventListener("abort", cancel); }
      })().catch(() => {}).finally(() => { clearTimeout(timeout); reads--; });
    }, () => {}).catch(() => {});
    return promise;
  };
  const xhrSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function (...args) {
    const observe = () => {
      if (this.readyState !== 4) return;
      try {
        if (["arraybuffer", "blob"].includes(this.responseType)) bind(this.response, this.responseURL);
        else if (["", "text"].includes(this.responseType) && /mpegurl|dash\+xml/i.test(this.getResponseHeader("content-type") || "")) {
          manifest(this.responseText, this.responseURL, this.getResponseHeader("content-type"));
        }
      } catch { /* Inaccessible or non-text response. */ }
    };
    // Observe before the page's onload/onreadystatechange handlers construct
    // a Blob; binding it afterwards loses the wrapper's source ownership.
    this.addEventListener("readystatechange", observe, { capture: true });
    this.addEventListener("load", observe, { once: true, capture: true });
    this.addEventListener("loadend", () => this.removeEventListener("readystatechange", observe, true), { once: true });
    return Reflect.apply(xhrSend, this, args);
  };
  const create = URL.createObjectURL;
  // TikTok can wrap an XHR/fetch blob in another Blob before assigning it to
  // the player or its download link. Preserve one observed HTTP source only.
  if (/(^|\.)tiktok\.com$/i.test(location.hostname)) window.Blob = new Proxy(window.Blob, {
    construct(target, args, newTarget) {
      const result = Reflect.construct(target, args, newTarget);
      const parts = Array.isArray(args[0]) ? args[0] : [];
      const origins = parts.map((part) => buffers.get(part) || buffers.get(part?.buffer));
      if (origins.length && origins.every(Boolean) && new Set(origins).size === 1) bind(result, origins[0]);
      return result;
    },
  });
  URL.createObjectURL = function (value) {
    const blob = Reflect.apply(create, this, [value]);
    if (typeof MediaSource !== "undefined" && value instanceof MediaSource) { blobs.set(value, blob); report(value); }
    else if (buffers.has(value)) send({ type: "blob", blob, urls: [buffers.get(value)] });
    return blob;
  };
  if (typeof MediaSource !== "undefined") {
    const add = MediaSource.prototype.addSourceBuffer;
    MediaSource.prototype.addSourceBuffer = function (...args) {
      const buffer = Reflect.apply(add, this, args); owners.set(buffer, this); return buffer;
    };
    const append = SourceBuffer.prototype.appendBuffer;
    SourceBuffer.prototype.appendBuffer = function (buffer) {
      const result = Reflect.apply(append, this, [buffer]);
      const url = buffers.get(buffer) || buffers.get(buffer?.buffer), owner = owners.get(this);
      if (url && owner) {
        const set = urls.get(owner) || new Set(); set.add(url);
        while (set.size > 64) set.delete(set.values().next().value);
        urls.set(owner, set); report(owner);
      }
      return result;
    };
  }
  const attach = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function (...args) {
    const root = Reflect.apply(attach, this, args);
    if (args[0]?.mode === "open") setTimeout(() => this.dispatchEvent(new Event("zoltraak-shadow-root", { bubbles: true, composed: true })), 0);
    return root;
  };
})();
