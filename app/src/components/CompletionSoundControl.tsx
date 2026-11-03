import { useCallback, useEffect, useRef, useState } from "react";
import {
  type CustomSoundMeta,
  formatAudioSize,
  formatDuration,
  getCustomSoundMeta,
  getSoundVolume,
  initCustomSound,
  playDefaultSfx,
  removeCustomSound,
  setSoundVolume,
  stopCurrentSound,
  testActiveSound,
  uploadCustomSound,
} from "../lib/completionSound";
import { Button, ErrorStrip } from "./Controls";
import { Check, Pause, Play, RotateCcw, Trash, Upload, Volume2, VolumeX } from "./Icons";
import { Legend } from "./Panel";

export function CompletionSoundControl({ disabled }: { disabled?: boolean }) {
  const [meta, setMeta] = useState<CustomSoundMeta | null>(null);
  const [volume, setVolumeState] = useState<number>(0.7);
  const [isPlaying, setIsPlaying] = useState<boolean>(false);
  const [isPlayingDefault, setIsPlayingDefault] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [isDragging, setIsDragging] = useState<boolean>(false);
  const [uploading, setUploading] = useState<boolean>(false);

  const fileInputRef = useRef<HTMLInputElement>(null);
  const stopFnRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    setVolumeState(getSoundVolume());
    void initCustomSound().then((stored) => {
      setMeta(stored ?? getCustomSoundMeta());
    });

    return () => {
      stopCurrentSound();
      stopFnRef.current?.();
    };
  }, []);

  const stopAnyPlayback = useCallback(() => {
    stopCurrentSound();
    if (stopFnRef.current) {
      stopFnRef.current();
      stopFnRef.current = null;
    }
    setIsPlaying(false);
    setIsPlayingDefault(false);
  }, []);

  const handleTestActive = useCallback(() => {
    if (isPlaying) {
      stopAnyPlayback();
      return;
    }
    stopAnyPlayback();
    setIsPlaying(true);

    const stop = testActiveSound();
    stopFnRef.current = stop;

    // Estimate duration for reset
    const durationMs = meta ? Math.min(meta.duration * 1000 + 100, 15000) : 900;
    setTimeout(() => {
      setIsPlaying(false);
    }, durationMs);
  }, [isPlaying, meta, stopAnyPlayback]);

  const handleTestDefault = useCallback(() => {
    if (isPlayingDefault) {
      stopAnyPlayback();
      return;
    }
    stopAnyPlayback();
    setIsPlayingDefault(true);

    const stop = playDefaultSfx(volume);
    stopFnRef.current = stop;

    setTimeout(() => {
      setIsPlayingDefault(false);
    }, 900);
  }, [isPlayingDefault, stopAnyPlayback, volume]);

  const handleVolumeChange = useCallback((nextVol: number) => {
    const clamped = Math.max(0, Math.min(1, nextVol));
    setVolumeState(clamped);
    setSoundVolume(clamped);
  }, []);

  const processFile = useCallback(
    async (file: File) => {
      setError(null);
      setUploading(true);
      stopAnyPlayback();

      try {
        const uploadedMeta = await uploadCustomSound(file);
        setMeta(uploadedMeta);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setUploading(false);
        if (fileInputRef.current) {
          fileInputRef.current.value = "";
        }
      }
    },
    [stopAnyPlayback],
  );

  const handleFileChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (file) void processFile(file);
  };

  const handleRemove = useCallback(async () => {
    stopAnyPlayback();
    await removeCustomSound();
    setMeta(null);
  }, [stopAnyPlayback]);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      setIsDragging(false);
      if (disabled) return;
      const file = e.dataTransfer.files?.[0];
      if (file) void processFile(file);
    },
    [disabled, processFile],
  );

  return (
    <div
      className={`flex flex-col gap-3 rounded-hw border border-edge bg-face/50 p-4 transition-opacity ${
        disabled ? "pointer-events-none opacity-45" : ""
      }`}
    >
      <input
        ref={fileInputRef}
        type="file"
        accept="audio/*,.mp3,.wav,.ogg,.m4a,.aac,.flac,.opus,.webm"
        className="hidden"
        onChange={handleFileChange}
      />

      {error && <ErrorStrip message={error} onDismiss={() => setError(null)} />}

      {/* Active Sound Status Card & Drag-Drop Zone */}
      <div
        onDragOver={(e) => {
          e.preventDefault();
          if (!disabled) setIsDragging(true);
        }}
        onDragLeave={() => setIsDragging(false)}
        onDrop={handleDrop}
        className={`flex flex-col gap-3 border px-3.5 py-3 transition-colors ${
          isDragging
            ? "border-route bg-route/10"
            : meta
              ? "border-edge-strong bg-recess/60"
              : "border-dashed border-edge-strong/80 bg-recess/30"
        }`}
      >
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex min-w-0 items-center gap-3">
            <span
              className={`flex size-7 shrink-0 items-center justify-center rounded-hw border ${
                meta
                  ? "border-route/50 bg-route/15 text-route"
                  : "border-edge-strong bg-face text-ink-dim"
              }`}
            >
              {meta ? <Check className="size-3.5" /> : <Volume2 className="size-3.5" />}
            </span>
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <p className="truncate text-[13px] font-medium text-ink">
                  {meta ? meta.name : "Default SFX"}
                </p>
                <span className="legend shrink-0 rounded-hw border border-edge-strong px-1.5 py-0.5 text-[11px] uppercase tracking-wider text-ink-faint">
                  {meta ? meta.type : "Synthetic"}
                </span>
              </div>
              <p className="mt-0.5 text-[11px] text-ink-dim">
                {meta
                  ? `${formatAudioSize(meta.size)} • ${formatDuration(meta.duration)} duration`
                  : "Harmonic 3-tone chime (523Hz → 659Hz → 784Hz)"}
              </p>
            </div>
          </div>

          {/* Action Buttons */}
          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              tone="ghost"
              onClick={handleTestActive}
              disabled={disabled}
              className="flex items-center gap-1.5 !px-3 !py-1 text-[11px]"
            >
              {isPlaying ? (
                <>
                  <Pause className="size-3 text-clear" />
                  <span>Stop</span>
                </>
              ) : (
                <>
                  <Play className="size-3 text-ink-dim" />
                  <span>Test sound</span>
                </>
              )}
            </Button>

            {meta && (
              <Button
                type="button"
                tone="ghost"
                onClick={handleTestDefault}
                disabled={disabled}
                className="flex items-center gap-1.5 !px-3 !py-1 text-[11px] text-ink-faint hover:text-ink"
              >
                {isPlayingDefault ? (
                  <>
                    <Pause className="size-3 text-clear" />
                    <span>Stop default</span>
                  </>
                ) : (
                  <>
                    <RotateCcw className="size-3 text-ink-faint" />
                    <span>Preview default</span>
                  </>
                )}
              </Button>
            )}

            <Button
              type="button"
              tone={meta ? "ghost" : "primary"}
              onClick={() => fileInputRef.current?.click()}
              disabled={disabled || uploading}
              className="flex items-center gap-1.5 !px-3 !py-1 text-[11px]"
            >
              <Upload className="size-3" />
              <span>{meta ? "Replace" : "Upload sound"}</span>
            </Button>

            {meta && (
              <Button
                type="button"
                tone="danger"
                onClick={() => void handleRemove()}
                disabled={disabled}
                className="flex items-center gap-1.5 !px-2.5 !py-1 text-[11px]"
                aria-label="Remove custom sound and revert to default"
              >
                <Trash className="size-3" />
                <span>Remove</span>
              </Button>
            )}
          </div>
        </div>
      </div>

      {/* Volume & Details Bar */}
      <div className="flex flex-wrap items-center justify-between gap-4 pt-1">
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={() => handleVolumeChange(volume === 0 ? 0.7 : 0)}
            className="focus-ring text-ink-dim transition-colors hover:text-ink"
            aria-label={volume === 0 ? "Unmute completion sound" : "Mute completion sound"}
          >
            {volume === 0 ? <VolumeX className="size-4" /> : <Volume2 className="size-4" />}
          </button>
          <div className="flex items-center gap-2">
            <Legend className="text-[11px] text-ink-faint">volume</Legend>
            <input
              type="range"
              min="0"
              max="1"
              step="0.05"
              value={volume}
              onChange={(e) => handleVolumeChange(parseFloat(e.target.value))}
              className="h-1.5 w-24 cursor-pointer accent-route"
              aria-label="Completion sound volume"
            />
            <span className="min-w-[2.5rem] font-mono text-[11px] text-ink-dim">
              {Math.round(volume * 100)}%
            </span>
          </div>
        </div>

        <p className="text-[11px] text-ink-faint">
          {meta
            ? "Custom sound active. Reverting removes the file and restores the default chime."
            : "No file uploaded. Zoltraak uses the default audio chime."}
        </p>
      </div>
    </div>
  );
}
