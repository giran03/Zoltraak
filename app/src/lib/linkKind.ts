/**
 * Which tab a link belongs on, when it is not a plain file.
 *
 * Mirrors the routes the extension and the engine already understand: music
 * services go to Music, social post permalinks to Posts (the Images section),
 * and single-video pages to Video. Anything else — including direct files and
 * unknown sites — stays a regular download.
 */
export type LinkTarget = { section: "Video" | "Images" | "Music"; noun: string; tab: string };

const MUSIC: LinkTarget = { section: "Music", noun: "a music link", tab: "Music" };
const POST: LinkTarget = { section: "Images", noun: "a post", tab: "Posts" };
const VIDEO: LinkTarget = { section: "Video", noun: "a video page", tab: "Video" };

const MUSIC_HOSTS = [
  "spotify.com", "spotify.link", "music.apple.com", "itunes.apple.com", "deezer.com",
  "deezer.page.link", "link.deezer.com", "music.youtube.com", "soundcloud.com", "snd.sc", "bandcamp.com",
];

const isHost = (host: string, name: string) => host === name || host.endsWith(`.${name}`);

function isSocialPost(host: string, url: URL): boolean {
  const path = url.pathname;
  if (host === "vm.tiktok.com" || host === "vt.tiktok.com") return /^\/[\w-]+\/?$/.test(path);
  if (host === "instagram.com") {
    const match = path.match(/^\/(p|reel|reels|tv)\/([\w-]+)(?:\/|$)/);
    return Boolean(match) && !["audio", "liked", "saved"].includes(match?.[2] ?? "");
  }
  if (host === "x.com" || host === "twitter.com") return /^\/\w+\/(?:web\/)?status\/\d+(?:\/|$)/.test(path);
  if (host === "facebook.com") {
    return /^\/(?:reel\/\d+|(?:[^/]+\/)?(?:videos|posts)\/[^/]+|groups\/[^/]+\/(?:posts|permalink)\/[^/]+|share\/[prv]\/[^/]+)(?:\/|$)/.test(path)
      || (["/watch", "/watch/", "/photo.php", "/photo", "/photo/", "/permalink.php", "/story.php"].includes(path)
        && ["v", "fbid", "story_fbid"].some(key => /^\d+$/.test(url.searchParams.get(key) ?? "")));
  }
  if (host === "reddit.com") return /^\/(?:r\/[^/]+\/)?comments\/[a-z0-9]+(?:\/|$)/i.test(path);
  if (host === "tiktok.com") return /^\/t\/[\w-]+\/?$/.test(path) || /^\/(?:@[^/]+\/)?(?:video|photo)\/\d+(?:\/|$)/.test(path);
  return false;
}

function isVideoPage(host: string, url: URL): boolean {
  const path = url.pathname;
  if (host === "youtu.be") return /^\/[\w-]{6,}\/?$/.test(path);
  if (host === "youtube.com" || host === "youtube-nocookie.com") {
    return (path === "/watch" && Boolean(url.searchParams.get("v"))) || /^\/(?:shorts|live|embed)\/[\w-]+/.test(path);
  }
  if (host === "vimeo.com") return /^\/(?:channels\/[^/]+\/|groups\/[^/]+\/videos\/)?\d+(?:\/|$)/.test(path);
  return false;
}

export function classifyLink(raw: string): LinkTarget | null {
  let url: URL;
  try { url = new URL(raw); } catch { return null; }
  if (!/^https?:$/.test(url.protocol) || url.username || url.password) return null;
  const host = url.hostname.toLowerCase().replace(/^(www|m|mobile)\./, "").replace(/^(old|new)\.reddit\.com$/, "reddit.com");

  if (MUSIC_HOSTS.some(name => isHost(host, name)) && (url.pathname !== "/" || host === "spotify.link")) return MUSIC;
  if (isSocialPost(host, url)) return POST;
  if (isVideoPage(host, url)) return VIDEO;
  return null;
}

/** The question asked before a pasted link is sent to another tab. */
export function routeMessage(target: LinkTarget): string {
  return `This link looks like ${target.noun}.\n\nOpen it in the ${target.tab} tab instead?\n\nOK opens it there. Cancel downloads the link as a regular file.`;
}
