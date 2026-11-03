import { useEffect, useRef, useState } from "react";
import { ClearButton, Button, ErrorStrip, Section } from "./Controls";
import { Check, Discord, DownloadGlyph, Film, Frame, FolderOpen } from "./Icons";
import { Legend } from "./Panel";
import type { ToastItem } from "./Toast";
import { TooltipGroup } from "./Tooltip";
import { engine, hasEngine, openUrl, type ImageAlbum, type ImageItem } from "../lib/engine";
import { formatBytes } from "../lib/format";
import { itemFilename, useImages } from "../lib/useImages";
import { albumStatus } from "../lib/albumStatus";
import { useFocusedDismiss } from "../lib/useFocusedDismiss";
import { playsWithAudio } from "../lib/useVideo";

/**
 * The Images destination: a post URL in, the pictures it holds out.
 *
 * Deliberately not a second Downloads list. Every picture the user keeps is
 * handed to the ordinary queue and appears in the register with pause, resume
 * and reveal already working — so what this screen owns is only the question
 * the queue has no opinion about: which of a post's pictures are wanted. Once
 * that is answered the screen has nothing left to say, and says nothing.
 */

/** `2048 × 1536 · 1.2 MB · JPG`, minus whatever the engine could not establish. */
function describe(item: ImageItem): string {
  const parts: string[] = [];
  if (item.width && item.height) parts.push(`${item.width} × ${item.height}`);
  if (item.filesize) parts.push(formatBytes(item.filesize));
  parts.push(item.extension.toUpperCase());
  return parts.join(" · ");
}

/**
 * What a clip has to admit about its sound, if anything.
 *
 * The same rule the Video view applies to a format row, for the same reason: a
 * rendition the engine *probed* and found silent is still worth offering — on
 * Reddit's `fallback_url` it may be all there is — but never without saying so.
 * Silence is only reported when it was established; an unprobed clip says
 * nothing rather than guessing.
 */
function audioWarning(item: ImageItem): string | null {
  if (item.kind !== "video") return null;
  const format = item.formats?.[0];
  if (!format || playsWithAudio(format)) return null;
  return "no audio";
}

/**
 * One picture in the sheet.
 *
 * The whole tile is the checkbox. A grid of pictures with a small square in
 * the corner of each is a grid you have to aim at; the picture is the target,
 * and a selected one is plated in route blue rather than merely ticked.
 */
function Tile({
  item,
  index,
  album,
  selected,
  onToggle,
}: {
  item: ImageItem;
  index: number;
  album: ImageAlbum;
  selected: boolean;
  onToggle: () => void;
}) {
  const [broken, setBroken] = useState(false);
  // A clip's own URL is not renderable in an <img>, so a video tile with no
  // poster shows the placeholder rather than a permanently broken image.
  const preview = item.thumbnail ?? (item.kind === "video" ? null : item.url);
  const warning = audioWarning(item);

  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={selected}
      aria-label={`${itemFilename(album, item, index)} — ${describe(item)}`}
      onClick={onToggle}
      className={`focus-ring group relative flex flex-col overflow-hidden rounded-hw border text-left transition-colors ${
        selected ? "border-route bg-face" : "border-edge bg-recess hover:border-edge-strong"
      }`}
    >
      <span className="relative block aspect-square w-full overflow-hidden bg-block-clear">
        {broken || !preview ? (
          <span className="flex size-full items-center justify-center text-ink-faint">
            {item.kind === "video" ? <Film className="size-6" /> : <Frame className="size-6" />}
          </span>
        ) : (
          <img
            src={preview}
            alt=""
            loading="lazy"
            onError={() => setBroken(true)}
            className="size-full object-cover"
          />
        )}

        {/* The index is how a carousel's order survives into the filenames,
            so it is worth showing here rather than only in the saved name. */}
        <span className="legend absolute left-1 top-1 rounded-hw-sm bg-panel/80 px-1 py-px text-[11px] text-ink-dim">
          {index + 1}
        </span>

        {item.kind === "video" && (
          <span className="legend absolute right-1 top-1 flex items-center gap-1 rounded-hw-sm bg-panel/80 px-1 py-px text-[11px] text-ink-dim">
            <Film className="size-3" />
            clip
          </span>
        )}

        <span
          aria-hidden="true"
          className={`absolute bottom-1 right-1 flex size-[18px] items-center justify-center rounded-hw-sm border transition-colors ${
            selected
              ? "border-route bg-route text-on-signal"
              : "border-edge-strong bg-panel/80 text-transparent"
          }`}
        >
          <Check className="size-3" />
        </span>
      </span>

      <span className="describer flex items-baseline gap-1.5 border-t border-edge px-2 py-1.5 text-[11px] text-ink-dim">
        <span className="min-w-0 flex-1 truncate">{describe(item)}</span>
        {warning && <span className="shrink-0 text-caution">{warning}</span>}
      </span>
    </button>
  );
}

export function ImagesView({
  prefill,
  onAddToast,
  onNavigateToDownloads,
}: {
  /** A URL (and its captured session) the extension asked us to open here. */
  prefill?: { url: string; headers: [string, string][]; nonce: number } | null;
  onAddToast?: (toast: ToastItem) => void;
  onNavigateToDownloads?: () => void;
}) {
  const images = useImages();
  const [url, setUrl] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  useFocusedDismiss(saved, () => setSaved(null));
  const inputRef = useRef<HTMLInputElement>(null);

  // Headers only apply to the URL they were captured for; a user who then
  // types a different link must not inherit the previous page's session.
  const prefillHeaders = useRef<{ url: string; headers: [string, string][] } | null>(null);

  useEffect(() => {
    if (!prefill?.url) return;
    setUrl(prefill.url);
    prefillHeaders.current = { url: prefill.url, headers: prefill.headers };
    void images.analyze(prefill.url, prefill.headers.length ? prefill.headers : undefined);
    // Re-runs when the extension sends the same URL again, which is what the
    // nonce is for.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prefill?.nonce]);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = url.trim();
    if (!trimmed || images.analyzing) return;
    setSaved(null);
    const headers =
      prefillHeaders.current?.url === trimmed ? prefillHeaders.current.headers : undefined;
    await images.analyze(trimmed, headers?.length ? headers : undefined);
  }

  const [discordSaving, setDiscordSaving] = useState(false);
  const [discordSuccess, setDiscordSuccess] = useState(false);

  async function save(autoDiscord = false) {
    if (autoDiscord) {
      setDiscordSaving(true);
      setDiscordSuccess(false);
    }
    const result = await images.save(undefined, autoDiscord);
    if (autoDiscord) {
      setDiscordSaving(false);
    }
    if (result.saved === 0 && result.failed === 0) return;

    if (autoDiscord) {
      setDiscordSuccess(true);
      setTimeout(() => setDiscordSuccess(false), 2000);
    }

    const discordSuffix = autoDiscord ? " · auto-sending to Discord" : "";
    setSaved(
      result.failed > 0
        ? `Sent ${result.saved} to the queue · ${result.failed} rejected${discordSuffix}`
        : `Sent ${result.saved} ${result.saved === 1 ? "file" : "files"} to the queue${discordSuffix}`,
    );

    if (autoDiscord) {
      if (hasEngine()) {
        try {
          const status = await engine.discordStatus();
          if (!status.connected) {
            onAddToast?.({
              id: `discord-warn-${Date.now()}`,
              type: "error",
              title: "Discord not connected",
              message:
                "Images sent to download queue, but Discord delivery is not configured yet. Connect it in the Discord tab.",
            });
            return;
          }
        } catch {}
      }
      onAddToast?.({
        id: `discord-auto-${Date.now()}`,
        type: "info",
        title: "Queued with Discord auto-delivery",
        message: `${result.saved} ${result.saved === 1 ? "file" : "files"} will be automatically sent to Discord once downloaded.`,
      });
    }

    if (result.saved > 0 && onNavigateToDownloads) {
      setUrl("");
      images.clear();
      setSaved(null);
      prefillHeaders.current = null;
      onNavigateToDownloads();
    }
  }

  const album = images.album;
  const selectedCount = images.selectedItems.length;
  const allSelected = album ? selectedCount === album.items.length : false;

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-7 px-6 py-6">
      <Section
        title="Extract from a post"
        aside={url.length > 0 ? <ClearButton disabled={images.analyzing || images.saving} onClick={() => {
          setUrl("");
          images.clear();
          setSaved(null);
          prefillHeaders.current = null;
          inputRef.current?.focus();
        }} /> : undefined}
        description="Extracts all media from a post — videos, video clips, photos, carousels, and animations — from X, Instagram, Facebook, Reddit, and TikTok, plus any page that describes its own media or direct links. Every media item attached to the post is detected and extracted together. A private post needs the site's sign-in; share it from the extension first. Whatever you keep lands in the register with the rest of your downloads."
      >
        <form autoComplete="off" onSubmit={(e) => void submit(e)} className="flex items-end gap-2">
          <input
            ref={inputRef}
            value={url}
            onChange={(e) => {
              setUrl(e.target.value);
              if (!e.target.value.trim()) {
                images.clear();
                setSaved(null);
                prefillHeaders.current = null;
              }
            }}
            placeholder="https://x.com/…/status/…  or a direct media link (.jpg, .png, .mp4, …)"
            aria-label="Post or media URL" autoComplete="off"
            spellCheck={false}
            className="recess focus-ring min-w-0 flex-1 rounded-hw px-2.5 py-1.5 font-mono text-[12px] text-ink outline-none placeholder:text-ink-faint"
          />
          <Button type="submit" disabled={images.analyzing}>
            {images.analyzing ? "extracting…" : "extract"}
          </Button>
        </form>

        {images.error && <ErrorStrip message={images.error} onDismiss={images.clearError} />}
      </Section>

      {album && (
        <Section
          title={albumStatus(album)}
          aside={
            <Legend>
              {selectedCount} of {album.items.length} selected
            </Legend>
          }
        >
          {!!album.warnings?.length && (
            <div role="status" className="describer space-y-1 text-[12px] text-caution">
              {Array.from(new Set(album.warnings)).map((warning) => <p key={warning}>{warning}</p>)}
              <p>Available media can still be downloaded.</p>
              {album.completeness !== "complete" && /^https:\/\/(?:[a-z0-9-]+\.)*facebook\.com\//i.test(album.sourceUrl) && (
                <Button onClick={() => void openUrl(album.sourceUrl)}>Open Facebook post</Button>
              )}
            </div>
          )}
          <div className="border border-edge bg-face">
            <div className="flex items-center gap-3 border-b border-edge bg-recess px-4 py-2.5">
              <div className="plate flex size-11 shrink-0 items-center justify-center rounded-hw text-ink-faint">
                <Frame className="size-4" />
              </div>
              <div className="min-w-0 flex-1">
                <p className="truncate text-[13px] text-ink">{album.title}</p>
                <p data-selectable className="describer mt-0.5 truncate text-[11px] text-ink-dim">
                  {album.uploader ? `${album.uploader} · ` : ""}
                  {album.sourceUrl}
                </p>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2 border-b border-edge px-4 py-2.5">
              <button
                type="button"
                onClick={allSelected ? images.selectNone : images.selectAll}
                className="focus-ring legend rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-face hover:text-ink"
              >
                {allSelected ? "select none" : "select all"}
              </button>

              <span className="flex-1" />

              {saved && (
                <span className="describer text-[11px] text-clear">{saved}</span>
              )}

              {hasEngine() && (
                <TooltipGroup side="bottom">
                  <button
                    type="button"
                    onClick={() => void engine.revealFolder()}
                    data-tooltip="Open download folder (Ctrl + E)"
                    className="t-tt-trigger focus-ring legend flex items-center gap-1.5 rounded-hw border border-edge-strong px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-face hover:text-ink"
                  >
                    <FolderOpen className="size-3.5" />
                    Open folder
                  </button>
                </TooltipGroup>
              )}

              <Button
                onClick={() => void save(false)}
                disabled={selectedCount === 0 || images.saving || discordSaving}
              >
                <span className="flex items-center gap-1.5">
                  <DownloadGlyph className="size-3.5" />
                  {images.saving && !discordSaving
                    ? "sending…"
                    : selectedCount === album.items.length
                      ? `download all (${selectedCount})`
                      : `download ${selectedCount}`}
                </span>
              </Button>

              <TooltipGroup side="top">
                <button
                  type="button"
                  onClick={() => void save(true)}
                  disabled={selectedCount === 0 || images.saving || discordSaving}
                  data-tooltip={
                    selectedCount === album.items.length
                      ? `Download all (${selectedCount}) and auto-send to Discord`
                      : `Download ${selectedCount} and auto-send to Discord`
                  }
                  aria-label="Download and auto-send to Discord"
                  className={`t-tt-trigger focus-ring flex h-[31px] w-[31px] shrink-0 items-center justify-center rounded-hw border transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-45 ${
                    discordSuccess
                      ? "border-route bg-route/15 text-route"
                      : "border-edge-strong bg-face text-ink-dim hover:border-discord hover:bg-discord/10 hover:text-discord"
                  }`}
                >
                  {discordSuccess ? <Check className="size-3.5" /> : <Discord className="size-4" />}
                </button>
              </TooltipGroup>
            </div>

            <div
              role="group"
              aria-label="Extracted media"
              className="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-2.5 p-4"
            >
              {album.items.map((item, index) => (
                <Tile
                  key={item.id}
                  item={item}
                  index={index}
                  album={album}
                  selected={images.selected.has(item.id)}
                  onToggle={() => images.toggle(item.id)}
                />
              ))}
            </div>
          </div>
        </Section>
      )}
    </div>
  );
}
