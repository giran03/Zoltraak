// Shared by the isolated content script and background module. No page-world data
// or document-wide network timing is treated as proof of a player association.
(() => {
  // Only post routes understood by the Images extractors. Never accept a feed,
  // profile, audio page or CDN URL as a recovery target.
  function socialPostUrl(value) {
    try {
      const u = new URL(value);
      if (!/^https?:$/.test(u.protocol) || u.username || u.password || u.port) return null;
      const host = u.hostname.replace(/^(www|m|mobile)\./, "").replace(/^(old|new)\.reddit\.com$/, "reddit.com");
      if (["vm.tiktok.com", "vt.tiktok.com"].includes(host) && /^\/[\w-]+\/?$/.test(u.pathname)) {
        return `https://${host}${u.pathname}`;
      }
      let path;
      if (host === "instagram.com") {
        const match = u.pathname.match(/^\/(p|reel|reels|tv)\/([\w-]+)(?:\/|$)/);
        if (!match || ["audio", "liked", "saved"].includes(match[2])) return null;
        path = `/${match[1] === "reels" ? "reel" : match[1]}/${match[2]}/`;
      } else if (host === "x.com" || host === "twitter.com") {
        const match = u.pathname.match(/^\/([\w]+)\/(?:web\/)?status\/(\d+)(?:\/|$)/);
        if (!match) return null;
        return `https://x.com/${match[1]}/status/${match[2]}`;
      } else if (host === "facebook.com") {
        if (!/^\/(?:reel\/\d+|(?:[^/]+\/)?(?:videos|posts)\/[^/]+|groups\/[^/]+\/(?:posts|permalink)\/[^/]+|share\/[prv]\/[^/]+)(?:\/|$)/.test(u.pathname)
            && !(["/watch", "/watch/", "/photo.php", "/photo", "/photo/", "/permalink.php", "/story.php"].includes(u.pathname)
              && ["v", "fbid", "story_fbid"].some((key) => /^\d+$/.test(u.searchParams.get(key) || "")))) return null;
        u.hostname = "www.facebook.com";
        u.hash = "";
        for (const key of [...u.searchParams.keys()]) {
          if (key.startsWith("__") || ["mibextid", "rdid", "share_url", "ref", "refid"].includes(key)) u.searchParams.delete(key);
        }
        return u.href;
      } else if (host === "reddit.com") {
        const match = u.pathname.match(/^(\/(?:r\/[^/]+\/)?comments\/[a-z0-9]+)(?:\/|$)/i);
        if (!match) return null;
        path = `${match[1]}/`;
      } else if (host === "tiktok.com") {
        if (/^\/t\/[\w-]+\/?$/.test(u.pathname)) return `https://www.tiktok.com${u.pathname}`;
        const match = u.pathname.match(/^(\/(?:@[^/]+\/)?(?:video|photo)\/\d+)(?:\/|$)/);
        if (!match) return null;
        path = match[1];
      } else return null;
      return `https://www.${host}${path}`;
    } catch { return null; }
  }

  function itemIdentity(value) {
    try {
      const u = new URL(value);
      const host = u.hostname.replace(/^(www|m|mobile)\./, "");
      let id;
      let site = host;
      if (host === "youtu.be" || host === "youtube.com") {
        site = "youtube";
        id = host === "youtu.be" ? u.pathname.slice(1).split("/")[0]
          : u.searchParams.get("v") || u.pathname.match(/^\/(?:shorts|embed|live)\/([^/]+)/)?.[1];
      } else if (host === "facebook.com") {
        site = "facebook";
        id = u.searchParams.get("v") || u.pathname.match(/\/(?:reel|videos)\/([^/]+)/)?.[1];
        if (!id) {
          const post = u.searchParams.get("story_fbid") || u.pathname.match(/\/(?:posts|permalink)\/([^/]+)/)?.[1];
          const share = u.pathname.match(/\/share\/([prv])\/([^/]+)/);
          if (post) id = `post:${post}`;
          else if (share) id = `share:${share[1]}:${share[2]}`;
        }
      } else if (host === "instagram.com") {
        site = "instagram";
        id = u.pathname.match(/^\/(?:p|reel|reels|tv)\/([\w-]+)(?:\/|$)/)?.[1];
        if (["audio", "liked", "saved"].includes(id)) id = null;
      } else if (host === "x.com" || host === "twitter.com") {
        site = "x";
        id = u.pathname.match(/\/status\/(\d+)/)?.[1];
      } else if (host === "tiktok.com") {
        site = "tiktok";
        id = u.pathname.match(/\/(?:video|photo)\/(\d+)/)?.[1]
          || u.pathname.match(/^\/(?:embed\/(?:v2\/)?|player\/v1\/)(\d+)/)?.[1]
          || u.searchParams.get("itemId") || u.searchParams.get("item_id")
          || u.searchParams.get("share_item_id") || u.searchParams.get("aweme_id");
      } else if (host === "reddit.com") {
        site = "reddit";
        id = u.pathname.match(/\/comments\/([^/]+)/)?.[1];
      } else if (host === "vimeo.com" || host === "player.vimeo.com") {
        site = "vimeo";
        id = u.pathname.match(/\/(\d+)(?:\/|$)/)?.[1];
      }
      return id ? `${site}:${id}` : null;
    } catch { return null; }
  }

  function mediaIdentity(value) {
    try {
      const u = new URL(completeMediaUrl(value));
      if (!/^https?:$/.test(u.protocol)) return null;
      u.hash = "";
      // Meta byte ranges were removed above. Only Google Video also permits
      // ignoring signature fields; retain all unknown CDN content queries.
      if (/(^|\.)googlevideo\.com$/.test(u.hostname) && u.searchParams.has("id")) {
        for (const key of ["expire", "sig", "signature", "lsig", "n", "pot", "range", "bytestart", "byteend"]) {
          u.searchParams.delete(key);
        }
      }
      u.searchParams.sort();
      return u.href;
    } catch { return null; }
  }

  function sameMedia(a, b) {
    const identity = mediaIdentity(a);
    return Boolean(identity && identity === mediaIdentity(b));
  }

  function completeMediaUrl(value) {
    try {
      const u = new URL(value);
      if (!/(^|\.)(fbcdn\.net|cdninstagram\.com|cdninstagram\.net)$/.test(u.hostname)) return value;
      // Filter the original query text to preserve signed escaping and order.
      if (!/[?&](?:bytestart|byteend)=/i.test(value)) return value;
      const q = value.indexOf("?");
      const hash = value.indexOf("#", q);
      const base = value.slice(0, q);
      const params = value.slice(q + 1, hash < 0 ? undefined : hash);
      const kept = params.split("&").filter((pair) => !/^(bytestart|byteend)=/i.test(pair));
      return base + (kept.length ? `?${kept.join("&")}` : "") + (hash < 0 ? "" : value.slice(hash));
    } catch { return value; }
  }

  function contextPostTarget(link, context, page) {
    // A profile, CDN attachment or external link inside a post must not
    // override the post recovered from the clicked container.
    const attachment = /\/photo(?:\.php)?(?:[/?]|$)/.test(socialPostUrl(link) || "");
    for (const value of attachment ? [context, link, page] : [link, context, page]) {
      if (socialPostUrl(value)) return value;
    }
    try {
      const host = new URL(page).hostname;
      if (/(^|\.)(facebook\.com|instagram\.com|x\.com|twitter\.com|reddit\.com|tiktok\.com)$/.test(host)) return null;
    } catch { return null; }
    return link || context || page || null;
  }

  function verifyAndScoreCandidate(metadata, url, context) {
    if (!metadata?.formats?.length) return { valid: false, reason: "No formats in candidate" };
    const evidence = [];
    let score = 0;
    const expected = itemIdentity(context?.permalink);
    const actual = itemIdentity(metadata.webpageUrl) || itemIdentity(url);
    if (expected && actual && expected !== actual) return { valid: false, reason: "Item ID mismatch" };
    if (expected && actual === expected) { evidence.push("item-id"); score += 100; }
    const trusted = [context?.currentSrc, context?.elementManifest].filter(Boolean);
    if (trusted.some((src) => sameMedia(src, url) || metadata.formats.some((f) => sameMedia(src, f.videoUrl)))) {
      evidence.push("player-source"); score += 100;
    }
    if (context?.duration > 1 && metadata.duration > 1) {
      const diff = Math.abs(metadata.duration - context.duration);
      if (diff > 3 && diff / context.duration > 0.1) return { valid: false, reason: "Duration mismatch" };
      score += diff <= 1.5 ? 30 : 10;
    }
    // CSS layout dimensions are not intrinsic media dimensions.
    if (context?.width > 0 && context?.height > 0) {
      const f = metadata.formats.find((f) => f.width && f.height);
      if (f) {
        const expectedRatio = context.width / context.height;
        const actualRatio = f.width / f.height;
        if ((expectedRatio < 0.85 && actualRatio > 1.15) || (expectedRatio > 1.15 && actualRatio < 0.85)) {
          return { valid: false, reason: "Orientation mismatch" };
        }
        if (Math.abs(expectedRatio - actualRatio) / expectedRatio <= 0.15) score += 10;
      }
    }
    return { valid: true, score, strong: evidence.length > 0, evidence };
  }

  function filterFormatsByVideoContext(formats, context) {
    const trusted = [context?.currentSrc, context?.elementManifest].filter(Boolean);
    const matching = formats.filter((f) => trusted.some((src) => sameMedia(src, f.videoUrl)));
    // Do not group on audio URL: distinct clips can reuse an audio track.
    if (matching.length) {
      const clip = matching[0].id?.match(/^(clip-\d+-)/)?.[1];
      return clip ? formats.filter((f) => f.id?.startsWith(clip)) : matching;
    }
    // A carousel's clips are separate items, not interchangeable qualities.
    // Duration can narrow choices but cannot establish player identity.
    if (formats.some((f) => /^clip-\d+-/.test(f.id || "")) && context?.duration > 1) {
      return formats.filter((f) => {
        const duration = f.validation?.duration;
        return !(duration > 1) || Math.abs(duration - context.duration) <= Math.max(3, context.duration * 0.1);
      });
    }
    return formats;
  }

  function createPlayerTracker() {
    const states = new WeakMap();
    let nextId = 0;
    function observe(player, fingerprint, reset = false, itemId = null) {
      let state = states.get(player);
      if (!state) {
        state = { playerInstanceId: `player-${++nextId}`, generation: 0 };
        states.set(player, state);
      }
      const playbackTime = player.currentTime;
      // MSE players can reuse a blob and omit loadstart between clips. A reset
      // of playback time invalidates an unidentified selection. A known post
      // keeps its identity across replay, looping, and seeking to the start.
      const rewound = !itemId && Number.isFinite(playbackTime) && state.playbackTime > 2 && playbackTime < 0.5;
      if ((reset && !itemId) || rewound || state.fingerprint !== fingerprint) {
        state.generation += 1;
        state.fingerprint = fingerprint;
        state.playbackStartedAt = Date.now();
      }
      state.playbackTime = playbackTime;
      return { ...state };
    }
    function matches(player, fingerprint, binding, itemId = null) {
      const state = observe(player, fingerprint, false, itemId);
      return Boolean(binding && state.playerInstanceId === binding.playerInstanceId
        && state.generation === binding.generation && state.fingerprint === binding.fingerprint);
    }
    return { observe, matches };
  }

  function isAudioOnly(format) {
    return ["m4a", "mp3", "aac", "ogg", "oga", "opus", "wav", "flac"].includes(format.container)
      || format.id === "audio" || format.id?.startsWith("dash-audio-")
      || (format.validation?.hasVideo === false && format.validation?.hasAudio === true);
  }

  globalThis.ZoltraakVideoIdentity = { socialPostUrl, contextPostTarget, completeMediaUrl, itemIdentity, mediaIdentity, sameMedia, isAudioOnly,
    verifyAndScoreCandidate, filterFormatsByVideoContext, createPlayerTracker };
})();
