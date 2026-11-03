/**
 * A scripted stand-in for the Rust engine. The real React app is mounted
 * unchanged on top of it, so every pixel on screen is the shipping UI; only
 * the data underneath is staged.
 *
 * Installed as `window.__TAURI_INTERNALS__` before the app imports anything.
 */
/* eslint-disable @typescript-eslint/no-explicit-any */
import { CONVERSION_FORMATS } from "../src/lib/conversion";
const MB = 1024 * 1024;
const GB = 1024 * MB;
const iso = (offsetMs = 0) => new Date(Date.now() + offsetMs).toISOString();

type Row = any;
export type Sim = { speed: number; target?: number };

const listeners = new Map<string, Set<number>>();
let cbId = 1;
const w = window as any;
try { localStorage.setItem("zoltraak_release_notes_seen", "9.9.9"); localStorage.removeItem("zoltraak.notifications.v1"); localStorage.setItem("zoltraak.appearance.v1", JSON.stringify({ themeMode: "dark" })); } catch { /* storage is optional */ }

function emit(event: string, payload: unknown) {
  for (const id of listeners.get(event) ?? []) w[`_${id}`]?.({ event, id, payload });
}

export const state: {
  queue: Row[]; video: Row[]; music: Row[]; remux: Row[];
  settings: any; musicSettings: any; paused: boolean;
  analyzeDelay: number;
  sims: Map<string, Sim>;
  deliveries: Row[];
  extensionOnline: boolean;
} = {
  queue: [], video: [], music: [], remux: [], paused: false, analyzeDelay: 900,
  sims: new Map(), deliveries: [], extensionOnline: true,
  settings: {
    bandwidthLimitBytesPerSec: null, defaultConnections: 16, defaultDirectory: "D:\\Downloads",
    desktopNotifications: false, playSoundOnComplete: false, startOnLaunch: false, closeToTray: true,
    minimizeToTray: false, deleteFilesOnRemove: true,
    schedule: { enabled: false, startMinutes: 60, stopMinutes: 420 },
    onComplete: { type: "none" },
  },
  musicSettings: {
    output: {
      format: "flac", bitrateKbps: null, directory: "D:\\Music", folderMode: "collection", folderName: null,
      filenameTemplate: "{track} - {title}", numberTracks: true, embedMetadata: true, embedCover: true,
      fetchArtwork: true, saveCoverFile: false, fetchLyrics: false, writePlaylistFile: false,
      skipExisting: true, normalizeLoudness: false,
    },
    maxConcurrent: 3, preferYoutubeMusic: false,
  },
};

const pushQueue = () => emit("zoltraak://queue", JSON.parse(JSON.stringify(state.queue)));

export function addDownload(row: Partial<Row> & { filename: string; url: string }, sim?: Sim): Row {
  const id = row.id ?? `dl_${Math.random().toString(36).slice(2, 8)}`;
  const item: Row = {
    id, state: "downloading", downloadedBytes: 0, totalBytes: 100 * MB, bytesPerSecond: 0, etaSeconds: null,
    connections: 16, directory: "D:\\Downloads", priority: 0, error: null, createdAt: iso(), ...row,
  };
  state.queue.unshift(item);
  if (sim) state.sims.set(id, sim);
  pushQueue();
  return item;
}
export const find = (id: string) => state.queue.find(r => r.id === id) ?? state.video.find(r => r.id === id) ?? state.music.find(r => r.id === id);

/** Advance every running transfer by `dt` ms. Called by the director's clock. */
export function step(dt: number) {
  if (state.paused) return;
  const sec = dt / 1000;
  let queueDirty = false;
  for (const row of state.queue) {
    const sim = state.sims.get(row.id);
    if (!sim || row.state !== "downloading") continue;
    const speed = sim.speed * (0.93 + Math.random() * 0.14);
    row.bytesPerSecond = Math.round(speed);
    row.downloadedBytes = Math.min(row.totalBytes, row.downloadedBytes + speed * sec);
    row.etaSeconds = Math.max(0, Math.round((row.totalBytes - row.downloadedBytes) / Math.max(1, speed)));
    if (row.downloadedBytes >= row.totalBytes) {
      row.state = "finished"; row.bytesPerSecond = 0; row.etaSeconds = null; row.path = `${row.directory}\\${row.filename}`;
      // Bounded concurrency: a slot freed, so the next waiting transfer starts.
      const next = state.queue.slice().reverse().find(r => r.state === "waiting");
      if (next) setTimeout(() => { next.state = "downloading"; pushQueue(); }, 600);
    }
    queueDirty = true;
  }
  if (queueDirty) pushQueue();
  for (const job of state.video) {
    const sim = state.sims.get(job.id);
    if (!sim || job.state !== "downloading") continue;
    const speed = sim.speed * (0.93 + Math.random() * 0.14);
    job.bytesPerSecond = Math.round(speed);
    job.downloadedBytes = Math.min(job.totalBytes, job.downloadedBytes + speed * sec);
    job.etaSeconds = Math.max(0, Math.round((job.totalBytes - job.downloadedBytes) / Math.max(1, speed)));
    if (job.downloadedBytes >= job.totalBytes) { job.state = "finished"; job.bytesPerSecond = 0; job.etaSeconds = null; job.path = `D:\\Video\\${job.title}.mp4`; }
  }
  for (const job of state.music) {
    const sim = state.sims.get(job.id);
    if (!sim) continue;
    if (job.state === "downloading") {
      const speed = sim.speed * (0.93 + Math.random() * 0.14);
      job.bytesPerSecond = Math.round(speed);
      job.downloadedBytes = Math.min(job.totalBytes, job.downloadedBytes + speed * sec);
      job.etaSeconds = Math.max(0, Math.round((job.totalBytes - job.downloadedBytes) / Math.max(1, speed)));
      job.stage = "Downloading";
      if (job.downloadedBytes >= job.totalBytes) { job.state = "converting"; job.stage = "Tagging and converting"; job.bytesPerSecond = 0; sim.target = 0.6; }
    } else if (job.state === "converting") {
      sim.target = (sim.target ?? 0) - sec;
      if ((sim.target ?? 0) <= 0) { job.state = "finished"; job.stage = null; job.path = `${job.directory}\\${job.track.title}.flac`; job.finishedAt = iso(); }
    }
  }
}

export function startMusicJobs(source: any, format: string, delayStep = 650, speed = 6 * MB) {
  const collectionId = `col_${Math.random().toString(36).slice(2, 6)}`;
  const ids: string[] = [];
  source.tracks.forEach((track: any, i: number) => {
    const id = `mj_${collectionId}_${i}`;
    ids.push(id);
    const job = {
      id, collectionId, collectionTitle: source.title, collectionUrl: source.sourceUrl, position: i, track, format,
      state: "queued", stage: "Queued", matchedUrl: null, matchedTitle: null, downloadedBytes: 0,
      totalBytes: Math.round((track.duration ?? 200) * 0.55 * MB / 10 * 10), bytesPerSecond: 0, etaSeconds: null,
      error: null, path: null, directory: "D:\\Music\\" + source.title, createdAt: iso(), updatedAt: iso(), finishedAt: null,
    };
    state.music.push(job);
    state.sims.set(id, { speed });
    setTimeout(() => { if (job.state === "queued") { job.state = "resolving"; job.stage = "Matching source"; } }, i * delayStep);
    setTimeout(() => { if (job.state === "resolving") { job.state = "downloading"; job.stage = "Downloading"; } }, i * delayStep + 900);
  });
  return { collectionId, jobIds: ids, skipped: 0, directory: "D:\\Music\\" + source.title };
}


/** Anything a screen asks for that the scene does not care about. */
const unknown = new Set<string>();
export const hooks: Record<string, (args: any) => any | Promise<any>> = {};

async function invoke(cmd: string, args: any = {}): Promise<any> {
  if (cmd === "plugin:event|listen") {
    const set = listeners.get(args.event) ?? new Set<number>();
    set.add(args.handler); listeners.set(args.event, set);
    return args.handler;
  }
  if (cmd === "plugin:event|unlisten") return null;
  if (cmd.startsWith("plugin:")) return null;
  if (hooks[cmd]) return hooks[cmd](args);
  switch (cmd) {
    case "engine_ready": return true;
    case "default_directory": return state.settings.defaultDirectory;
    case "music_directory": return state.musicSettings.output.directory;
    case "choose_directory": return null;
    case "queue_snapshot": return JSON.parse(JSON.stringify(state.queue));
    case "api_status": return { listening: true, port: 1450, token: "showcase-token", detail: null };
    case "get_settings": return JSON.parse(JSON.stringify(state.settings));
    case "update_settings": state.settings = args.settings; return args.settings;
    case "get_app_version": return "1.0.0";
    case "list_video_jobs": return JSON.parse(JSON.stringify(state.video));
    case "list_music_jobs": return JSON.parse(JSON.stringify(state.music));
    case "list_remux_jobs": return JSON.parse(JSON.stringify(state.remux));
    case "media_capabilities": return { formats: CONVERSION_FORMATS.map(format => format.value), ffprobe: true };
    case "choose_media_files": case "scan_media": return [];
    case "plan_conversion": return [];
    case "list_categories": return [
      { id: 1, name: "Software", kind: "extension", pattern: "exe,msi,iso,dmg,deb", directory: "D:\\Apps", priority: 10, enabled: true },
      { id: 2, name: "Documents", kind: "extension", pattern: "pdf,docx,epub,txt", directory: "D:\\Docs", priority: 20, enabled: true },
      { id: 3, name: "Archives", kind: "extension", pattern: "zip,rar,7z,tar,gz", directory: "D:\\Archives", priority: 30, enabled: true },
      { id: 4, name: "Video", kind: "extension", pattern: "mp4,mkv,mov,webm", directory: "D:\\Video", priority: 40, enabled: true },
    ];
    case "music_settings": return JSON.parse(JSON.stringify(state.musicSettings));
    case "update_music_settings": state.musicSettings = args.settings; return args.settings;
    case "music_tools": return { ffmpeg: "7.1", ffprobe: true };
    case "list_sessions": return [{ domain: "youtube.com", updatedAt: iso(-3600_000), cookieNames: ["SID", "HSID", "SAPISID"], hasPoToken: true, signedIn: true }];
    case "sync_app_icon": return { shortcutsUpdated: 3, warning: null };
    case "download_file_statuses": { const m: any = {}; for (const id of args.ids) m[id] = { path: null, status: "present", detail: null }; return m; }
    case "download_file_status": return { path: null, status: "present", detail: null };
    case "download_thumbnail": return null;
    case "extension_source_state": return {
      revision: 1, preferredClientId: "c1", activeClientId: "c1",
      clients: [{ clientId: "c1", browserId: "chrome", browserName: "Chrome", extensionVersion: "0.2.9", lastSeenAtMs: Date.now(), online: state.extensionOnline, preferred: true, active: true, temporary: false }],
    };
    case "get_extension_version": case "get_firefox_extension_version": return "0.2.9";
    case "detect_browsers": return [];
    case "discord_status": return { connected: false, mode: null, detail: null };
    case "discord_settings": return {};
    case "discord_deliveries": return [];
    case "pause_download": { const r = find(args.id); if (r) { r.state = "paused"; r.bytesPerSecond = 0; r.etaSeconds = null; pushQueue(); } return null; }
    case "resume_download": { const r = find(args.id); if (r) { r.state = "downloading"; pushQueue(); } return null; }
    case "remove_download": state.queue = state.queue.filter(r => r.id !== args.id); pushQueue(); return null;
    case "add_download": {
      const name = decodeURIComponent((args.url as string).split("/").pop()?.split("?")[0] || "download");
      return addDownload({ filename: args.filename || name, url: args.url, totalBytes: 48 * MB, connections: 16 }, { speed: 9 * MB }).id;
    }
    case "check_for_updates": return null;
    case "get_cached_updates": return null;
    default:
      if (!unknown.has(cmd)) { unknown.add(cmd); console.debug("[mock] unhandled", cmd, args); }
      return null;
  }
}

w.__TAURI_INTERNALS__ = {
  invoke,
  transformCallback(cb: (e: unknown) => void) { const id = cbId++; w[`_${id}`] = cb; return id; },
  unregisterCallback(id: number) { delete w[`_${id}`]; },
  convertFileSrc: (p: string) => p,
  metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
};
w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
w.__mock = { KEYNOTE: undefined as any, state, emit, pushQueue, addDownload, step, startMusicJobs, hooks, find, MB, GB };

/* ------------------------------------------------------------------ staged content */

/** A gradient poster as a data URI, so no screen ever depends on the network. */
export function art(a: string, b: string, c = "#0b0e0d", w = 320, hgt = 320, glyph = ""): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${hgt}" viewBox="0 0 ${w} ${hgt}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${a}"/><stop offset="1" stop-color="${b}"/></linearGradient></defs><rect width="${w}" height="${hgt}" fill="url(#g)"/><circle cx="${w * 0.7}" cy="${hgt * 0.3}" r="${hgt * 0.16}" fill="${c}" opacity=".35"/><path d="M0 ${hgt * 0.78} L${w * 0.3} ${hgt * 0.55} L${w * 0.55} ${hgt * 0.72} L${w * 0.8} ${hgt * 0.5} L${w} ${hgt * 0.68} V${hgt} H0Z" fill="${c}" opacity=".45"/>${glyph}</svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

const mf = (id: string, h: number, vc: string, mb: number, merge = true) => ({
  id, container: "mp4", videoCodec: vc, audioCodec: "aac", width: Math.round(h * 16 / 9), height: h, fps: h >= 1080 ? 60 : 30,
  bitrate: null, filesize: mb * MB, videoUrl: `https://media.example.com/${id}.mp4`, audioUrl: merge ? "https://media.example.com/audio.m4a" : null,
  requiresMerge: merge, hasAudio: true, validation: { status: "verified", hasVideo: true, hasAudio: true, duration: 3492, contentId: "x", samplesDecoded: true },
});
export const KEYNOTE = {
  title: "Keynote 2026 — Full Session", duration: 3492, uploader: "Northfield Conference", webpageUrl: "https://video.example.com/watch/4kz8",
  thumbnail: art("#27465f", "#3a2a4a", "#0b0e0d", 640, 360), subtitles: [],
  formats: [mf("2160p", 2160, "av01", 1420), mf("1440p", 1440, "vp9", 890), mf("1080p", 1080, "avc1", 574), mf("720p", 720, "avc1", 298), mf("480p", 480, "avc1", 141)],
};

const trackDefs: [string, string, number][] = [
  ["Neon Harbour", "Kestrel Hollow", 222], ["Glass Avenue", "Mira Vance", 245], ["Tail Lights", "The Slow Hours", 198], ["Overpass", "Kestrel Hollow", 301],
  ["Sodium Rain", "Juno & Ash", 236], ["Last Exit Home", "Mira Vance", 267], ["Static Bloom", "The Slow Hours", 189], ["Dawn Shift", "Juno & Ash", 284],
];
export const PLAYLIST = {
  kind: "playlist", service: "spotify", title: "Late Night Drive", owner: "Mira Vance", total: 8, note: null,
  coverUrl: art("#1f3a5c", "#5b2a58", "#e9b14a"), sourceUrl: "https://music.example.com/playlist/late-night-drive",
  tracks: trackDefs.map(([title, artist, duration], i) => ({
    id: `t${i}`, title, artists: [artist], album: "Late Night Drive", albumArtist: "Various Artists", trackNumber: i + 1, trackTotal: 8, discNumber: 1,
    year: 2026, date: "2026-03-14", genre: "Synthwave", duration, coverUrl: art(`hsl(${i * 37 + 200} 45% 38%)`, `hsl(${i * 37 + 240} 50% 20%)`, "#e9b14a", 160, 160),
    isrc: null, sourceUrl: `https://music.example.com/track/${i}`, service: "spotify", source: { kind: "match" }, explicit: false, unavailable: null,
  })),
};

hooks.analyze_video = async () => { await new Promise(r => setTimeout(r, state.analyzeDelay)); return JSON.parse(JSON.stringify(KEYNOTE)); };
hooks.analyze_music = async () => { await new Promise(r => setTimeout(r, state.analyzeDelay)); return JSON.parse(JSON.stringify(PLAYLIST)); };
hooks.start_video_download = async (a: any) => {
  const id = `vj_${Math.random().toString(36).slice(2, 6)}`;
  const fmt = a.format ?? KEYNOTE.formats[2];
  state.video.unshift({
    id, title: a.title, webpageUrl: a.webpageUrl, formatId: fmt.id, totalBytes: fmt.filesize, thumbnail: KEYNOTE.thumbnail, state: "downloading",
    downloadedBytes: 0, bytesPerSecond: 0, etaSeconds: null, error: null, path: null, createdAt: iso(), validation: fmt.validation ?? null,
  });
  state.sims.set(id, { speed: 26 * MB });
  return id;
};
hooks.start_music_download = async (a: any) => startMusicJobs(a.source, a.options.format);
hooks.pause_video_job = async (a: any) => { const j = find(a.id); if (j) { j.state = "paused"; j.bytesPerSecond = 0; } return null; };
hooks.resume_video_job = async (a: any) => { const j = find(a.id); if (j) j.state = "downloading"; return null; };
hooks.remove_video_job = async (a: any) => { state.video = state.video.filter(j => j.id !== a.id); return null; };
hooks.remove_music_job = async (a: any) => { state.music = state.music.filter(j => j.id !== a.id); return null; };

w.__mock.KEYNOTE = KEYNOTE;
