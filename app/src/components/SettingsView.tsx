import { useEffect, useState } from "react";
import { Button, ErrorStrip, Field, Section, Switch, TextInput } from "./Controls";
import { ArrowUpRight, BookOpen, Check, DownloadGlyph, RotateCcw } from "./Icons";
import { formatBytes } from "../lib/format";
import { Legend } from "./Panel";
import { SaveIndicator } from "./SaveIndicator";
import { useAutosaveSettings } from "../lib/useAutosaveSettings";
import {
  engine,
  hasEngine,
  openGuide,
  onFfmpegInstallProgress,
  onUpdateProgress,
  onUpdateReady,
  type FfmpegInstallProgress,
  type MusicTools,
  type UpdateInfo,
  type UpdateProgress,
} from "../lib/engine";
import { CompletionSoundControl } from "./CompletionSoundControl";
import { useAppearance } from "../lib/useAppearance";
import { removeCustomSound, setSoundVolume } from "../lib/completionSound";

/** A named behaviour with its lever. The lever states its own position. */
function ToggleRow({
  title,
  description,
  checked,
  onChange,
}: {
  title: string;
  description: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-6 border-b border-edge py-3 last:border-b-0">
      <div className="min-w-0">
        <p className="text-[13px] font-medium text-ink">{title}</p>
        <p className="mt-0.5 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
          {description}
        </p>
      </div>
      <Switch label={title} checked={checked} onChange={onChange} />
    </div>
  );
}

/**
 * Engine defaults, committed as you go.
 *
 * Every field writes on blur; the toggle writes on change. There is no Save
 * button, no dirty flag, and no navigation guard — see `useAutosaveSettings`
 * for why that machinery was removed rather than repaired.
 */
export function SettingsView({ settingsState }: { settingsState: ReturnType<typeof useAutosaveSettings> }) {
  const { settings, loading, error, clearError, commit, resetToDefaults, saveState } = settingsState;
  const {
    settings: appearanceSettings,
    setAnimationsEnabled,
    animationsEnabled,
    setHardwareAcceleration,
    setWebGpu,
    resetDefaults: resetAppearanceDefaults,
  } = useAppearance();

  const [bandwidth, setBandwidth] = useState("");
  const [connections, setConnections] = useState("");
  const [folder, setFolder] = useState("");

  const [isResetting, setIsResetting] = useState(false);
  const [resetFeedback, setResetFeedback] = useState<string | null>(null);

  const handleResetAllToDefault = async () => {
    if (
      !window.confirm(
        "Are you sure you want to reset all settings to factory new?\n\nThis will restore engine parameters, system behavior, timetable schedule, custom audio chimes, and interface appearance back to their original factory defaults. Your downloaded files will remain intact."
      )
    ) {
      return;
    }

    setIsResetting(true);
    setResetFeedback(null);
    try {
      // 1. Reset engine settings to factory defaults
      await resetToDefaults();

      // 2. Reset appearance and theme options
      resetAppearanceDefaults();

      // 3. Reset audio completion sound and volume
      await removeCustomSound();
      setSoundVolume(0.7);

      // 4. Reset local inputs
      setBandwidth("");
      setConnections("");
      setFolder("");

      setResetFeedback("All settings have been restored to factory new defaults.");
      setTimeout(() => setResetFeedback(null), 5000);
    } catch (e) {
      setResetFeedback(e instanceof Error ? `Reset failed: ${e.message}` : "Failed to reset settings.");
    } finally {
      setIsResetting(false);
    }
  };

  const [ffmpegTools, setFfmpegTools] = useState<MusicTools | null>(null);
  const [ffmpegLoading, setFfmpegLoading] = useState(true);
  const [installingFfmpeg, setInstallingFfmpeg] = useState(false);
  const [installProgress, setInstallProgress] = useState<FfmpegInstallProgress | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);

  useEffect(() => {
    if (!hasEngine()) {
      setFfmpegLoading(false);
      return;
    }
    engine
      .musicTools()
      .then(setFfmpegTools)
      .catch(() => {})
      .finally(() => setFfmpegLoading(false));

    return onFfmpegInstallProgress((prog) => {
      setInstallProgress(prog);
      if (prog.phase === "completed") {
        setInstallingFfmpeg(false);
        engine.musicTools().then(setFfmpegTools).catch(() => {});
      } else if (prog.phase === "error") {
        setInstallingFfmpeg(false);
        setInstallError(prog.message);
      }
    });
  }, []);

  const handleInstallFfmpeg = async () => {
    setInstallingFfmpeg(true);
    setInstallError(null);
    try {
      const tools = await engine.installFfmpeg();
      setFfmpegTools(tools);
    } catch (e) {
      setInstallError(e instanceof Error ? e.message : String(e));
    } finally {
      setInstallingFfmpeg(false);
    }
  };

  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [updateStatus, setUpdateStatus] = useState<UpdateInfo | null>(null);
  const [updateDownloading, setUpdateDownloading] = useState(false);
  const [updateDownloadInfo, setUpdateDownloadInfo] = useState<UpdateProgress | null>(null);
  const [updateReadyVer, setUpdateReadyVer] = useState<string | null>(null);
  const [updateInstallScheduled, setUpdateInstallScheduled] = useState(false);
  const [appVer, setAppVer] = useState<string>("0.1.0");
  const [updateProgressDismissed, setUpdateProgressDismissed] = useState(false);

  useEffect(() => {
    void engine.getAppVersion().then(setAppVer).catch(() => {});
  }, []);

  useEffect(() => {
    if (!hasEngine()) return;

    const applyProgress = (prog: UpdateProgress) => {
      setUpdateDownloading(prog.phase === "downloading");
      setUpdateDownloadInfo(prog);
      setUpdateProgressDismissed(false);
      if (prog.phase === "ready") {
        setUpdateReadyVer(prog.version);
      }
    };

    void engine.updateProgress().then((progress) => {
      if (progress) applyProgress(progress);
    }).catch(() => {});

    const unlistenProgress = onUpdateProgress(applyProgress);

    const unlistenReady = onUpdateReady((p) => {
      setUpdateDownloading(false);
      setUpdateReadyVer(p.version);
      setUpdateDownloadInfo({
        phase: "ready",
        percent: 100,
        downloaded: 0,
        total: 0,
        version: p.version,
        message: null,
      });
      setUpdateProgressDismissed(false);
    });

    return () => {
      unlistenProgress();
      unlistenReady();
    };
  }, []);

  const handleCheckUpdates = async () => {
    setCheckingUpdates(true);
    try {
      const res = await engine.checkForUpdates();
      setUpdateStatus(res);
    } catch (e) {
      setUpdateStatus({
        available: false,
        currentVersion: appVer,
        latestVersion: null,
        body: null,
        date: null,
        error: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setCheckingUpdates(false);
    }
  };

  const handleDownloadUpdate = async () => {
    setUpdateDownloading(true);
    try {
      await engine.downloadUpdate();
    } catch (e) {
      setUpdateDownloading(false);
      setUpdateDownloadInfo({
        phase: "error",
        percent: 0,
        downloaded: 0,
        total: 0,
        version: updateStatus?.latestVersion ?? "",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  };

  const handleInstallNextStart = async () => {
    try {
      await engine.installOnNextStart();
      setUpdateInstallScheduled(true);
    } catch (e) {
      setUpdateDownloadInfo({
        phase: "error",
        percent: 0,
        downloaded: 0,
        total: 0,
        version: updateReadyVer ?? "",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  };

  // Mirror the engine's values whenever they change underneath us (initial
  // load, or a save that came back normalized).
  useEffect(() => {
    if (!settings) return;
    setBandwidth(
      settings.bandwidthLimitBytesPerSec
        ? String(Math.round(settings.bandwidthLimitBytesPerSec / 1024))
        : "",
    );
    setConnections(settings.defaultConnections ? String(settings.defaultConnections) : "");
    setFolder(settings.defaultDirectory ?? "");
  }, [settings?.bandwidthLimitBytesPerSec, settings?.defaultConnections, settings?.defaultDirectory]);

  if (loading) {
    return (
      <div className="flex flex-col gap-7">
        <Legend className="text-ink-dim">reading settings…</Legend>
      </div>
    );
  }

  if (!settings) {
    return (
      <div className="flex flex-col gap-7">
        <Section title="Engine defaults">
          <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-12 text-center">
            <span className="h-px w-24 bg-edge" />
            <Legend className="text-ink-dim">engine detached</Legend>
            <p className="max-w-sm text-[12px] leading-relaxed text-ink-dim">
              Bandwidth, connections and folder defaults are held by the engine. Launch the desktop
              app to reach them.
            </p>
          </div>
        </Section>
      </div>
    );
  }

  const soundEnabled = settings.playSoundOnComplete ?? true;

  return (
    <div className="flex flex-col gap-7">
      {error && <ErrorStrip message={error} onDismiss={clearError} />}

      <Section
        title="Engine defaults"
        aside={<SaveIndicator state={saveState} />}
        description="Applied to every download from now on — the app, the CLI and the extension all read these values."
      >
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
          <Field
            label="bandwidth limit"
            hint="KB/s. Blank is unlimited, and the cap is shared across every concurrent download rather than applied per download."
          >
            <TextInput
              value={bandwidth}
              onChange={setBandwidth}
              onBlur={() =>
                void commit({
                  bandwidthLimitBytesPerSec: bandwidth.trim() ? Number(bandwidth) * 1024 : null,
                })
              }
              inputMode="numeric"
              placeholder="Unlimited"
              ariaLabel="Bandwidth limit in KB/s"
            />
          </Field>

          <Field label="default connections" hint="Blank uses the automatic size-based tier.">
            <TextInput
              value={connections}
              onChange={setConnections}
              onBlur={() =>
                void commit({
                  defaultConnections: connections.trim() ? Number(connections) : null,
                })
              }
              inputMode="numeric"
              placeholder="Automatic"
              ariaLabel="Default connections"
            />
          </Field>

          <Field label="default folder" hint="Where a download lands when no rule claims it.">
            <TextInput
              value={folder}
              onChange={setFolder}
              onBlur={() => void commit({ defaultDirectory: folder.trim() || null })}
              placeholder="System default"
              ariaLabel="Default folder"
            />
          </Field>
        </div>
      </Section>

      <Section title="Behaviour">
        <div>
          <ToggleRow
            title="Delete local file with trash button"
            description="The trash button removes the download entry and permanently deletes its local file after the Undo window. Turn off to keep downloaded files. Right-click always offers both choices."
            checked={settings.deleteFilesOnRemove ?? true}
            onChange={(next) => void commit({ deleteFilesOnRemove: next })}
          />
          <ToggleRow
            title="Start on launch"
            description="Launch Zoltraak silently in the system tray when you sign in to your computer (the window stays hidden)."
            checked={settings.startOnLaunch ?? false}
            onChange={(next) => void commit({ startOnLaunch: next })}
          />
          <ToggleRow
            title="Close to tray"
            description="Keep Zoltraak running in the system tray when closing the window."
            checked={settings.closeToTray ?? true}
            onChange={(next) => void commit({ closeToTray: next })}
          />
          <ToggleRow
            title="Minimize to tray"
            description="Hide Zoltraak to the system tray when the window is minimized."
            checked={settings.minimizeToTray ?? false}
            onChange={(next) => void commit({ minimizeToTray: next })}
          />
          <ToggleRow
            title="Desktop notifications"
            description="Show a system notification when a download finishes or fails."
            checked={settings.desktopNotifications ?? true}
            onChange={(next) => void commit({ desktopNotifications: next })}
          />
          <ToggleRow
            title="Completion sound"
            description="Play an audio chime when a download finishes."
            checked={soundEnabled}
            onChange={(next) => void commit({ playSoundOnComplete: next })}
          />
          <div className="mt-2 pl-0 sm:pl-1 pb-2">
            <CompletionSoundControl disabled={!soundEnabled} />
          </div>
        </div>
      </Section>

      <Section
        title="Display, Motion & Hardware Acceleration"
        description="Control interface motion, glass effects and the optional WebGPU waveform."
      >
        <div>
          <ToggleRow
            title="Enable Interface Animations"
            description="Activate mechanical relay throws, block handover sweeps on THE LINE, and snappy plate transitions. Turn off for instantaneous switching and maximum power efficiency."
            checked={animationsEnabled}
            onChange={setAnimationsEnabled}
          />
          <ToggleRow
            title="Accelerated Visual Effects"
            description="Enable glass effects and waveform rendering. This leaves the desktop webview's graphics acceleration unchanged."
            checked={appearanceSettings.hardwareAcceleration}
            onChange={setHardwareAcceleration}
          />
          <ToggleRow
            title="WebGPU Shader Graphics (WGSL)"
            description="Enable native WebGPU pipelines for high-performance oscillograph telemetry. Automatically falls back to Canvas 2D when disabled or unsupported."
            checked={appearanceSettings.webGpu}
            onChange={setWebGpu}
          />
        </div>
      </Section>

      <Section title="Media Processing & Codecs (FFmpeg)">
        <div className="flex flex-col gap-3 py-2">
          <div className="flex items-start justify-between gap-6">
            <div className="min-w-0">
              <div className="flex items-center gap-2.5">
                <p className="text-[13px] text-ink font-medium">FFmpeg Media Multiplexer</p>
                {ffmpegLoading ? (
                  <span className="legend text-[11px] text-ink-faint">checking...</span>
                ) : ffmpegTools?.ffmpeg ? (
                  <span className="inline-flex items-center gap-1.5 border border-clear/30 bg-clear/10 px-2 py-0.5 text-[11px] legend text-clear">
                    <span className="size-1.5 rounded-full bg-clear" />
                    Installed ({ffmpegTools.ffmpeg})
                  </span>
                ) : (
                  <span className="inline-flex items-center gap-1.5 border border-caution/40 bg-caution/10 px-2 py-0.5 text-[11px] legend text-caution">
                    <span className="size-1.5 rounded-full bg-caution" />
                    Not Detected
                  </span>
                )}
              </div>
              <p className="mt-1 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
                Required to merge 1080p+ YouTube and DASH split audio/video streams, embed album art & ID3 metadata in music downloads, and repackage video containers.
              </p>
            </div>

            {!ffmpegLoading && !ffmpegTools?.ffmpeg && (
              <button
                type="button"
                onClick={() => void handleInstallFfmpeg()}
                disabled={installingFfmpeg}
                className="focus-ring flex shrink-0 items-center gap-2 border border-route bg-route/15 px-3.5 py-1.5 text-[11px] legend text-route hover:bg-route/25 transition-colors disabled:opacity-50"
              >
                <DownloadGlyph className="size-3.5" />
                <span>{installingFfmpeg ? "Installing..." : "Install Automatically"}</span>
              </button>
            )}
          </div>

          {installingFfmpeg && (
            <div className="flex flex-col gap-1.5 border border-edge bg-recess/40 p-2.5 rounded-hw">
              <div className="flex items-center justify-between text-[11px] legend text-ink-dim">
                <span>{installProgress?.message || "Preparing download..."}</span>
                <span>{installProgress?.percent || 0}%</span>
              </div>
              <div className="h-1.5 w-full bg-recess rounded-full overflow-hidden">
                <div
                  className="h-full bg-route transition-all duration-300"
                  style={{ width: `${installProgress?.percent || 0}%` }}
                />
              </div>
            </div>
          )}

          {installError && (
            <ErrorStrip message={installError} onDismiss={() => setInstallError(null)} />
          )}
        </div>
      </Section>

      <Section title="Application Updates">
        <div className="flex flex-col gap-3 py-2">
          <div className="flex items-start justify-between gap-6">
            <div className="min-w-0">
              <div className="flex items-center gap-2.5">
                <p className="text-[13px] text-ink font-medium">Zoltraak Auto-Updater</p>
                {updateReadyVer ? (
                  <span className="inline-flex items-center gap-1.5 border border-route/40 bg-route/15 px-2 py-0.5 text-[11px] legend text-route">
                    <span className="size-1.5 rounded-hw-sm bg-route" />
                    Ready to install ({updateReadyVer})
                  </span>
                ) : updateStatus?.available ? (
                  <span className="inline-flex items-center gap-1.5 border border-route/40 bg-route/15 px-2 py-0.5 text-[11px] legend text-route animate-pulse">
                    <span className="size-1.5 rounded-hw-sm bg-route" />
                    New version available ({updateStatus.latestVersion})
                  </span>
                ) : updateStatus ? (
                  <span className="inline-flex items-center gap-1.5 border border-clear/30 bg-clear/10 px-2 py-0.5 text-[11px] legend text-clear">
                    <span className="size-1.5 rounded-hw-sm bg-clear" />
                    Up to date ({updateStatus.currentVersion})
                  </span>
                ) : (
                  <span className="inline-flex items-center gap-1.5 border border-edge bg-recess px-2 py-0.5 text-[11px] legend text-ink-dim">
                    Channel: Zoltraak (v{appVer})
                  </span>
                )}
              </div>
              <p className="mt-1 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
                Zoltraak checks for updates in the background, then asks before downloading. An update installs only after you approve it now or for the next application start.
              </p>
            </div>

            <div className="flex items-center gap-2 shrink-0">
              {updateReadyVer ? (
                <>
                  <button
                    type="button"
                    onClick={() => void handleInstallNextStart()}
                    disabled={updateInstallScheduled}
                    className="focus-ring flex items-center gap-2 border border-edge-strong bg-face px-3.5 py-1.5 text-[11px] legend text-ink hover:bg-recess transition-colors disabled:opacity-50"
                  >
                    <span>{updateInstallScheduled ? "Scheduled for Next Start" : "Install Next Start"}</span>
                  </button>
                  <button
                    type="button"
                    onClick={() => void engine.installAndRestart()}
                    className="focus-ring flex items-center gap-2 border border-route bg-route px-3.5 py-1.5 text-[11px] legend text-paper hover:bg-route-hover transition-colors"
                  >
                    <RotateCcw className="size-3.5" />
                    <span>Install Now</span>
                  </button>
                </>
              ) : updateStatus?.available ? (
                <button
                  type="button"
                  onClick={() => void handleDownloadUpdate()}
                  disabled={updateDownloading}
                  className="focus-ring flex items-center gap-2 border border-route bg-route px-3.5 py-1.5 text-[11px] legend text-paper hover:bg-route-hover transition-colors disabled:opacity-50"
                >
                  <DownloadGlyph className="size-3.5" />
                  <span>{updateDownloading ? "Downloading..." : "Download Update"}</span>
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => void handleCheckUpdates()}
                  disabled={checkingUpdates || updateDownloading}
                  className="focus-ring flex items-center gap-2 border border-edge-strong bg-face px-3.5 py-1.5 text-[11px] legend text-ink hover:bg-recess transition-colors disabled:opacity-50"
                >
                  <RotateCcw className={`size-3.5 ${checkingUpdates ? "animate-spin" : ""}`} />
                  <span>{checkingUpdates ? "Checking..." : updateDownloading ? "Downloading..." : "Check for Updates"}</span>
                </button>
              )}
            </div>
          </div>

          {updateDownloadInfo && !updateProgressDismissed && (() => {
            const phase = updateDownloadInfo.phase;
            const pct = updateDownloadInfo.percent ?? 0;
            const isDownloading = phase === "downloading";
            const isReady = phase === "ready";
            const isError = phase === "error";

            // Indeterminate until the engine starts reporting real progress
            const indeterminate = isDownloading && pct === 0;

            const meterColor = isError
              ? "text-danger"
              : isReady
                ? "text-clear"
                : "text-route";

            const labelColor = isError
              ? "text-danger"
              : isReady
                ? "text-clear"
                : "text-ink-dim";

            const borderColor = isError
              ? "border-danger/30"
              : isReady
                ? "border-clear/30"
                : "border-edge";

            const sizeLabel =
              isDownloading && updateDownloadInfo.total > 0
                ? `${formatBytes(updateDownloadInfo.downloaded)} / ${formatBytes(updateDownloadInfo.total)}`
                : null;

            const statusLabel = isError
              ? `Update download failed${updateDownloadInfo.message ? ` — ${updateDownloadInfo.message}` : ""}`
              : isReady
                ? `Ready to install (${updateDownloadInfo.version})`
                : `Downloading update${updateDownloadInfo.version ? ` (${updateDownloadInfo.version})` : ""}…`;

            return (
              <div className={`flex flex-col gap-1.5 border ${borderColor} bg-recess/40 p-2.5 rounded-hw`}>
                <div className={`flex items-center justify-between text-[11px] legend ${labelColor}`}>
                  <span>{statusLabel}</span>
                  <span className="flex items-center gap-3">
                    {sizeLabel && (
                      <span className="describer text-[11px] text-ink-faint">{sizeLabel}</span>
                    )}
                    {!indeterminate && <span>{pct}%</span>}
                    {(isReady || isError) && (
                      <button
                        type="button"
                        onClick={() => setUpdateProgressDismissed(true)}
                        className="focus-ring text-[11px] legend text-ink-faint hover:text-ink transition-colors"
                      >
                        Dismiss
                      </button>
                    )}
                  </span>
                </div>
                <progress
                  className={`update-progress ${meterColor}`}
                  max={100}
                  value={indeterminate ? undefined : pct}
                  aria-label={statusLabel}
                />
              </div>
            );
          })()}

          {updateStatus?.error && (
            <p className="text-[12px] text-ink-faint">
              Notice: {updateStatus.error}
            </p>
          )}
        </div>
      </Section>

      <Section title="Handbook & Documentation">
        <div className="flex items-center justify-between gap-6 py-2">
          <div className="min-w-0">
            <p className="text-[13px] text-ink">User Guide & Operation Manual</p>
            <p className="mt-0.5 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
              Explore detailed guides for segmented downloading, video extraction, gallery harvesting, routing rules, browser extensions, and keyboard shortcuts.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void openGuide()}
            className="focus-ring flex shrink-0 items-center gap-2 border border-edge-strong bg-face px-3.5 py-1.5 text-[11px] legend text-ink hover:bg-recess transition-colors"
          >
            <BookOpen className="size-3.5" />
            <span>Open Guide</span>
            <ArrowUpRight className="size-3 text-ink-faint" />
          </button>
        </div>
      </Section>

      <Section
        id="section-factory-reset"
        title="Factory Reset"
        description="Revert all application parameters — engine defaults, system behavior, scheduler timetable, sound alerts, and appearance themes — back to original factory new state."
      >
        <div className="flex flex-col gap-3">
          {resetFeedback && (
            <div className="flex items-center gap-2 border border-clear/40 bg-clear/10 px-3 py-2 text-[12px] text-clear">
              <Check className="size-3.5 shrink-0" />
              <span>{resetFeedback}</span>
            </div>
          )}
          <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4 border border-edge bg-face/50 p-4">
            <div className="min-w-0">
              <p className="text-[13px] font-medium text-ink">Reset All Settings to Factory New</p>
              <p className="mt-0.5 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
                Restores bandwidth, connections, folders, tray behavior, schedule timetables, audio alerts, and interface appearance to factory new defaults. (Your downloads are never removed.)
              </p>
            </div>
            <Button
              tone="danger"
              disabled={isResetting}
              onClick={() => void handleResetAllToDefault()}
            >
              <span className="flex items-center gap-1.5">
                <RotateCcw className={`size-3.5 ${isResetting ? "animate-spin" : ""}`} />
                <span>{isResetting ? "Resetting..." : "Reset to Default"}</span>
              </span>
            </Button>
          </div>
        </div>
      </Section>
    </div>
  );
}
