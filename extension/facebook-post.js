// Shared by the page probe and isolated content script. Cache only attachment
// metadata, never request bodies, account tokens, comments or media bytes.
(() => {
  if (globalThis.ZoltraakFacebook) return;
  const host = (value) => { try { const u = new URL(value, location.href); return u.protocol === 'https:' && /(^|\.)facebook\.com$/.test(u.hostname) ? u : null; } catch { return null; } };
  const postId = (value) => { const u = host(value); return u?.searchParams.get('story_fbid') || u?.pathname.match(/\/posts\/(pfbid[\w]+|\d+)(?:\/|$)/)?.[1] || null; };
  const cdn = (value) => { try { const u = new URL(value); return u.protocol === 'https:' && !u.username && !u.password && (!u.port || u.port === '443') && /(^|\.)(fbcdn\.net|fbsbx\.com)$/.test(u.hostname); } catch { return false; } };
  const blocked = new Set(['feedback', 'comments', 'recommendations', 'related_stories', 'creation_story']);
  const scalar = new Set(['__typename', 'id', 'post_id', 'story_fbid', 'url', 'permalink_url', 'uri', 'width', 'height', 'count', 'total_count', 'has_next_page', 'mediaset_token', 'mediaSetToken', 'mediasetToken', 'browser_native_hd_url', 'browser_native_sd_url', 'playable_url', 'playable_url_quality_hd']);
  function compact(value) {
    let budget = 30000;
    function copy(v, depth = 0) {
      if (!v || typeof v !== 'object' || depth > 45 || --budget <= 0) return null;
      if (Array.isArray(v)) return v.slice(0, 1000).map((x) => copy(x, depth + 1)).filter(Boolean);
      const out = {};
      for (const [key, child] of Object.entries(v)) {
        if (blocked.has(key)) continue;
        if (scalar.has(key) && ['string', 'number', 'boolean'].includes(typeof child)) {
          if (typeof child !== 'string' || child.length <= 8192) out[key] = child;
        } else if (child && typeof child === 'object') {
          const next = copy(child, depth + 1);
          if (next && Object.keys(next).length) out[key] = next;
        }
      }
      return Object.keys(out).length ? out : null;
    }
    return copy(value);
  }
  function walk(v, fn, depth = 0) {
    if (!v || typeof v !== 'object' || depth > 45 || fn(v)) return;
    for (const [key, child] of Object.entries(v)) if (!blocked.has(key)) walk(child, fn, depth + 1);
  }
  function parse(text) {
    if (typeof text !== 'string' || text.length > 2 * 1024 * 1024) return [];
    const cleaned = text.trim().replace(/^for\s*\(\s*;\s*;\s*\)\s*;?\s*/, '').replace(/^\)\]\}',?\s*/, '');
    try { return [compact(JSON.parse(cleaned))].filter(Boolean); } catch { /* Relay can stream one JSON envelope per line. */ }
    return cleaned.split('\n').slice(0, 128).flatMap((line) => { try { return [compact(JSON.parse(line))].filter(Boolean); } catch { return []; } });
  }
  function scripts(doc) { return [...doc.scripts].slice(0, 256).flatMap((s) => parse(s.textContent)); }
  function select(documents, url, permalinkDocuments = []) {
    const wanted = postId(url);
    if (!wanted) return null;
    const roots = [];
    for (const doc of documents) walk(doc, (v) => {
      if (v.post_id === wanted || v.story_fbid === wanted || [v.permalink_url, v.url].some((s) => s && postId(s) === wanted)) {
        if (v.attachments || v.__typename === 'Story') { roots.push(v); return true; }
      }
      return false;
    });
    // Only the HTML of the exact open permalink can establish this alias.
    // A feed or an unrelated GraphQL response is never an alias source.
    if (!roots.length && postId(location.href) === wanted) {
      const requested = new Map();
      for (const doc of permalinkDocuments) walk(doc, (v) => {
        const story = v.data?.node_v2;
        const declared = postId(story?.permalink_url || story?.url);
        if (story?.__typename === 'Story' && /^\d+$/.test(story.post_id || '') && (!declared || declared === wanted)) requested.set(story.post_id, story);
        return false;
      });
      if (requested.size === 1) roots.push([...requested.values()][0]);
    }
    if (!roots.length) return null;
    const sets = new Set(), items = new Map(), navigation = new Map(), counts = new Set();
    const add = (v) => {
      if (!['Photo', 'Video'].includes(v.__typename) || !/^\d+$/.test(v.id || '')) return;
      if (!['viewer_image', 'image', 'photo_image', 'full_size_image'].some((key) => cdn(v[key]?.uri))
          && !['browser_native_hd_url', 'browser_native_sd_url', 'playable_url', 'playable_url_quality_hd'].some((key) => cdn(v[key]))) return;
      const old = items.get(v.id);
      // Merge richer viewer fields with existing thumbnail/post metadata.
      items.set(v.id, { ...old, ...v });
    };
    function attachments(root) {
      walk(root, (v) => {
        if (v !== root && v.__typename === 'Story' && v.post_id && !roots.some((r) => r.post_id === v.post_id)) return true;
        for (const key of ['mediaset_token', 'mediaSetToken', 'mediasetToken']) if (/^pcb\.\d+$/.test(v[key] || '')) sets.add(v[key]);
        if (v.all_subattachments) {
          const n = v.all_subattachments.count ?? v.all_subattachments.total_count;
          if (Number.isSafeInteger(n) && n > 0 && n <= 10000) counts.add(n);
        }
        if (['Photo', 'Video'].includes(v.__typename)) {
          add(v);
          const u = host(v.url), set = u?.searchParams.get('set');
          if (/^pcb\.\d+$/.test(set || '')) sets.add(set);
          return true;
        }
        return false;
      });
    }
    for (const root of roots) {
      if (/^\d+$/.test(root.post_id || '')) sets.add(`pcb.${root.post_id}`);
      walk(root, (v) => { if (v.attachments) { attachments(v.attachments); return true; } return v !== root && v.__typename === 'Story' && v.post_id !== root.post_id; });
    }
    // Gallery responses may have no Story wrapper. A matching post collection
    // token or an exact photo+set URL is required before using their media.
    for (const doc of documents) walk(doc, (v) => {
      if (v.__typename === 'Story' && v.post_id && !roots.some((r) => r.post_id === v.post_id)) return true;
      const declared = ['mediaset_token', 'mediaSetToken', 'mediasetToken'].map((key) => v[key]).filter(Boolean);
      if (declared.length && !declared.some((set) => sets.has(set))) return true;
      const scoped = ['mediaset_token', 'mediaSetToken', 'mediasetToken'].some((key) => sets.has(v[key]));
      const u = host(v.url), ownsPhoto = sets.has(u?.searchParams.get('set')) && u?.searchParams.get('fbid') === v.id;
      if (scoped) {
        attachments(v);
        walk(v, (n) => {
          const id = n.media?.id || n.id;
          if (id && n.nextMediaAfterNodeId?.id) navigation.set(id, n.nextMediaAfterNodeId.id);
          return false;
        });
      } else if (ownsPhoto) add(v);
      return scoped || ownsPhoto;
    });
    if (!items.size) return null;
    const ordered = [], seen = new Set();
    function chain(id) { while (items.has(id) && !seen.has(id)) { seen.add(id); ordered.push(items.get(id)); id = navigation.get(id); } }
    for (const id of items.keys()) chain(id);
    const expected = counts.size === 1 ? [...counts][0] : null;
    const nodes = ordered.map((v) => {
      const out = { __typename: v.__typename, id: v.id };
      if (host(v.url)) out.url = v.url;
      for (const key of ['viewer_image', 'image', 'photo_image', 'full_size_image']) if (cdn(v[key]?.uri)) out[key] = { uri: v[key].uri, width: v[key].width, height: v[key].height };
      for (const key of ['browser_native_hd_url', 'browser_native_sd_url', 'playable_url', 'playable_url_quality_hd']) if (cdn(v[key])) out[key] = v[key];
      return out;
    });
    const root = { __typename: 'Story', permalink_url: url, attachments: [{ all_subattachments: { nodes, ...(expected ? { count: expected } : {}) } }] };
    const numeric = new Set(roots.map((r) => r.post_id).filter((id) => /^\d+$/.test(id || '')));
    if (numeric.size === 1) root.post_id = [...numeric][0];
    const set = sets.size === 1 ? [...sets][0] : null;
    if (set) root.mediaSetToken = set;
    const result = { documents: [root], expected, set, ids: nodes.map((n) => n.id), next: [...navigation.values()] };
    return JSON.stringify(result).length <= 512 * 1024 ? result : null;
  }
  async function read(response, milliseconds = 3000) {
    if (!response.ok || Number(response.headers.get('content-length')) > 2 * 1024 * 1024) return '';
    const reader = response.body?.getReader();
    if (!reader) return '';
    const timer = setTimeout(() => { void reader.cancel().catch(() => {}); }, milliseconds);
    let text = '', size = 0;
    const decoder = new TextDecoder();
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) return text + decoder.decode();
        size += value.byteLength;
        if (size > 2 * 1024 * 1024) return '';
        text += decoder.decode(value, { stream: true });
      }
    } finally { clearTimeout(timer); void reader.cancel().catch(() => {}); }
  }
  function installProbe() {
    if (!host(location.href) || globalThis.__zoltraakFacebookProbe) return;
    globalThis.__zoltraakFacebookProbe = true;
    const cache = [];
    let bytes = 0, reads = 0;
    const remember = (text) => {
      for (const doc of parse(text)) {
        const size = JSON.stringify(doc).length;
        if (size > 512 * 1024) continue;
        cache.push({ doc, size, at: Date.now() }); bytes += size;
        while (cache.length > 32 || bytes > 2 * 1024 * 1024) bytes -= cache.shift().size;
      }
    };
    const rememberPermalink = () => {
      const initial = scripts(document), selected = select(initial, location.href, initial);
      if (selected) remember(JSON.stringify(selected.documents));
    };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', rememberPermalink, { once: true });
    else rememberPermalink();
    const relevant = (url) => { const u = host(url); return u?.origin === location.origin && /^\/(?:api\/graphql|ajax\/)/.test(u.pathname); };
    const fetch = window.fetch;
    window.fetch = function (...args) {
      const promise = Reflect.apply(fetch, this, args);
      promise.then((response) => {
        if (!relevant(response.url) || reads >= 2) return;
        reads++;
        read(response.clone()).then(remember).catch(() => {}).finally(() => { reads--; });
      }, () => {}).catch(() => {});
      return promise;
    };
    const send = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.send = function (...args) {
      this.addEventListener('load', () => {
        try {
          if (!relevant(this.responseURL)) return;
          if (this.responseType === 'json') remember(JSON.stringify(this.response));
          else if (['', 'text'].includes(this.responseType)) remember(this.responseText);
        } catch { /* Ignore inaccessible and non-text responses. */ }
      }, { once: true });
      return Reflect.apply(send, this, args);
    };
    window.addEventListener('message', (event) => {
      if (event.source !== window || event.data?.channel !== 'zoltraak-facebook-request' || typeof event.data.requestId !== 'string') return;
      const docs = cache.filter((c) => Date.now() - c.at < 300000).map((c) => c.doc);
      const initial = scripts(document);
      const selected = select([...initial, ...docs], event.data.url, initial);
      window.postMessage({ channel: 'zoltraak-facebook-result', requestId: event.data.requestId, documents: selected?.documents || [] }, location.origin);
    });
  }
  async function collect(url) {
    if (!host(location.href) || !postId(url)) return [];
    const docs = scripts(document), requestId = crypto.randomUUID();
    const observed = await new Promise((resolve) => {
      const finish = (documents) => { clearTimeout(timer); window.removeEventListener('message', listener); resolve(documents); };
      const listener = (event) => { if (event.source === window && event.data?.channel === 'zoltraak-facebook-result' && event.data.requestId === requestId) finish(Array.isArray(event.data.documents) ? event.data.documents.slice(0, 32) : []); };
      const timer = setTimeout(() => finish([]), 700);
      window.addEventListener('message', listener);
      window.postMessage({ channel: 'zoltraak-facebook-request', requestId, url }, location.origin);
    });
    docs.push(...observed);
    let selected = select(docs, url, docs.slice(0, docs.length - observed.length));
    if (!selected || selected.expected === selected.ids.length || !selected.set) return selected?.documents || [];
    const deadline = Date.now() + 10000, visited = new Set();
    // Ask the browser for ordinary viewer pages, using its own cookies and UA.
    // No private GraphQL query IDs or account tokens are fabricated/replayed.
    for (let requests = 0; requests < 32 && Date.now() < deadline; requests++) {
      const id = [...selected.ids, ...selected.next].find((n) => /^\d+$/.test(n) && !visited.has(n));
      if (!id) break;
      visited.add(id);
      const controller = new AbortController(), timer = setTimeout(() => controller.abort(), Math.min(2500, deadline - Date.now()));
      try {
        const viewer = new URL('/photo/', location.origin);
        viewer.searchParams.set('fbid', id); viewer.searchParams.set('set', selected.set);
        const response = await fetch(viewer, { credentials: 'include', signal: controller.signal });
        if (!host(response.url) || /\/(?:login|checkpoint)(?:\/|$)/.test(new URL(response.url).pathname)) continue;
        const html = await read(response, Math.min(2500, deadline - Date.now()));
        docs.push(...scripts(new DOMParser().parseFromString(html, 'text/html')));
        // Keep the bound Story from the first result so subsequent viewer
        // pages need not repeat the permalink's numeric/pfbid relationship.
        selected = select([...selected.documents, ...docs], url) || selected;
        if (selected.expected === selected.ids.length) break;
      } catch { /* Keep all already found attachments. */ }
      finally { clearTimeout(timer); }
    }
    return selected.documents;
  }
  globalThis.ZoltraakFacebook = { compact, parse, select, collect, installProbe };
})();
