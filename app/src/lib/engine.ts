import { hasEngine } from "./native.ts";
/**
 * The one place that talks to the Rust engine.
 *
 * Every call funnels through `command()` so the browser case — `npm run dev`
 * with no native shell around it — degrades to a clear error instead of an
 * opaque "cannot find module" from deep inside a component.
 */
import type { DownloadView } from "./types";
import type { RowDelta } from "./polling";
import { startDrag } from "@crabnebula/tauri-plugin-drag";

// The native drag API requires a PNG preview. Keeping the tiny bundled app
// mark inline avoids filesystem assumptions and any runtime asset fetch.
const DRAG_PREVIEW =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAEEklEQVR42u1XX4hUVRj/nXPPnblzZ9pZqxVrRRuMICkMd5TpwTIlyl6iFHGRwoSoHtRYEV93oV6sBx96MwTRsJd6aE3qoShyQSqSlExMF9nVxj+otDt75+7MPff0nXPvrNO2yt5xFgk6w2EO555zvt/3ne/7zu8TiJqgHriuuwDguwD1MsAW0lwa7Wk1OvMSnTmolNxTrVZpDIu6ZA3hmUzuVVq0n3OeV0phLhpjDEqFlTDEm74/8akGoVGEWjjn7DMaOxqMXhv3djatlaRjHVJyg2XZZ4OgfpJpsyvFzhC6fLTAmGYuW0PGRBgGj1u2nR4gRM/FmotEJiUD0l4kvDKuZZHCjumumz1HRy2JTTRrs6tQoeZ7ZpxyXDDOkl6H3nCRAOT8RN5O25QMYWczKLxY0p6FC18fR23MAxM8OjoBEJE01Bj9ZCCR7sxh7Ud94LbAoeJW+DfGIOx00utgosWAMlfg3xwHF5YZtxo0ouWYNk7IjSPeTRO4x+1/AP9hAOweAqDcQ5mPN6VkHgFq4RHlrUjXuaY27t167PVYC+dsbgGY95we8zAIsHzHRggnZRJRz7sbzbxO0QnfBNBrmOqfPZlQqHtVrP5wO4p9vUZrRgAWPvMU3AfzOPfFMQIkIhCqjQD+IfyDbSju7MVfF8q4/NNp86+tUFhXgnN/HucHk4EQSYQ/S8J7+jZhbOQKvnrjPYz+cMKs6S49iZc+6cfybRuMJ367Yy9s1zEgoneiRQtMF14k4eOjV/Hl5n6MHvsFmY4OiHQaN86P4PLx01i8tojCC9oSHbO2xO0BxA4XVP1/Cx8i4flOckZpNEy5Lm4Oj6A8Awgdrs0hmwwACVj1/ltYsWtzk/ATkfB60MSOiKBMB0FkJdWRw8g3P98xMmaEpjdIfxIPLC2gtHsLJsrXcfS1AVwc+hXuvHlRuJmn+FbXcxn6Vv7xNwI6YPyk1Pc6upY9SlacvK0VNCWb1LRuOoC6NwkrZeOx9avJ23/HlbPDtMhGOMXa1Qy5WZFGgqqQOrqWLMbDTz+BPz7/3gCws85MDqk0KR2mzYUGUdRIaxUPjzy/EsveeQWVS9cM/9MHhFIaSnZntkkgiKTUPR/1ShW57i6c+ngQw0eHkMplzXU1kdI/hS6XyNu3k7dLE5Z09yFRd3d+Jxat6SGyOTHlkImypqbrpHG6M0vOOGTONFEV1wY0FlQlHWGZTKab1D5DW3JTRYPJcNxcAe62TCOhshYYh46N1yhMfCmtpWbKcbKbLIsfjhmtvmSLxmSKNtWI5FOkcVyaQUT5RW7xPO8Aa1SpBKKX6sN99DE7Ra1Zmx79+Lw4sVEdEr6thTeKU/3V0oUiWeGgLpdo6UM0d18bC1Qto0x/h6UUvb5f+a6h+N9WnrumxU5byAAAAABJRU5ErkJggg==";

/** Mirrors `zoltraak_core::supervisor::DownloadSnapshot`. */
export type DownloadSnapshot = DownloadView & {
  url: string;
  directory: string;
  priority: number;
  error: string | null;
  createdAt?: string;
};

/** Mirrors the Rust `ApiStatus` the pairing card is built from. */
export type ApiStatus = {
  listening: boolean;
  port: number;
  token: string;
  detail: string | null;
};

export type OutputStatus = { path: string | null; status: "present" | "missing" | "inaccessible" | "unknown"; detail: string | null };
export type LinkInspection = { filename: string; contentType: string | null; isHtml: boolean };
export type DestinationHealth ={ path: string; writable: boolean; detail: string | null };

export type CategoryKind = "extension" | "domain" | "regex";

/** Mirrors `zoltraak_core::Category`. */
export type Category = {
  id: number;
  name: string;
  kind: CategoryKind;
  pattern: string;
  directory: string;
  priority: number;
  enabled: boolean;
};

/** Mirrors `zoltraak_core::TimeWindow`. */
export type TimeWindow = {
  enabled: boolean;
  /** Minutes since local midnight, 0..1440. */
  startMinutes: number;
  stopMinutes: number;
};

/** Mirrors `zoltraak_core::CompletionAction`. */
export type CompletionAction =
  | { type: "none" }
  | { type: "open_folder" }
  | { type: "run_command"; command: string }
  | { type: "exit_app" };

/** Mirrors `zoltraak_core::EngineSettings`. */
export type EngineSettings = {
  bandwidthLimitBytesPerSec: number | null;
  defaultConnections: number | null;
  defaultDirectory: string | null;
  desktopNotifications?: boolean;
  playSoundOnComplete?: boolean;
  startOnLaunch?: boolean;
  closeToTray?: boolean;
  minimizeToTray?: boolean;
  deleteFilesOnRemove?: boolean;
  schedule: TimeWindow;
  onComplete: CompletionAction;
};

/** Factory defaults for `EngineSettings`. */
export const DEFAULT_ENGINE_SETTINGS: EngineSettings = {
  bandwidthLimitBytesPerSec: null,
  defaultConnections: null,
  defaultDirectory: null,
  desktopNotifications: true,
  playSoundOnComplete: true,
  startOnLaunch: false,
  closeToTray: true,
  minimizeToTray: false,
  deleteFilesOnRemove: true,
  schedule: {
    enabled: false,
    startMinutes: 0,
    stopMinutes: 0,
  },
  onComplete: { type: "none" },
};

/** Mirrors `zoltraak_core::DiscordMode`. */
export type DiscordMode = "webhook" | "bot";
export type DiscordProfileScope = "global" | "application" | "server";
export type DiscordBotProfile = {
  id?: string;
  username?: string;
  name?: string;
  avatar?: string | null;
  banner?: string | null;
  icon?: string | null;
  description?: string;
  tags?: string[] | null;
  nick?: string | null;
  bio?: string | null;
  bot_public?: boolean;
};
export type DiscordProfileFields = Partial<Pick<DiscordBotProfile, "username" | "avatar" | "banner" | "icon" | "description" | "tags" | "nick" | "bio">>;
export type DiscordBotDiagnostics = {
  credentialValid: boolean;
  destinationSelected: boolean;
  checks: { label: string; allowed: boolean }[];
};
export type DiscordSendOptions = {
  caption?: string;
  pingSender?: boolean;
  includeSourceUrl?: boolean;
  compressToFit?: boolean;
  expectedDestination?: string;
};

/** Mirrors `zoltraak_core::DiscordSettings`. Credentials are never in here. */
export type DiscordSettings = {
  enabled: boolean;
  mode: DiscordMode;
  autoSend: boolean;
  guildId: string | null;
  channelId: string | null;
  guildName: string | null;
  channelName: string | null;
  botUsername: string | null;
  uploadLimitBytes: number | null;
  botUploadLimitBytes?: number;
  webhookUsername?: string | null;
  webhookAvatarUrl?: string | null;
  webhookAvatarAssetId?: string | null;
  pingMessageTemplate?: string;
  discordUserId?: string | null;
  pingSender?: boolean;
  compressToFit: boolean;
  compressMaxHeight: number;
  messageTemplate: string;
  includeSourceUrl: boolean;
  notifyOnDelivery: boolean;
};

/** Mirrors `zoltraak_core::DiscordStatus`. */
export type DiscordStatus = {
  enabled: boolean;
  mode: DiscordMode;
  autoSend: boolean;
  hasCredential: boolean;
  /** Last four characters of the stored credential — never the whole thing. */
  credentialHint: string | null;
  destinationRef?: string | null;
  connected: boolean;
  botUsername: string | null;
  guildName: string | null;
  channelName: string | null;
  guildId: string | null;
  channelId: string | null;
  uploadLimitBytes: number;
  uploadLimitLabel: string;
  /** False when the limit is the assumed unboosted floor rather than read. */
  uploadLimitDetected: boolean;
  compressToFit: boolean;
  lastError: string | null;
};

/** Mirrors `zoltraak_core::discord::Connection`. */
export type DiscordConnection = {
  botUsername: string | null;
  guildName: string | null;
  channelName: string | null;
  channelId: string | null;
  guildId: string | null;
  inviteUrl: string | null;
  uploadLimitBytes: number;
};

/** Mirrors `zoltraak_core::discord::rest::Guild`. */
export type DiscordGuild = { id: string; name: string; icon: string | null };

/** Mirrors `zoltraak_core::discord::rest::Channel`. */
export type DiscordChannel = { id: string; name: string | null; kind: number; position: number };

export type DeliveryState = "pending" | "preparing" | "uploading" | "sent" | "deleted" | "skipped" | "failed";

/** Mirrors `zoltraak_core::Delivery`. */
export type Delivery = {
  id: string;
  source: "download" | "video" | "manual";
  sourceId: string | null;
  path: string;
  filename: string;
  bytes: number;
  state: DeliveryState;
  reason: string | null;
  messageUrl: string | null;
  /** Non-secret reference to the sender used for this message. */
  messageDestination?: string | null;
  /** What was actually uploaded, when the file was re-encoded to fit. */
  sentBytes: number | null;
  attempts: number;
  createdAt: string;
  updatedAt: string;
};

/** Mirrors `zoltraak_core::VideoFormat`. */
export type VideoValidation = {
  status: "unknown" | "inferred" | "verified";
  hasVideo: boolean | null;
  hasAudio: boolean | null;
  duration: number | null;
  videoDuration?: number | null;
  audioDuration?: number | null;
  contentId: string | null;
  samplesDecoded: boolean;
};

export type VideoFormat = {
  downloadOptions?: VideoDownloadOptions;
  id: string;
  container: string;
  videoCodec: string | null;
  audioCodec: string | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  bitrate: number | null;
  filesize: number | null;
  videoUrl: string | null;
  audioUrl: string | null;
  requiresMerge: boolean;
  /**
   * Whether `videoUrl`'s own stream carries audio. `null`/absent means the
   * engine never established it, which is the ordinary state for a plain
   * file; `false` is a positive finding — a DASH video track, or a probed
   * file with no sound track — and is the one worth showing.
   */
  hasAudio?: boolean | null;
  validation?: VideoValidation | null;
  /** Headers the stream must be fetched with (a CDN's client check). Pass back verbatim. */
  headers?: [string, string][];
};

/** Mirrors `zoltraak_core::VideoMetadata`. */
export type VideoMetadata = {
  subtitles?: VideoSubtitle[];
  title: string;
  thumbnail: string | null;
  duration: number | null;
  uploader: string | null;
  webpageUrl: string;
  formats: VideoFormat[];
};

export type VideoSubtitle = { language: string; label: string; url: string; extension: string; automatic: boolean };
export type VideoDownloadOptions = { thumbnail?: string | null; subtitle?: VideoSubtitle | null; liveSeconds?: number | null };

/** Mirrors `zoltraak_core::image::MediaKind`. */
export type MediaKind = "image" | "video";

/** Mirrors `zoltraak_core::ImageItem`. */
export type ImageItem = {
  id: string;
  kind: MediaKind;
  url: string;
  /** A smaller rendition for the grid; fall back to `url` when absent. */
  thumbnail: string | null;
  width: number | null;
  height: number | null;
  filesize: number | null;
  /** Container the engine settled on after reading the first bytes. No dot. */
  extension: string;
  /** Headers this URL specifically needs. Pass back verbatim when queueing. */
  headers?: [string, string][];
  /**
   * For a video item: every rendition, best first. Always non-empty on a
   * `video` and absent on an `image` — it is what decides the download route.
   * A clip goes to `startVideoDownload(formats[0])`, the only path that can
   * fetch a manifest, pair a split audio track and mux the result.
   */
  formats?: VideoFormat[];
  /**
   * Whether the first bytes confirmed a picture. `null` means the check could
   * not be made — shown as unverified, not as bad. A definite `false` never
   * reaches the client: the engine drops those before returning.
   */
  verified?: boolean | null;
};

/** Mirrors `zoltraak_core::ImageAlbum`. */
export type ImageAlbum = {
  title: string;
  sourceUrl: string;
  uploader: string | null;
  items: ImageItem[];
  completeness?: "complete" | "partial" | "unknown";
  expectedCount?: number;
  warnings?: string[];
};

/** Mirrors `zoltraak_core::music::Service`. */
export type MusicService =
  | "spotify"
  | "apple_music"
  | "deezer"
  | "youtube"
  | "youtube_music"
  | "soundcloud"
  | "bandcamp"
  | "direct"
  | "search";

/** Mirrors `zoltraak_core::music::SourceKind`. */
export type MusicSourceKind = "track" | "album" | "playlist" | "artist" | "search";

/** Mirrors `zoltraak_core::music::AudioSource`. Opaque to the UI: it is handed
 * back to the engine verbatim, which is what lets analysis stay stateless. */
export type AudioSource =
  | { kind: "match" }
  | { kind: "youtube"; videoId: string }
  | { kind: "direct"; url: string; headers?: [string, string][]; container: string | null }
  | { kind: "soundcloud"; trackId: number }
  | { kind: "bandcamp"; pageUrl: string; trackId: number };

/** Mirrors `zoltraak_core::music::MusicTrack`. */
export type MusicTrack = {
  id: string;
  title: string;
  artists: string[];
  album: string | null;
  albumArtist: string | null;
  trackNumber: number | null;
  trackTotal: number | null;
  discNumber: number | null;
  year: number | null;
  date: string | null;
  genre: string | null;
  /** Seconds. */
  duration: number | null;
  coverUrl: string | null;
  isrc: string | null;
  sourceUrl: string;
  service: MusicService;
  source: AudioSource;
  explicit: boolean;
  /** Why the service says it cannot be played, when it says so. */
  unavailable: string | null;
};

/** Mirrors `zoltraak_core::music::MusicSource`. */
export type MusicSource = {
  kind: MusicSourceKind;
  service: MusicService;
  title: string;
  owner: string | null;
  coverUrl: string | null;
  sourceUrl: string;
  tracks: MusicTrack[];
  /** What the service says the collection holds, when more than listed. */
  total: number | null;
  note: string | null;
};

/** Mirrors `zoltraak_core::music::AudioFormat`. */
export type AudioFormat = "best" | "mp3" | "m4a" | "opus" | "ogg" | "flac" | "wav" | "alac";

/** Mirrors `zoltraak_core::music::FolderMode`. */
export type FolderMode = "collection" | "custom" | "none";

/** Mirrors `zoltraak_core::music::OutputOptions`. */
export type OutputOptions = {
  format: AudioFormat;
  bitrateKbps: number | null;
  /** The music folder; `null` means the engine's default. */
  directory: string | null;
  folderMode: FolderMode;
  folderName: string | null;
  filenameTemplate: string;
  numberTracks: boolean;
  embedMetadata: boolean;
  embedCover: boolean;
  /** Look cover art up on Apple Music and Deezer when the source has none or
   * only a video still. */
  fetchArtwork: boolean;
  saveCoverFile: boolean;
  fetchLyrics: boolean;
  writePlaylistFile: boolean;
  skipExisting: boolean;
  normalizeLoudness: boolean;
};

/** Mirrors `zoltraak_core::music::MusicSettings`. */
export type MusicSettings = {
  output: OutputOptions;
  maxConcurrent: number;
  preferYoutubeMusic: boolean;
};

/** Mirrors `zoltraak_core::music::manager::MusicJobState`. */
export type MusicJobState =
  | "queued"
  | "resolving"
  | "downloading"
  | "converting"
  | "finished"
  | "failed"
  | "skipped"
  | "paused";

/** Mirrors `zoltraak_core::MusicJob`. */
export type MusicJob = {
  id: string;
  collectionId: string | null;
  collectionTitle: string | null;
  collectionUrl: string | null;
  position: number;
  track: MusicTrack;
  format: AudioFormat;
  state: MusicJobState;
  /** One line on what the job is doing right now. */
  stage: string | null;
  matchedUrl: string | null;
  matchedTitle: string | null;
  downloadedBytes: number;
  totalBytes: number | null;
  bytesPerSecond: number;
  etaSeconds: number | null;
  error: string | null;
  path: string | null;
  directory: string;
  createdAt: string;
  updatedAt: string;
  finishedAt: string | null;
};

/** Mirrors `zoltraak_core::music::manager::Enqueued`. */
export type MusicEnqueued = {
  collectionId: string | null;
  jobIds: string[];
  /** Already on disk, so not queued. */
  skipped: number;
  directory: string;
};

export type MusicTools = {
  /** The installed ffmpeg's version, or `null` when it is not on PATH. */
  ffmpeg: string | null;
  ffprobe: boolean;
};

/** Mirrors `zoltraak_core::remux::Container`. */
export type Container = "mp4" | "mkv" | "mov" | "webm";

export type ConversionFormat = Container | "gif" | "webp" | "png" | "jpg" | "bmp" | "tiff" | "mp3" | "m4a" | "alac" | "flac" | "wav" | "aiff" | "opus" | "ogg";
export type ConversionSettings = {
  format: ConversionFormat; mode: "auto" | "encode" | "copy"; quality: number;
  width: number | null; height: number | null; fps: number | null;
  startSeconds: number; endSeconds: number | null; durationSeconds: number | null;
  mute: boolean; audioBitrate: number; sampleRate: number | null; channels: number | null;
  audioTrack: number; normalize: boolean; fadeIn: number; fadeOut: number;
  normalizeTwoPass: boolean; loudnessLufs: number; truePeakDb: number;
  stripMetadata: boolean; background: string; loopCount: number; dither: string; lossless: boolean;
  /** Approximate size budget in MiB for video and lossy audio; null uses quality. */
  targetSizeMb: number | null;
  /** Filename pattern with {name}, {format}, {date} and {time}. */
  nameTemplate: string;
  /** The part of the picture to keep, as fractions (0 to 1) of the source. */
  crop: ConversionCrop | null;
  /** Clockwise degrees. */
  rotate: 0 | 90 | 180 | 270;
  flipHorizontal: boolean;
  flipVertical: boolean;
};
export type ConversionCrop = { x: number; y: number; width: number; height: number };
/** Mirrors `zoltraak_core::remux::manager::QueueState`. */
export type ConversionQueueState = { paused: boolean; parallelism: number; queued: string[] };
export type ConversionPreviewSample = {
  url: string; format: ConversionFormat; startSeconds: number; sampleSeconds: number;
  bytes: number; width: number | null; height: number | null;
};
/** Mirrors `zoltraak_core::conversion_watch::WatchRule`. */
export type ConversionWatchRule = {
  id: string; folder: string; extensions: string[]; settings: ConversionSettings;
  outputDirectory?: string | null; enabled: boolean;
};
export type ConversionWatchStatus = { ruleId: string; queued: number; lastError: string | null };
export type ConversionPlan = { streams: RemuxPlan; description: string; warnings: string[]; durationSeconds: number | null; outputExtension: string };
export type ConversionPlanRow = { path: string; plan: ConversionPlan | null; error: string | null };
export type ConversionCapabilities = { formats: ConversionFormat[]; ffprobe: boolean };

/** Mirrors `zoltraak_core::remux::StreamAction`. */
export type StreamAction = "copy" | "encode" | "absent";

/** Mirrors `zoltraak_core::remux::Plan`. */
export type RemuxPlan = { video: StreamAction; audio: StreamAction };

/** Mirrors `zoltraak_core::remux::MediaInfo`. */
export type MediaInfo = {
  path: string;
  /** The file's own extension, lowercased. */
  extension: string;
  videoCodec: string | null;
  audioCodec: string | null;
  audioTracks?: { index: number; codec: string; channels: number | null; sampleRate: number | null; language: string | null }[];
  unsupportedAnimation?: boolean;
  width: number | null;
  height: number | null;
  durationSeconds: number | null;
  bytes: number | null;
  /** True when ffprobe could not read it, so the codecs are unknown rather
   * than absent — the difference between "no sound" and "we cannot tell". */
  unprobed: boolean;
};

/** Mirrors `zoltraak_core::remux::manager::RemuxJobState`. */
export type RemuxJobState =
  | "queued"
  | "probing"
  | "running"
  | "finished"
  | "failed"
  | "skipped"
  | "cancelled";

/** Mirrors `zoltraak_core::RemuxJob`. */
export type RemuxJob = {
  id: string;
  input: string;
  filename: string;
  container: Container;
  conversion?: ConversionSettings | null;
  /** What the finished file turned out to be; compare with `source`. */
  outputInfo?: MediaInfo | null;
  outputDirectory?: string | null;
  state: RemuxJobState;
  stage: string | null;
  source: MediaInfo | null;
  plan: RemuxPlan | null;
  /** Whether the run copies the streams rather than re-encoding them. */
  lossless: boolean | null;
  percent: number;
  /** ffmpeg's own multiple of realtime, e.g. `48.2`. */
  speed: number | null;
  inputBytes: number | null;
  outputBytes: number | null;
  output: string | null;
  error: string | null;
  replaceSource: boolean;
  createdAt: string;
  updatedAt: string;
  finishedAt: string | null;
};

/** Mirrors `zoltraak_core::video::manager::VideoJobState`. */
export type VideoJobState = "downloading" | "merging" | "paused" | "finished" | "failed";

/** Mirrors `zoltraak_core::VideoJob`. */
export type VideoJob = {
  thumbnail?: string | null;
  validation?: VideoValidation | null;
  id: string;
  title: string;
  webpageUrl: string;
  formatId?: string | null;
  totalBytes?: number | null;
  state: VideoJobState;
  downloadedBytes: number;
  /** Live throughput; 0 when the job is not running. */
  bytesPerSecond: number;
  etaSeconds: number | null;
  error: string | null;
  path: string | null;
  createdAt: string;
};

/** A browser session the extension shared, as the settings screen may show
 * it: the site, when, and which cookie *names* — never a value. */
export type SessionSummary = {
  domain: string;
  updatedAt: string;
  cookieNames: string[];
  hasPoToken: boolean;
  signedIn: boolean;
};

export type DetectedBrowser = {
  id: string;
  name: string;
  kind: "firefox" | "chromium";
  running: boolean;
  executablePath: string | null;
  installed: boolean;
};

export type ExtensionClientSummary = {
  clientId: string;
  browserId: string;
  browserName: string;
  extensionVersion: string;
  lastSeenAtMs: number | null;
  online: boolean;
  preferred: boolean;
  active: boolean;
  temporary: boolean;
};

export type ExtensionSourceState = {
  revision: number;
  preferredClientId: string | null;
  activeClientId: string | null;
  clients: ExtensionClientSummary[];
};

export type ExtensionInstallResult = {
  browserId: string;
  browserName: string;
  success: boolean;
  message: string;
  extensionPath: string;
  nextStep?: "browser_confirmation" | "restart_browser" | "manual_install" | "reload_requested";
};

export type PurgeRegenerateResult = {
  success: boolean;
  message: string;
  purgedPaths: string[];
  regeneratedBrowsers: string[];
};

export const QUEUE_EVENT = "zoltraak://queue";
export const DOWNLOAD_PREFILL_EVENT = "zoltraak://download-prefill";
export const VIDEO_PREFILL_EVENT = "zoltraak://video-prefill";
export const IMAGE_PREFILL_EVENT = "zoltraak://image-prefill";
export const MUSIC_PREFILL_EVENT = "zoltraak://music-prefill";

/** Whether the native shell is present. False under a plain `vite dev`. */
export { hasEngine } from "./native.ts";

/** The engine returns `{ message }`; anything else is a real crash. */
function toMessage(error: unknown): string {
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

async function command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  if (!hasEngine()) {
    throw new Error("The Zoltraak engine is only available in the desktop app.");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>(name, args);
  } catch (error) {
    throw new Error(toMessage(error));
  }
}

export const engine = {
  ready: () => command<boolean>("engine_ready"),
  defaultDirectory: () => command<string>("default_directory"),
  /**
   * Open the folder picker, seeded at `start`. Resolves to `null` when the
   * user cancels — which is a choice to keep the current destination, not an
   * error to report.
   */
  chooseDirectory: (start?: string) =>
    command<string | null>("choose_directory", { start }),
  snapshot: () => command<DownloadSnapshot[]>("queue_snapshot"),
  apiStatus: () => command<ApiStatus>("api_status"),

  add: (
    url: string,
    directory?: string,
    connections?: number,
    /** A name to save under; the URL's own is used when omitted. */
    filename?: string,
    /** Headers this URL specifically needs (a CDN's client check). */
    headers?: [string, string][],
    /** Whether to auto-send to Discord upon completion. */
    autoDiscord?: boolean,
  ) => command<string>("add_download", { url, directory, connections, filename, headers, autoDiscord }),

  /** The name a link would save under, and whether that is a web page. */
  inspectLink: (url: string, headers?: [string, string][]) =>
    command<LinkInspection>("inspect_download_link", { url, headers }),

  pause: (id: string) => command<void>("pause_download", { id }),
  resume: (id: string) => command<void>("resume_download", { id }),
  remove: (id: string, deleteFile = false) => command<void>("remove_download", { id, deleteFile }),
  reveal: (id: string) => command<void>("reveal_download", { id }),
  fileStatus: (id: string) => command<OutputStatus>("download_file_status", { id }),
  fileStatuses: async (ids: string[]): Promise<Record<string, OutputStatus>> => {
    if (ids.length === 0) return {};
    try {
      return await command<Record<string, OutputStatus>>("download_file_statuses", { ids });
    } catch {
      const results: Record<string, OutputStatus> = {};
      await Promise.all(
        ids.map(async (id) => {
          try {
            results[id] = await command<OutputStatus>("download_file_status", { id });
          } catch {}
        })
      );
      return results;
    }
  },
  downloadThumbnail: (id: string) => command<string | null>("download_thumbnail", { id }),
  locateFile: (id: string) => command<string | null>("locate_download_file", { id }),
  redownloadMissing: (id: string) => command<void>("redownload_missing", { id }),
  destinationHealth: () => command<DestinationHealth>("destination_health"),
  open: (id: string) => command<void>("open_download", { id }),
  /** Start a native copy drag for a finished file resolved by the engine. */
  dragOut: async (id: string) => {
    const path = await command<string>("drag_download_path", { id });
    await startDrag({ item: [path], icon: DRAG_PREVIEW, mode: "copy" });
  },
  revealFolder: (path?: string) => command<void>("reveal_folder", { path }),

  categories: () => command<Category[]>("list_categories"),
  addCategory: (
    name: string,
    kind: CategoryKind,
    pattern: string,
    directory: string,
    priority?: number,
  ) => command<number>("add_category", { name, kind, pattern, directory, priority }),
  setCategoryEnabled: (id: number, enabled: boolean) =>
    command<void>("set_category_enabled", { id, enabled }),
  removeCategory: (id: number) => command<void>("remove_category", { id }),

  settings: () => command<EngineSettings>("get_settings"),
  updateSettings: (settings: EngineSettings) =>
    command<void>("update_settings", { settings }),

  analyzeVideo: (url: string, headers?: [string, string][], useFallback = false) =>
    command<VideoMetadata>("analyze_video", { url, headers, useFallback }),
  /** The pictures a post holds. Each item is queued through `add` — there is
   * no separate image download command, because there is no separate job. */
  analyzeImages: (url: string, headers?: [string, string][]) =>
    command<ImageAlbum>("analyze_images", { url, headers }),
  startVideoDownload: (
    title: string,
    webpageUrl: string,
    format: VideoFormat,
    directory?: string,
    headers?: [string, string][],
    /** Whether to auto-send to Discord upon completion. */
    autoDiscord?: boolean,
    options?: VideoDownloadOptions,
  ) => command<string>("start_video_download", { title, webpageUrl, format: options ? { ...format, downloadOptions: { ...format.downloadOptions, ...options } } : format, directory, headers, autoDiscord }),
  videoJobs: () => command<VideoJob[]>("list_video_jobs"),
  removeVideoJob: (id: string, deleteFile = false) => command<void>("remove_video_job", { id, deleteFile }),
  pauseVideoJob: (id: string) => command<void>("pause_video_job", { id }),
  resumeVideoJob: (id: string) => command<void>("resume_video_job", { id }),

  /** Where music lands by default: a `Music` folder in the download folder. */
  musicDirectory: () => command<string>("music_directory"),
  /** A link resolves to its tracks; anything that is not a link is searched. */
  analyzeMusic: (input: string, headers?: [string, string][]) =>
    command<MusicSource>("analyze_music", { input, headers }),
  /** Queue a source's tracks. Pass the source holding only the kept tracks. */
  startMusicDownload: (source: MusicSource, options: OutputOptions, autoDiscord?: boolean) =>
    command<MusicEnqueued>("start_music_download", { source, options, autoDiscord }),
  musicJobs: () => command<MusicJob[]>("list_music_jobs"),
  removeMusicJob: (id: string, deleteFile = false) => command<void>("remove_music_job", { id, deleteFile }),
  pauseMusicJob: (id: string) => command<void>("pause_music_job", { id }),
  /** `force` downloads a skipped track even though its file exists. */
  resumeMusicJob: (id: string, force?: boolean) =>
    command<void>("resume_music_job", { id, force }),
  removeMusicCollection: (collectionId: string) =>
    command<number>("remove_music_collection", { collectionId }),
  retryMusicCollection: (collectionId: string) =>
    command<number>("retry_music_collection", { collectionId }),
  clearMusicDone: () => command<number>("clear_music_done"),
  musicSettings: () => command<MusicSettings>("music_settings"),
  updateMusicSettings: (settings: MusicSettings) =>
    command<MusicSettings>("update_music_settings", { settings }),
  musicTools: () => command<MusicTools>("music_tools"),
  installFfmpeg: () => command<MusicTools>("install_ffmpeg"),
  checkForUpdates: () => command<UpdateInfo>("check_for_updates"),
  downloadUpdate: () => command<boolean>("download_update"),
  updateProgress: () => command<UpdateProgress | null>("update_progress"),
  installOnNextStart: () => command<void>("install_on_next_start"),
  installAndRestart: () => command<void>("install_and_restart"),
  getAppVersion: () => command<string>("get_app_version"),
  getExtensionVersion: () => command<string>("get_extension_version"),
  getFirefoxExtensionVersion: () => command<string | null>("get_firefox_extension_version"),

  /** Describe local files, walking any folders among them. */
  scanMedia: (paths: string[]) => command<MediaInfo[]>("scan_media", { paths }),
  /** The native file picker, filtered to media. An empty list is a cancel. */
  chooseMediaFiles: () => command<MediaInfo[]>("choose_media_files"),
  mediaCapabilities: () => command<ConversionCapabilities>("media_capabilities"),
  planConversion: (paths: string[], settings: ConversionSettings) => command<ConversionPlanRow[]>("plan_conversion", { paths, settings }),
  startConversions: (paths: string[], settings: ConversionSettings, directory?: string, replaceSource = false) => command<string[]>("start_conversions", { paths, settings, directory, replaceSource }),
  /** Repackage local files. No directory uses the working folder's Converted subfolder. */
  startRemux: (
    paths: string[],
    container: Container,
    directory?: string,
    replaceSource?: boolean,
  ) => command<string[]>("start_remux", { paths, container, directory, replaceSource }),
  /** Repackage what a finished row in the register points at. The engine
   * resolves the id to a path — the three subsystems store it differently. */
  remuxDownload: (id: string, container: Container, directory?: string) =>
    command<string>("remux_download", { id, container, directory }),
  remuxJobs: () => command<RemuxJob[]>("list_remux_jobs"),
  remuxQueueState: () => command<ConversionQueueState>("remux_queue_state"),
  setRemuxPaused: (paused: boolean) => command<void>("set_remux_paused", { paused }),
  setRemuxParallelism: (parallelism: number) => command<void>("set_remux_parallelism", { parallelism }),
  moveRemuxJob: (id: string, movement: "top" | "up" | "down") => command<void>("move_remux_job", { id, movement }),
  /** A frame of the source as a data URL. */
  conversionThumbnail: (path: string, atSeconds: number, maxWidth: number) => command<string>("conversion_thumbnail", { path, atSeconds, maxWidth }),
  conversionWaveform: (path: string, buckets: number) => command<number[]>("conversion_waveform", { path, buckets }),
  /** Render a few seconds with a recipe; only while nothing else is converting. */
  conversionPreview: (path: string, settings: ConversionSettings, startSeconds: number, lengthSeconds: number) => command<ConversionPreviewSample>("conversion_preview", { path, settings, startSeconds, lengthSeconds }),
  listWatchRules: () => command<{ rules: ConversionWatchRule[]; statuses: ConversionWatchStatus[] }>("list_watch_rules"),
  saveWatchRules: (rules: ConversionWatchRule[]) => command<void>("save_watch_rules", { rules }),
  cancelRemuxJob: (id: string) => command<void>("cancel_remux_job", { id }),
  removeRemuxJob: (id: string) => command<void>("remove_remux_job", { id }),
  retryRemuxJob: (id: string, directory?: string) =>
    command<void>("retry_remux_job", { id, directory }),
  clearRemuxDone: () => command<number>("clear_remux_done"),
  openRemuxOutput: (id: string) => command<void>("open_remux_output", { id }),

  discordStatus: () => command<DiscordStatus>("discord_status"),
  discordSettings: () => command<DiscordSettings>("discord_settings"),
  updateDiscordSettings: (settings: DiscordSettings) =>
    command<DiscordSettings>("update_discord_settings", { settings }),
  discordConnectWebhook: (url: string) =>
    command<DiscordConnection>("discord_connect_webhook", { url }),
  discordConnectBot: (token: string) =>
    command<DiscordConnection>("discord_connect_bot", { token }),
  discordDisconnect: () => command<void>("discord_disconnect"),
  discordReconnect: () => command<DiscordConnection>("discord_reconnect"),
  discordGuilds: () => command<DiscordGuild[]>("discord_guilds"),
  discordChannels: (guildId: string) =>
    command<DiscordChannel[]>("discord_channels", { guildId }),
  discordSetTarget: (guildId: string, channelId: string) =>
    command<DiscordSettings>("discord_set_target", { guildId, channelId }),
  discordInviteUrl: () => command<string>("discord_invite_url"),
  /** `avatar` is a `data:` URI, which is the shape Discord's API wants. */
  discordUpdateBot: (username?: string, avatar?: string) =>
    command<string>("discord_update_bot", { username, avatar }),
  discordBotProfile: (scope: DiscordProfileScope, guildId?: string, fields?: DiscordProfileFields) =>
    command<DiscordBotProfile>("discord_bot_profile", { request: { scope, guildId, fields } }),
  discordBotDiagnostics: () => command<DiscordBotDiagnostics>("discord_bot_diagnostics"),
  discordTest: () => command<string>("discord_test"),
  /** Takes queue or video-job ids; the engine resolves them to paths. */
  discordSend: (ids: string[], options?: DiscordSendOptions) => command<number>("discord_send", { ids, options }),
  discordDeliveries: () => command<Delivery[]>("discord_deliveries"),
  discordRetryDelivery: (id: string) => command<boolean>("discord_retry_delivery", { id }),
  discordClearDeliveries: () => command<number>("discord_clear_deliveries"),
  /** Delete the Discord message associated with a delivery. Returns true when deleted. */
  discordDeleteMessage: (id: string) => command<boolean>("discord_delete_message", { id }),
  discordTagAutoSend: (ids: string[]) => command<void>("discord_tag_auto_send", { ids }),

  sessions: () => command<SessionSummary[]>("list_sessions"),
  forgetSession: (domain: string) => command<void>("forget_session", { domain }),
  forgetAllSessions: () => command<void>("forget_all_sessions"),
  extensionSourceState: () => command<ExtensionSourceState>("extension_source_state"),
  selectExtensionSource: (clientId: string) =>
    command<ExtensionSourceState>("select_extension_source", { clientId }),

  detectBrowsers: () => command<DetectedBrowser[]>("detect_browsers"),
  importFirefoxExtension: () =>
    command<ExtensionInstallResult[]>("import_firefox_extension"),
  installExtension: (browserId: string = "auto") =>
    command<ExtensionInstallResult[]>("install_extension", { browserId }),
  removeExtension: (browserId: string = "auto") =>
    command<ExtensionInstallResult[]>("remove_extension", { browserId }),
  reloadExtension: (browserId: string = "auto") =>
    command<ExtensionInstallResult[]>("reload_extension", { browserId }),
  relaunchBrowser: (browserId: string) =>
    command<ExtensionInstallResult>("relaunch_browser", { browserId }),
  getExtensionDirectory: (browserId: string) =>
    command<string>("get_extension_directory", { browserId }),
  openExtensionManager: (browserId: string) => command<void>("open_extension_manager", { browserId }),
  purgeAndRegenerateExtensions: () =>
    command<PurgeRegenerateResult>("purge_and_regenerate_extensions"),
  openGuide: () => command<void>("open_guide"),
  openLegal: () => command<void>("open_legal"),
  openUrl: (url: string) => command<void>("open_url", { url }),
  openDiscordMessage: (url: string) => command<void>("open_discord_message", { url }),
};

export async function openUrl(url: string): Promise<void> {
  if (hasEngine()) {
    try {
      await engine.openUrl(url);
      return;
    } catch {
      // Fall through to browser window open
    }
  }
  window.open(url, "_blank");
}

export async function openDiscordPortal(): Promise<void> {
  await openUrl("https://discord.com/developers/applications");
}

export async function openDiscordMessage(url: string): Promise<void> {
  if (hasEngine()) return engine.openDiscordMessage(url);
  await openUrl(url);
}

export async function openGuide(): Promise<void> {
  if (hasEngine()) {
    try {
      await engine.openGuide();
      return;
    } catch {
      // Fall through to browser window open
    }
  }
  window.open("http://127.0.0.1:1450/guide", "_blank");
}

export async function openLegal(): Promise<void> {
  if (hasEngine()) {
    try {
      await engine.openLegal();
      return;
    } catch {
      // Fall through to browser window open
    }
  }
  window.open("http://127.0.0.1:1450/legal", "_blank");
}


/**
 * Subscribe to queue pushes. Returns an unsubscribe function.
 *
 * The listener is registered asynchronously, so unsubscribing before it
 * resolves has to be handled explicitly or a closed window keeps receiving.
 */
/** A full list (after a command) or a delta from the native emitter. */
export type QueuePush = DownloadSnapshot[] | RowDelta<DownloadSnapshot>;

export function onQueue(handler: (push: QueuePush) => void): () => void {
  let stop: (() => void) | undefined;
  let cancelled = false;

  if (hasEngine()) {
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const unlisten = await listen<QueuePush>(QUEUE_EVENT, (event) =>
          handler(event.payload),
        );
        if (cancelled) unlisten();
        else stop = unlisten;
      } catch {
        // Event listening fallback or permission denied
      }
    })();
  }

  return () => {
    cancelled = true;
    stop?.();
  };
}

/**
 * Subscribe to "fill the Video view with this URL" pushes from the engine —
 * driven by the extension's "Send video to Zoltraak" action. Returns an
 * unsubscribe function. Same async-registration caveat as {@link onQueue}.
 */
export type VideoPrefillPayload = {
  url: string;
  /** Sanitized headers (a session cookie, most often) the extension captured
   * for `url` — pass these into `analyzeVideo`/`startVideoDownload` so a
   * login-gated page resolves with the same session it was captured under. */
  headers: [string, string][];
  /** When true (the floating "Download this video" button), the Video view
   * should immediately download the best-quality result after analyzing rather
   * than waiting for a format pick. */
  autoDownload?: boolean;
  /** The source page's title, used only as a fallback filename when the
   * analyzed metadata has none of its own (a raw CDN link). */
  title?: string;
};

export function onVideoPrefill(handler: (payload: VideoPrefillPayload) => void): () => void {
  let stop: (() => void) | undefined;
  let cancelled = false;

  if (hasEngine()) {
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const unlisten = await listen<VideoPrefillPayload>(VIDEO_PREFILL_EVENT, (event) =>
          handler(event.payload),
        );
        if (cancelled) unlisten();
        else stop = unlisten;
      } catch {
        // Event listening fallback or permission denied
      }
    })();
  }

  return () => {
    cancelled = true;
    stop?.();
  };
}

export interface FfmpegInstallProgress {
  phase: "downloading" | "extracting" | "completed" | "error";
  percent: number;
  message: string;
}

export const FFMPEG_EVENT = "ffmpeg-install-progress";

export function onFfmpegInstallProgress(handler: (payload: FfmpegInstallProgress) => void): () => void {
  let stop: (() => void) | undefined;
  let cancelled = false;

  if (hasEngine()) {
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const unlisten = await listen<FfmpegInstallProgress>(FFMPEG_EVENT, (event) =>
          handler(event.payload),
        );
        if (cancelled) unlisten();
        else stop = unlisten;
      } catch {
        // Event listening fallback
      }
    })();
  }

  return () => {
    cancelled = true;
    stop?.();
  };
}

export interface UpdateInfo {
  available: boolean;
  currentVersion: string;
  latestVersion: string | null;
  body: string | null;
  date: string | null;
  error: string | null;
}

export interface UpdateProgress {
  phase: "downloading" | "ready" | "error";
  percent: number;
  downloaded: number;
  total: number;
  version: string;
  message: string | null;
}

export const EVENT_UPDATE_AVAILABLE = "update-available";
export const EVENT_UPDATE_PROGRESS = "update-progress";
export const EVENT_UPDATE_READY = "update-ready";
export const EVENT_UPDATE_ERROR = "update-error";

export function onUpdateAvailable(handler: (payload: { version: string; body?: string }) => void): () => void {
  return listenEvent<{ version: string; body?: string }>(EVENT_UPDATE_AVAILABLE, handler);
}

export function onUpdateProgress(handler: (payload: UpdateProgress) => void): () => void {
  return listenEvent<UpdateProgress>(EVENT_UPDATE_PROGRESS, handler);
}

export function onUpdateReady(handler: (payload: { version: string }) => void): () => void {
  return listenEvent<{ version: string }>(EVENT_UPDATE_READY, handler);
}

function listenEvent<T>(eventName: string, handler: (payload: T) => void): () => void {
  let stop: (() => void) | undefined;
  let cancelled = false;

  if (hasEngine()) {
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const unlisten = await listen<T>(eventName, (event) => handler(event.payload));
        if (cancelled) unlisten();
        else stop = unlisten;
      } catch {
        // Event listening fallback
      }
    })();
  }

  return () => {
    cancelled = true;
    stop?.();
  };
}

export type DownloadPrefillPayload = {
  url: string;
  headers: [string, string][];
};

export type MusicPrefillPayload = {
  url: string;
  headers: [string, string][];
};

export type ImagePrefillPayload = {
  url: string;
  headers: [string, string][];
};

export function onImagePrefill(handler: (payload: ImagePrefillPayload) => void): () => void {
  return listenEvent<ImagePrefillPayload>(IMAGE_PREFILL_EVENT, handler);
}

/**
 * Subscribe to "fill the Music view with this link" pushes — the extension's
 * "Download music with Zoltraak". Same async-registration caveat as
 * {@link onQueue}.
 */
export function onMusicPrefill(handler: (payload: MusicPrefillPayload) => void): () => void {
  let stop: (() => void) | undefined;
  let cancelled = false;

  if (hasEngine()) {
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const unlisten = await listen<MusicPrefillPayload>(MUSIC_PREFILL_EVENT, (event) =>
          handler(event.payload),
        );
        if (cancelled) unlisten();
        else stop = unlisten;
      } catch {
        // Event listening fallback or permission denied
      }
    })();
  }

  return () => {
    cancelled = true;
    stop?.();
  };
}

export function onDownloadPrefill(handler: (payload: DownloadPrefillPayload) => void): () => void {
  let stop: (() => void) | undefined;
  let cancelled = false;

  if (hasEngine()) {
    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const unlisten = await listen<DownloadPrefillPayload>(DOWNLOAD_PREFILL_EVENT, (event) =>
          handler(event.payload),
        );
        if (cancelled) unlisten();
        else stop = unlisten;
      } catch {
        // Event listening fallback or permission denied
      }
    })();
  }

  return () => {
    cancelled = true;
    stop?.();
  };
}

export type AppNavigatePayload = {
  section: string;
  tab?: string;
};

export const APP_NAVIGATE_EVENT = "zoltraak://navigate";

export function onAppNavigate(handler: (payload: AppNavigatePayload) => void): () => void {
  return listenEvent<AppNavigatePayload>(APP_NAVIGATE_EVENT, handler);
}
