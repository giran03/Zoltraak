/**
 * Completion sound management for Zoltraak.
 *
 * Downloads should give audible feedback on finish when configured to do so.
 * When a custom audio file is uploaded, it is cached in memory and persisted
 * in IndexedDB. When no custom file exists, the engine synthesizes a clean,
 * harmonic 3-tone bell chime using Web Audio API, requiring zero asset downloads
 * or external dependencies.
 */

export type CustomSoundMeta = {
  name: string;
  size: number;
  type: string;
  duration: number;
  updatedAt: number;
};

const DB_NAME = "zoltraak_audio_db";
const DB_VERSION = 1;
const STORE_NAME = "audio_blobs";
const CUSTOM_SOUND_KEY = "custom_completion_sound";
const VOLUME_KEY = "zoltraak_sound_volume";
const LEGACY_VOLUME_KEY = "opendm_sound_volume";
const DEFAULT_VOLUME = 0.7;
const MAX_FILE_SIZE = 10 * 1024 * 1024; // 10 MB

// Shared AudioContext instance initialized lazily on first user interaction.
let sharedAudioCtx: AudioContext | null = null;

function getAudioContext(): AudioContext | null {
  if (typeof window === "undefined") return null;
  const AudioCtxClass =
    window.AudioContext ||
    (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
  if (!AudioCtxClass) return null;
  if (!sharedAudioCtx || sharedAudioCtx.state === "closed") {
    sharedAudioCtx = new AudioCtxClass();
  }
  if (sharedAudioCtx.state === "suspended") {
    void sharedAudioCtx.resume().catch(() => {});
  }
  return sharedAudioCtx;
}

// In-memory cache for instant zero-latency playback.
let cachedCustomBuffer: AudioBuffer | null = null;
let cachedMeta: CustomSoundMeta | null = null;
let dbInitialized = false;
let currentlyPlayingSource: AudioBufferSourceNode | null = null;
let lastSoundPlayTime = 0;

/** Open or create the IndexedDB database. */
function openDB(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    if (typeof indexedDB === "undefined") {
      reject(new Error("IndexedDB is not available in this environment."));
      return;
    }
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME);
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/** Retrieve persisted custom sound from IndexedDB. */
async function loadStoredCustomSound(): Promise<{
  buffer: ArrayBuffer;
  meta: CustomSoundMeta;
} | null> {
  try {
    const db = await openDB();
    return new Promise((resolve) => {
      const tx = db.transaction(STORE_NAME, "readonly");
      const store = tx.objectStore(STORE_NAME);
      const req = store.get(CUSTOM_SOUND_KEY);
      req.onsuccess = () => resolve(req.result || null);
      req.onerror = () => resolve(null);
    });
  } catch {
    return null;
  }
}

/** Save custom sound to IndexedDB. */
async function storeCustomSound(data: {
  buffer: ArrayBuffer;
  meta: CustomSoundMeta;
}): Promise<void> {
  const db = await openDB();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, "readwrite");
    const store = tx.objectStore(STORE_NAME);
    const req = store.put(data, CUSTOM_SOUND_KEY);
    req.onsuccess = () => resolve();
    req.onerror = () => reject(req.error);
  });
}

/** Delete custom sound from IndexedDB. */
async function deleteStoredCustomSound(): Promise<void> {
  try {
    const db = await openDB();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readwrite");
      const store = tx.objectStore(STORE_NAME);
      const req = store.delete(CUSTOM_SOUND_KEY);
      req.onsuccess = () => resolve();
      req.onerror = () => reject(req.error);
    });
  } catch {
    // Ignore cleanup error
  }
}

/** Initialize in-memory cache from IndexedDB once. */
export async function initCustomSound(): Promise<CustomSoundMeta | null> {
  if (dbInitialized && cachedMeta) return cachedMeta;
  dbInitialized = true;

  try {
    const stored = await loadStoredCustomSound();
    if (!stored) {
      cachedCustomBuffer = null;
      cachedMeta = null;
      return null;
    }
    const ctx = getAudioContext();
    if (ctx) {
      const decoded = await ctx.decodeAudioData(stored.buffer.slice(0));
      cachedCustomBuffer = decoded;
      cachedMeta = stored.meta;
      return cachedMeta;
    }
    cachedMeta = stored.meta;
    return cachedMeta;
  } catch {
    cachedCustomBuffer = null;
    cachedMeta = null;
    return null;
  }
}

/** Get currently active custom sound metadata if uploaded. */
export function getCustomSoundMeta(): CustomSoundMeta | null {
  return cachedMeta;
}

let currentVolume = DEFAULT_VOLUME;

/** Read configured sound volume (0.0 to 1.0). */
export function getSoundVolume(): number {
  if (typeof localStorage !== "undefined") {
    try {
      const raw = localStorage.getItem(VOLUME_KEY) || localStorage.getItem(LEGACY_VOLUME_KEY);
      if (raw) {
        const parsed = parseFloat(raw);
        if (Number.isFinite(parsed)) return Math.max(0, Math.min(1, parsed));
      }
    } catch {
      // Ignore storage read error
    }
  }
  return currentVolume;
}

/** Update configured sound volume (0.0 to 1.0). */
export function setSoundVolume(volume: number): void {
  const clamped = Math.max(0, Math.min(1, volume));
  currentVolume = clamped;
  if (typeof localStorage !== "undefined") {
    try {
      localStorage.setItem(VOLUME_KEY, clamped.toFixed(2));
    } catch {
      // Ignore storage write error
    }
  }
}

/**
 * Synthesize the default completion sound effect:
 * A rich, gentle 3-tone harmonic chime (C5 523Hz -> E5 659Hz -> G5 784Hz)
 * with a subtle higher shimmer (C6 1046Hz) and soft exponential decay.
 */
export function playDefaultSfx(volume = getSoundVolume()): () => void {
  const ctx = getAudioContext();
  if (!ctx || volume <= 0) return () => {};

  const now = ctx.currentTime;
  const masterGain = ctx.createGain();
  masterGain.gain.setValueAtTime(volume * 0.45, now);
  masterGain.connect(ctx.destination);

  // Frequencies for our harmonic 3-tone progression
  const notes = [
    { freq: 523.25, time: 0.00, dur: 0.55 }, // C5
    { freq: 659.25, time: 0.09, dur: 0.60 }, // E5
    { freq: 783.99, time: 0.18, dur: 0.75 }, // G5
    { freq: 1046.5, time: 0.24, dur: 0.80 }, // C6 overtone shimmer
  ];

  const oscillators: OscillatorNode[] = [];

  for (const { freq, time, dur } of notes) {
    const start = now + time;
    const osc = ctx.createOscillator();
    const noteGain = ctx.createGain();

    // Warm sine wave fundamental with soft triangle overtone
    osc.type = "sine";
    osc.frequency.setValueAtTime(freq, start);

    // Smooth envelope: fast 8ms attack to prevent click, exponential decay to 0.0001
    noteGain.gain.setValueAtTime(0.0001, start);
    noteGain.gain.exponentialRampToValueAtTime(0.65, start + 0.012);
    noteGain.gain.exponentialRampToValueAtTime(0.0001, start + dur);

    osc.connect(noteGain);
    noteGain.connect(masterGain);

    osc.start(start);
    osc.stop(start + dur + 0.05);
    oscillators.push(osc);
  }

  return () => {
    try {
      oscillators.forEach((osc) => osc.stop());
      masterGain.disconnect();
    } catch {
      // Ignore if already stopped
    }
  };
}

/**
 * Play a custom audio buffer through a dedicated gain node.
 */
function playAudioBuffer(buffer: AudioBuffer, volume: number): () => void {
  const ctx = getAudioContext();
  if (!ctx || volume <= 0) return () => {};

  // Stop any currently auditioning audio
  stopCurrentSound();

  const source = ctx.createBufferSource();
  source.buffer = buffer;

  const gainNode = ctx.createGain();
  gainNode.gain.setValueAtTime(volume, ctx.currentTime);

  source.connect(gainNode);
  gainNode.connect(ctx.destination);

  source.start(0);
  currentlyPlayingSource = source;

  source.onended = () => {
    if (currentlyPlayingSource === source) {
      currentlyPlayingSource = null;
    }
  };

  return () => {
    try {
      source.stop();
      source.disconnect();
      gainNode.disconnect();
    } catch {
      // Already stopped
    }
    if (currentlyPlayingSource === source) {
      currentlyPlayingSource = null;
    }
  };
}

/** Stop any currently testing or playing sound. */
export function stopCurrentSound(): void {
  if (currentlyPlayingSource) {
    try {
      currentlyPlayingSource.stop();
      currentlyPlayingSource.disconnect();
    } catch {
      // Ignore
    }
    currentlyPlayingSource = null;
  }
}

/**
 * Play the completion sound when a download completes.
 * Uses custom sound if uploaded, otherwise automatically falls back to default SFX.
 * Throttles simultaneous finished notifications within 300ms to avoid ear fatigue.
 */
export function playCompletionSound(force = false): () => void {
  const now = Date.now();
  if (!force && now - lastSoundPlayTime < 300) {
    return () => {};
  }
  lastSoundPlayTime = now;

  const volume = getSoundVolume();
  if (volume <= 0) return () => {};

  if (cachedCustomBuffer) {
    try {
      return playAudioBuffer(cachedCustomBuffer, volume);
    } catch {
      // Fall back to default SFX if custom buffer playback errors
      return playDefaultSfx(volume);
    }
  }

  return playDefaultSfx(volume);
}

/** Test playback of the active sound (custom if uploaded, default SFX if not). */
export function testActiveSound(): () => void {
  return playCompletionSound(true);
}

/** Explicitly test preview of the built-in default SFX. */
export function testDefaultSound(): () => void {
  stopCurrentSound();
  return playDefaultSfx(getSoundVolume());
}

/**
 * Upload and validate a custom completion sound file.
 * Decodes the audio to ensure full compatibility with the host's Web Audio engine.
 */
export async function uploadCustomSound(file: File): Promise<CustomSoundMeta> {
  if (file.size > MAX_FILE_SIZE) {
    throw new Error(
      `Sound file is too large (${(file.size / (1024 * 1024)).toFixed(1)} MB). Please choose an audio clip under 10 MB.`,
    );
  }

  const supportedTypes = [
    "audio/mpeg",
    "audio/mp3",
    "audio/wav",
    "audio/x-wav",
    "audio/ogg",
    "audio/webm",
    "audio/flac",
    "audio/x-flac",
    "audio/aac",
    "audio/m4a",
    "audio/mp4",
  ];

  const ext = file.name.slice(file.name.lastIndexOf(".")).toLowerCase();
  const validExtensions = [".mp3", ".wav", ".ogg", ".m4a", ".aac", ".flac", ".opus", ".webm"];

  const matchesMime = file.type ? supportedTypes.includes(file.type) || file.type.startsWith("audio/") : false;
  const matchesExt = validExtensions.includes(ext);

  if (!matchesMime && !matchesExt) {
    throw new Error(
      `Unsupported audio format (${ext || file.type || "unknown"}). Supported formats include MP3, WAV, OGG, M4A, FLAC, and AAC.`,
    );
  }

  const rawBytes = await file.arrayBuffer();

  const ctx = getAudioContext();
  if (!ctx) {
    throw new Error("Web Audio engine is not supported by your environment.");
  }

  let decoded: AudioBuffer;
  try {
    // decodeAudioData consumes the passed buffer, so pass a slice
    decoded = await ctx.decodeAudioData(rawBytes.slice(0));
  } catch (err) {
    throw new Error(
      `Could not decode audio file "${file.name}". Ensure the file is a valid, uncorrupted audio track.`,
    );
  }

  const meta: CustomSoundMeta = {
    name: file.name,
    size: file.size,
    type: file.type || ext.replace(".", "").toUpperCase(),
    duration: decoded.duration,
    updatedAt: Date.now(),
  };

  await storeCustomSound({
    buffer: rawBytes,
    meta,
  });

  cachedCustomBuffer = decoded;
  cachedMeta = meta;

  return meta;
}

/**
 * Remove any custom uploaded sound and revert cleanly back to default SFX.
 */
export async function removeCustomSound(): Promise<void> {
  stopCurrentSound();
  await deleteStoredCustomSound();
  cachedCustomBuffer = null;
  cachedMeta = null;
}

/** Format seconds into readable duration string (e.g. 1.2s). */
export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0.0s";
  if (seconds < 60) return `${(Math.round(seconds * 10) / 10).toFixed(1)}s`;
  const mins = Math.floor(seconds / 60);
  const secs = Math.round(seconds % 60);
  return `${mins}m ${secs}s`;
}

/** Format file size in bytes to human-readable string. */
export function formatAudioSize(bytes: number): string {
  if (!bytes || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}
