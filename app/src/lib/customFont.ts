/**
 * A typeface the user uploaded for the interface.
 *
 * The file itself lives in IndexedDB (it can be megabytes, which does not belong
 * in the settings JSON); the settings keep only its name and options. The
 * browser parses the font, so every file is validated by loading it before it
 * is kept, and nothing here ever runs the file's content as anything but a font.
 */

export const CUSTOM_FONT_FAMILY = "ZoltraakCustomUI";
export const MAX_FONT_BYTES = 8 * 1024 * 1024;
export const FONT_ACCEPT = ".woff2,.woff,.ttf,.otf";

/** What is kept in the appearance settings about the uploaded font. */
export type CustomFontMeta = {
  name: string;
  fileName: string;
  bytes: number;
  /** Also use it for figures and code, not just text. */
  applyToMono: boolean;
};

const SIGNATURES: Record<string, string> = {
  wOF2: "WOFF2",
  wOFF: "WOFF",
  OTTO: "OpenType",
  true: "TrueType",
  ttcf: "TrueType collection",
};

function signatureOf(header: Uint8Array): string | null {
  if (header.length < 4) return null;
  if (header[0] === 0 && header[1] === 1 && header[2] === 0 && header[3] === 0) return "TrueType";
  const tag = String.fromCharCode(header[0] ?? 0, header[1] ?? 0, header[2] ?? 0, header[3] ?? 0);
  return SIGNATURES[tag] ?? null;
}

/** A readable family name from a file name: "Fira-Sans_Medium.woff2" becomes "Fira Sans Medium". */
export function fontDisplayName(fileName: string): string {
  const stem = fileName.replace(/^.*[\\/]/, "").replace(/\.[^.]+$/, "");
  const name = stem.replace(/[-_]+/g, " ").replace(/\s+/g, " ").trim();
  return (name || "Custom typeface").slice(0, 48);
}

/** Why a file cannot be used, or null when it looks like a font worth loading. */
export function checkFontFile(fileName: string, bytes: number, header: Uint8Array): string | null {
  if (!/\.(woff2|woff|ttf|otf)$/i.test(fileName)) return "Choose a .woff2, .woff, .ttf or .otf font file.";
  if (bytes <= 0) return "That file is empty.";
  if (bytes > MAX_FONT_BYTES) return `Fonts can be up to ${MAX_FONT_BYTES / 1024 / 1024} MB; this one is ${(bytes / 1024 / 1024).toFixed(1)} MB.`;
  if (!signatureOf(header)) return "That file does not look like a font.";
  return null;
}

/** Settings read back from storage, restricted to what the app understands. */
export function normalizeCustomFont(raw: unknown): CustomFontMeta | null {
  if (!raw || typeof raw !== "object") return null;
  const value = raw as Partial<CustomFontMeta>;
  if (typeof value.name !== "string" || !value.name.trim()) return null;
  return {
    name: value.name.trim().slice(0, 48),
    fileName: typeof value.fileName === "string" ? value.fileName.slice(0, 120) : "",
    bytes: typeof value.bytes === "number" && Number.isFinite(value.bytes) && value.bytes > 0 ? Math.round(value.bytes) : 0,
    applyToMono: value.applyToMono === true,
  };
}

// ----------------------------------------------------------------- storage

const DB_NAME = "zoltraak-fonts";
const STORE = "fonts";
const KEY = "ui-font";

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    if (typeof indexedDB === "undefined") {
      reject(new Error("This environment cannot store font files."));
      return;
    }
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("Could not open font storage."));
  });
}

async function withStore<T>(mode: IDBTransactionMode, run: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await openDb();
  try {
    return await new Promise<T>((resolve, reject) => {
      const request = run(db.transaction(STORE, mode).objectStore(STORE));
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error("Font storage failed."));
    });
  } finally {
    db.close();
  }
}

export const saveCustomFontData = (data: ArrayBuffer) => withStore("readwrite", (store) => store.put(data, KEY)).then(() => undefined);
export const readCustomFontData = () => withStore<ArrayBuffer | undefined>("readonly", (store) => store.get(KEY)).then((data) => data ?? null);
export const deleteCustomFontData = () => withStore("readwrite", (store) => store.delete(KEY)).then(() => undefined);

// ----------------------------------------------------------------- loading

let activeFace: FontFace | null = null;
let activeKey = "";
let activeToken = 0;

/** The stored file changed: the next apply must read it again rather than reuse the loaded face. */
export function invalidateCustomFont() {
  activeKey = "";
}

export function customFontStack(fallback: string): string {
  return `"${CUSTOM_FONT_FAMILY}", ${fallback}`;
}

function release(root: HTMLElement) {
  if (activeFace) document.fonts.delete(activeFace);
  activeFace = null;
  activeKey = "";
  root.style.removeProperty("--ui-font-sans");
  root.style.removeProperty("--ui-font-mono");
}

/** Parse font bytes with the browser; resolves to the loaded face or throws. */
export async function loadFontFace(data: ArrayBuffer, family: string = CUSTOM_FONT_FAMILY): Promise<FontFace> {
  const face = new FontFace(family, data);
  await face.load();
  return face;
}

/**
 * Make the stored font the interface typeface, or put the presets back.
 * Safe to call on every settings change: a newer call supersedes an older one.
 */
export async function applyCustomFont(
  root: HTMLElement,
  meta: CustomFontMeta | null,
  fallbacks: { sans: string; mono: string },
): Promise<boolean> {
  const token = ++activeToken;
  if (!meta) {
    release(root);
    return false;
  }
  const key = `${meta.fileName}:${meta.bytes}`;
  if (activeFace && key === activeKey) {
    // Already loaded: only the font-mono option may have changed.
    root.style.setProperty("--ui-font-sans", customFontStack(fallbacks.sans));
    if (meta.applyToMono) root.style.setProperty("--ui-font-mono", customFontStack(fallbacks.mono));
    else root.style.removeProperty("--ui-font-mono");
    return true;
  }
  try {
    const data = await readCustomFontData();
    if (token !== activeToken) return false;
    if (!data) {
      release(root);
      return false;
    }
    const face = await loadFontFace(data);
    if (token !== activeToken) return false;
    release(root);
    document.fonts.add(face);
    activeFace = face;
    activeKey = key;
    root.style.setProperty("--ui-font-sans", customFontStack(fallbacks.sans));
    if (meta.applyToMono) root.style.setProperty("--ui-font-mono", customFontStack(fallbacks.mono));
    return true;
  } catch {
    if (token === activeToken) release(root);
    return false;
  }
}
