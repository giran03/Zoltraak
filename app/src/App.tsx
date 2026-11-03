import { ReleaseNotesDialog } from "./components/ReleaseNotesDialog";
import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { DownloadsView, type QueueActions } from "./components/DownloadsView";
import { PanelHeader, type Section } from "./components/PanelHeader";
import type { SettingsTab } from "./components/SettingsShell";
import { ToastContainer } from "./components/Toast";
import { VisitedPanel } from "./components/VisitedPanel";
import { engine, hasEngine, onAppNavigate, onDownloadPrefill, onImagePrefill, onMusicPrefill, onUpdateAvailable, onUpdateReady, onVideoPrefill, type Container } from "./lib/engine";
import { mergeItems, musicJobToSnapshot, videoJobToSnapshot, type QueueItem } from "./lib/snapshots";
import { useQueue } from "./lib/useQueue";
import { useSettings } from "./lib/useSettings";
import { addLinks, type RecoveryKind } from "./lib/downloadUx";
import { useDownloadNotifications } from "./lib/useDownloadNotifications";
import { useSoftRemove } from "./lib/useSoftRemove";
import { useToasts } from "./lib/useToasts";
import { useAppearance } from "./lib/useAppearance";
import { groupJobs, useMusic } from "./lib/useMusic";
import { useConvert } from "./lib/useConvert";
import { useVideo } from "./lib/useVideo";
import { parseUrls } from "./lib/urls";
import { classifyLink, routeMessage } from "./lib/linkKind";
import { useExtensionUpdates } from "./lib/useExtensionUpdates";
import { useShortcuts } from "./lib/useShortcuts";
import { isTypingTarget, matchesShortcut, shortcutsBlocked } from "./lib/shortcuts";

const CategoriesView = lazy(() => import("./components/CategoriesView").then(m => ({ default: m.CategoriesView })));
const DiscordSettings = lazy(() => import("./components/DiscordSettings").then(m => ({ default: m.DiscordSettings })));
const ImagesView = lazy(() => import("./components/ImagesView").then(m => ({ default: m.ImagesView })));
const MusicView = lazy(() => import("./components/MusicView").then(m => ({ default: m.MusicView })));
const ConvertView = lazy(() => import("./components/ConvertView").then(m => ({ default: m.ConvertView })));
const SettingsShell = lazy(() => import("./components/SettingsShell").then(m => ({ default: m.SettingsShell })));
// Dialogs opened on demand stay out of the startup bundle. Each mounts the
// first time it opens and then stays mounted, so its close transition and
// state behave exactly as before.
const DiscordSendDialog = lazy(() => import("./components/DiscordSendDialog").then(m => ({ default: m.DiscordSendDialog })));
const ExtensionGuideModal = lazy(() => import("./components/ExtensionGuideModal").then(m => ({ default: m.ExtensionGuideModal })));
const KeyboardShortcutsModal = lazy(() => import("./components/KeyboardShortcutsModal").then(m => ({ default: m.KeyboardShortcutsModal })));
const VideoView = lazy(() => import("./components/VideoView").then(m => ({ default: m.VideoView })));

export default function App() {
  const [section, setSection] = useState<Section>("Downloads");
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("General");
  const [composing, setComposing] = useState(false);
  const [destination, setDestination] = useState<string | null>(null);
  const [workingDirectory, setWorkingDirectory] = useState<string | null>(null);
  const [extensionGuideOpen, setExtensionGuideOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [extensionGuideMounted, setExtensionGuideMounted] = useState(false);
  const [shortcutsMounted, setShortcutsMounted] = useState(false);
  if (extensionGuideOpen && !extensionGuideMounted) setExtensionGuideMounted(true);
  if (shortcutsOpen && !shortcutsMounted) setShortcutsMounted(true);
  const [discordSendIds, setDiscordSendIds] = useState<string[] | null>(null);
  const shortcuts = useShortcuts();
  const [notificationHistoryOpen, setNotificationHistoryOpen] = useState(false);
  const [focusRequest, setFocusRequest] = useState<{ ids: string[]; nonce: number } | null>(null);

  // A URL (and its captured headers/cookies) the extension asked us to open in
  // the Video view. The nonce lets the same URL re-trigger VideoView (e.g.
  // sending the same tweet twice).
  const [videoPrefill, setVideoPrefill] = useState<{
    url: string;
    headers: [string, string][];
    autoDownload: boolean;
    title?: string;
    directory?: string;
    useFallback?: boolean;
    nonce: number;
  } | null>(null);

  // A URL sent from the browser extension to prefill the AddDownload input.
  const [downloadPrefill, setDownloadPrefill] = useState<{
    url: string;
    headers: [string, string][];
    directory?: string;
    nonce: number;
  } | null>(null);

  // A music link the extension asked us to open in the Music view.
  const [musicPrefill, setMusicPrefill] = useState<{
    url: string;
    headers: [string, string][];
    directory?: string;
    nonce: number;
  } | null>(null);

  const [imagePrefill, setImagePrefill] = useState<{
    url: string;
    headers: [string, string][];
    directory?: string;
    nonce: number;
  } | null>(null);

  const { settings } = useAppearance();
  const toasts = useToasts(settings.notificationLimit || 25);
  const openExtensionSettings = useCallback(() => { setSettingsTab("Extension"); setSection("Settings"); }, []);
  const extensionUpdates = useExtensionUpdates(toasts.add, openExtensionSettings);
  const queue = useQueue();
  const { settings: engineSettings } = useSettings();
  const deleteFilesOnRemove = engineSettings?.deleteFilesOnRemove ?? true;
  const video = useVideo();
  const music = useMusic(section === "Music");
  const remux = useConvert(section === "Convert", workingDirectory ?? destination, (finished) => toasts.add({
    id: `convert-done-${Date.now()}`, type: "success",
    title: finished.length === 1 ? "Conversion finished" : `${finished.length} conversions finished`,
    message: finished.length === 1 ? finished[0]!.filename : "Open Convert to see the results.",
  }));

  useEffect(() => {
    if (!hasEngine()) return;

    const showError = (id: string, title: string, error: unknown) => {
      toasts.add({
        id,
        type: "error",
        title,
        message: error instanceof Error ? error.message : String(error),
      });
    };

    const showUpdateAvailable = (version: string) => {
      toasts.add({
        id: "update-available",
        type: "info",
        title: `Zoltraak ${version} is available`,
        message: "Download the signed update files when you are ready.",
        durationMs: 12000,
        dismissLabel: "Later",
        action: {
          label: "Download update",
          onClick: () => {
            void engine.downloadUpdate().catch((error: unknown) => {
              showError("update-download-error", "Could not download update", error);
            });
          },
        },
      });
    };

    const showUpdateReady = (version: string) => {
      toasts.add({
        id: "update-ready",
        type: "success",
        title: `Zoltraak ${version} is ready`,
        message: "Install now, or approve installation for the next application start.",
        durationMs: 12000,
        action: {
          label: "Install now",
          onClick: () => {
            void engine.installAndRestart().catch((error: unknown) => {
              showError("update-install-error", "Could not install update", error);
            });
          },
        },
        secondaryAction: {
          label: "Install on next start",
          onClick: () => {
            void engine.installOnNextStart().then(() => {
              toasts.add({
                id: "update-install-scheduled",
                type: "info",
                title: "Update scheduled",
                message: `Zoltraak ${version} will install the next time the application starts.`,
              });
            }).catch((error: unknown) => {
              showError("update-schedule-error", "Could not schedule update", error);
            });
          },
        },
      });
    };

    const unlistenAvailable = onUpdateAvailable(({ version }) => showUpdateAvailable(version));
    const unlistenReady = onUpdateReady(({ version }) => showUpdateReady(version));
    void engine.updateProgress().then((progress) => {
      if (progress?.phase === "ready") showUpdateReady(progress.version);
    });
    return () => {
      unlistenAvailable();
      unlistenReady();
    };
  }, [toasts.add]);

  // A video job, a music job and a queued download are different subsystems
  // with different ids. They share a list, so every action has to be
  // dispatched on which one the row came from — calling `engine.pause` with a
  // video job's id was previously a silent no-op.
  const softRemove = useSoftRemove({
    addToast: toasts.add,
    commit: (item, deleteFile) =>
      item.isVideo ? video.remove(item.id, deleteFile) : item.isMusic ? music.remove(item.id, deleteFile) : queue.remove(item.id, deleteFile),
  });

  const visibleMusicJobs = useMemo(
    () => music.jobs.filter((job) => !softRemove.pending.has(job.id)),
    [music.jobs, softRemove.pending],
  );

  // Aggregate rate across both subsystems. It lives in the header rather than
  // in a headline readout: given monumental scale it becomes the total-speed
  // hero of exactly the dashboard this panel replaced.
  const throughput = useMemo(
    () =>
      mergeItems(queue.items, video.jobs, music.jobs)
        .filter((item) => item.state === "downloading")
        .reduce((sum, item) => sum + (item.bytesPerSecond || 0), 0),
    [queue.items, video.jobs, music.jobs],
  );

  const notificationItems = useMemo(() => mergeItems(queue.items, video.jobs, music.jobs), [queue.items, video.jobs, music.jobs]);
  useDownloadNotifications(notificationItems, toasts.add);

  const recoverDownload = useCallback(async (item: QueueItem, kind: RecoveryKind) => {
    if (kind === "signin") {
      setExtensionGuideOpen(true);
      toasts.add({ id: `signin-help-${item.id}`, type: "info", title: "Share your browser sign-in",
        message: "Sign in on the source site, then use the extension’s Share this site’s sign-in action. Return here and retry the download.", downloadIds: [item.id] });
      return;
    }
    if (kind === "extract") {
      setVideoPrefill({ url: item.url, title: item.filename, headers: [], autoDownload: false, useFallback: true, nonce: Date.now() });
      setSection("Video"); return;
    }
    if (kind === "folder") {
      const directory = await engine.chooseDirectory(item.directory || undefined);
      if (!directory) return;
      if (item.isVideo) {
        setVideoPrefill({ url: item.url, title: item.filename, headers: [], directory, autoDownload: false, nonce: Date.now() });
        setSection("Video");
      } else if (item.isMusic) {
        setMusicPrefill({ url: item.url, headers: [], directory, nonce: Date.now() }); setSection("Music");
      } else {
        setDownloadPrefill({ url: item.url, headers: [], directory, nonce: Date.now() }); setComposing(true); setSection("Downloads");
      }
    }
  }, [toasts.add]);

  const openNotification = useCallback((notification: { downloadIds?: string[] }) => {
    const ids = notification.downloadIds?.filter(id => notificationItems.some(item => item.id === id) && !softRemove.pending.has(id)) ?? [];
    if (!ids.length) {
      toasts.add({ id: "notification-entry-unavailable", type: "info", title: "Entry unavailable", message: "This entry has been removed or is pending removal. Its notification remains in history." }); return;
    }
    setSection("Downloads");
    setFocusRequest(previous => ({ ids, nonce: (previous?.nonce ?? 0) + 1 }));
  }, [notificationItems, softRemove.pending, toasts.add]);

  const retryNotifiedDownloads = useCallback(async (ids: string[]) => {
    const requested = new Set(ids);
    const failed = notificationItems.filter(item => requested.has(item.id) && item.state === "failed" && !softRemove.pending.has(item.id));
    let retried = 0;
    for (const item of failed) {
      const ok = await (item.isVideo ? video.resume(item.id) : item.isMusic ? music.resume(item.id) : queue.resume(item.id));
      if (ok) retried += 1;
    }
    toasts.add({ id: "notification-retries", type: retried < failed.length ? "error" : "info", title: retried ? `Queued ${retried} ${retried === 1 ? "retry" : "retries"}` : "No retries queued",
      message: failed.length ? (retried === failed.length ? "The failed downloads are back in the queue." : "Some retries failed. Review their recovery actions in Downloads.") : "There are no current failed downloads linked to these notifications.", downloadIds: failed.map(item => item.id) });
  }, [notificationItems, softRemove.pending, video.resume, music.resume, queue.resume, toasts.add]);

  const actions = useMemo<QueueActions>(
    () => ({
      toggle: (item) => {
        const running = item.state === "downloading" || item.state === "waiting";
        if (item.isVideo) {
          void (running ? video.pause(item.id) : video.resume(item.id));
        } else if (item.isMusic) {
          void (running ? music.pause(item.id) : music.resume(item.id));
        } else {
          void queue.toggle(item);
        }
      },
      pause: (item) =>
        item.isVideo ? video.pause(item.id) : item.isMusic ? music.pause(item.id) : queue.pause(item.id),
      resume: (item) =>
        item.isVideo ? video.resume(item.id) : item.isMusic ? music.resume(item.id) : queue.resume(item.id),
      remove: (items: QueueItem[], deleteFile = false) => softRemove.remove(items, deleteFile),
      recover: recoverDownload,
      reveal: (id) => queue.reveal(id),
      open: (id) => {
        if (hasEngine()) {
          void engine.open(id).catch(() => {
            void queue.reveal(id);
          });
        }
      },
      dragOut: hasEngine()
        ? (item) => {
            void engine.dragOut(item.id).catch((error: unknown) => {
              toasts.add({
                id: `drag-out-${Date.now()}`,
                type: "error",
                title: "Could not copy file",
                message: error instanceof Error ? error.message : String(error),
              });
            });
          }
        : undefined,
      // The unified table carries resolved local paths for every pipeline.
      convert: hasEngine()
        ? (items: QueueItem[]) => {
            const paths = items.flatMap((item) => item.path ? [item.path] : []);
            if (paths.length === 0) return;
            setSection("Convert");
            void remux.addPaths(paths, true);
          }
        : undefined,
      // Quick conversion resolves ids in the engine and starts immediately.
      remux: hasEngine()
        ? (ids: string[], container: Container) => {
            if (ids.length === 0) return;
            void Promise.allSettled(ids.map((id) => engine.remuxDownload(id, container, remux.defaultDirectory ?? undefined))).then(
              (results) => {
                const failed = results.filter((r) => r.status === "rejected");
                void remux.refresh();
                if (failed.length === results.length) {
                  const first = failed[0];
                  toasts.add({
                    id: `remux-failed-${Date.now()}`,
                    type: "error",
                    title: "Could not repackage",
                    message:
                      first && first.status === "rejected"
                        ? String(
                            first.reason instanceof Error ? first.reason.message : first.reason,
                          )
                        : "The engine refused the file.",
                  });
                  return;
                }
                setSection("Convert");
                toasts.add({
                  id: `remux-${Date.now()}`,
                  type: "info",
                  title: `Repackaging to .${container}`,
                  message:
                    failed.length > 0
                      ? `${failed.length} could not be started; the rest are saved in the working folder's Converted subfolder.`
                      : "The output is saved in the working folder's Converted subfolder. The original is kept.",
                  action: { label: "View", onClick: () => setSection("Convert") },
                });
              },
            );
          }
        : undefined,
      // Only offered where there is an engine to send with. The engine also
      // owns the "is this even set up?" question — its refusal names the
      // screen to fix it on, which is more use than a hidden menu entry.
      sendToDiscord: hasEngine() ? (ids: string[]) => { if (ids.length) setDiscordSendIds(ids); } : undefined,
    }),
    [queue.toggle, queue.pause, queue.resume, queue.reveal, video.pause, video.resume,
      music.pause, music.resume, softRemove.remove, toasts.add, remux.refresh, remux.addPaths, remux.defaultDirectory, recoverDownload],
  );

  const addUrls = useCallback(async (urls: string[], directory?: string) => {
    queue.clearError();
    // One pasted or dropped link that belongs on another tab is offered there
    // first; declining downloads it as a regular file.
    const [only] = urls;
    const target = urls.length === 1 && only ? classifyLink(only) : null;
    if (only && target && window.confirm(routeMessage(target))) {
      const prefill = { url: only, headers: [] as [string, string][], nonce: Date.now() };
      if (target.section === "Video") setVideoPrefill({ ...prefill, autoDownload: false });
      else if (target.section === "Images") setImagePrefill(prefill);
      else setMusicPrefill(prefill);
      setSection(target.section);
      return { added: urls, failed: [] };
    }
    const capturedUrl = parseUrls(downloadPrefill?.url ?? "")[0];
    const result = await addLinks(urls, url => engine.add(url, directory, undefined, undefined,
      capturedUrl === url && urls.length === 1 ? downloadPrefill?.headers : undefined));
    await queue.refresh();
    if (result.failed.length) toasts.add({ id: `add-failed-${Date.now()}`, type: "error",
      title: `${result.failed.length} ${result.failed.length === 1 ? "link was" : "links were"} not added`,
      message: "Unsuccessful links remain in the entry. Correct them and try again." });
    return result;
  }, [queue.clearError, queue.refresh, downloadPrefill, toasts.add]);

  useEffect(() => {
    const handlePaste = (event: ClipboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (
        target?.isContentEditable ||
        (target && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))
      ) {
        return;
      }

      const text = event.clipboardData?.getData("text/plain") ?? "";
      if (!text.trim()) return;
      const urls = parseUrls(text);
      if (urls.length === 0) {
        toasts.add({
          id: `paste-invalid-${Date.now()}`,
          type: "error",
          title: "No download link found",
          message: "Paste a complete http:// or https:// URL.",
        });
        return;
      }

      event.preventDefault();
      setSection("Downloads");
      setDownloadPrefill({ url: text, headers: [], nonce: Date.now() });
      setComposing(true);
    };

    window.addEventListener("paste", handlePaste);
    return () => window.removeEventListener("paste", handlePaste);
  }, [addUrls, toasts.add]);

  useEffect(() => {
    if (!hasEngine()) return;
    let cancelled = false;
    engine
      .defaultDirectory()
      .then((dir) => {
        if (!cancelled) setDestination(dir);
      })
      // Only used as a hint under the entry, so a failure is not worth
      // surfacing — the engine picks the same folder either way.
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [engineSettings?.defaultDirectory]);

  // "Send to Zoltraak" (download / link) from the extension: focus the entry
  // with the URL filled in.
  useEffect(() => {
    return onDownloadPrefill((payload) => {
      if (!payload.url) return;
      setSection("Downloads");
      setComposing(true);
      setDownloadPrefill({ url: payload.url, headers: payload.headers ?? [], nonce: Date.now() });
    });
  }, []);

  // "Download music with Zoltraak": jump to the Music view with the link filled in.
  useEffect(() => {
    return onMusicPrefill((payload) => {
      if (!payload.url) return;
      setSection("Music");
      setMusicPrefill({ url: payload.url, headers: payload.headers ?? [], nonce: Date.now() });
    });
  }, []);

  // "Download post with Zoltraak": open Images and immediately analyze it.
  useEffect(() => {
    return onImagePrefill((payload) => {
      if (!payload.url) return;
      setSection("Images");
      setImagePrefill({ url: payload.url, headers: payload.headers ?? [], nonce: Date.now() });
    });
  }, []);

  // "Send video to Zoltraak": jump to the Video view with the URL filled in.
  useEffect(() => {
    return onVideoPrefill((payload) => {
      if (!payload.url) return;
      setSection("Video");
      setVideoPrefill({
        url: payload.url,
        headers: payload.headers ?? [],
        autoDownload: Boolean(payload.autoDownload),
        title: payload.title,
        nonce: Date.now(),
      });
    });
  }, []);

  // External navigation (from web guide or deep link): focus window and navigate to section / tab
  useEffect(() => {
    return onAppNavigate((payload) => {
      if (payload.section) {
        setSection(payload.section as Section);
      }
      if (payload.tab) {
        setSettingsTab(payload.tab as SettingsTab);
      }
    });
  }, []);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (shortcutsBlocked(e) || isTypingTarget(e.target)) return;
      if (matchesShortcut(shortcuts, "openShortcuts", e)) {
        e.preventDefault();
        setShortcutsOpen(true);
      } else if (matchesShortcut(shortcuts, "openFolder", e)) {
        e.preventDefault();
        if (hasEngine()) {
          void engine.revealFolder(destination || undefined).catch(() => {});
        }
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [destination, shortcuts]);


  return (
    /* The panel is the window: a header flush to the top edge, the working
       area between, and each view deciding what scrolls inside it. Nothing
       floats over the content, so no destination costs the register height. */
    <div className="relative flex h-[100dvh] flex-col overflow-hidden bg-panel">
      {/* Schematic millimeter blueprint grid underlay - sits behind interface chrome & panels */}
      <div
        id="atmosphere-grid-underlay"
        aria-hidden="true"
        className="pointer-events-none fixed inset-0 z-0 select-none"
      />
      <div
        id="atmosphere-noise-overlay"
        aria-hidden="true"
        className="pointer-events-none fixed inset-0 select-none"
      />
      <ToastContainer toasts={toasts.toasts} onDismiss={toasts.dismiss} hidden={notificationHistoryOpen} />

      <div className="relative z-[1] flex min-h-0 flex-1 flex-col overflow-hidden">
        <PanelHeader
          active={section}
          onSelect={setSection}
          status={queue.status}
          throughput={throughput}
          onOpenExtension={() => setExtensionGuideOpen(true)}
          onOpenShortcuts={() => setShortcutsOpen(true)}
          extensionUpdates={extensionUpdates}
          notifications={toasts.history}
          unreadCount={toasts.unreadCount}
          onMarkAllRead={toasts.markAllRead}
          onClearNotifications={toasts.clearHistory}
          onRemoveNotification={toasts.removeNotification}
          onOpenNotification={openNotification}
          onNotificationVisibilityChange={setNotificationHistoryOpen}
          onRetryFailedNotifications={retryNotifiedDownloads}
          failedDownloadIds={notificationItems.filter(item => item.state === "failed" && !softRemove.pending.has(item.id)).map(item => item.id)}
        />

        {/* Mount on first visit, then retain filters, analyzed media and drafts
            across trips to another section. */}
      <main className="flex min-h-0 flex-1 flex-col overflow-hidden">
        <div className={section === "Downloads" ? "flex min-h-0 flex-1" : "hidden"}>
          <DownloadsView
            active={section === "Downloads"}
            items={queue.items}
            videoJobs={video.jobs}
            musicJobs={music.jobs}
            pendingRemoval={softRemove.pending}
            status={queue.status}
            error={queue.error || video.error || music.error}
            onClearError={() => { queue.clearError(); video.clearError(); music.clearError(); }}
            onAdd={addUrls}
            actions={actions}
            deleteFilesOnRemove={deleteFilesOnRemove}
            destination={destination}
            onWorkingDirectoryChange={setWorkingDirectory}
            composing={composing}
            onComposingChange={setComposing}
            prefill={downloadPrefill}
            focusRequest={focusRequest}
            onInvalidIntake={(message) =>
              toasts.add({
                id: `drop-invalid-${Date.now()}`,
                type: "error",
                title: "Nothing to download",
                message,
              })
            }
          />
        </div>

        <VisitedPanel active={section === "Video"} className="min-h-0 flex-1 overflow-y-auto">
          <VideoView
            video={{ ...video, jobs: video.jobs.filter((job) => !softRemove.pending.has(job.id)) }}
            onRemove={(job, deleteFile = deleteFilesOnRemove) => softRemove.remove([videoJobToSnapshot(job)], deleteFile)}
            onRecover={recoverDownload}
            deleteFilesOnRemove={deleteFilesOnRemove}
            prefill={videoPrefill}
            onAddToast={toasts.add}
            onNavigateToDownloads={() => setSection("Downloads")}
          />
        </VisitedPanel>

        <VisitedPanel active={section === "Images"} className="min-h-0 flex-1 overflow-y-auto">
          <ImagesView
            prefill={imagePrefill}
            onAddToast={toasts.add}
            onNavigateToDownloads={() => setSection("Downloads")}
          />
        </VisitedPanel>

        <VisitedPanel active={section === "Music"} className="min-h-0 flex-1 overflow-y-auto">
          <MusicView
            music={{ ...music, jobs: visibleMusicJobs, groups: groupJobs(visibleMusicJobs) }}
            onRemove={(job, deleteFile = deleteFilesOnRemove) => softRemove.remove([musicJobToSnapshot(job)], deleteFile)}
            onRecover={recoverDownload}
            deleteFilesOnRemove={deleteFilesOnRemove}
            prefill={musicPrefill}
            onAddToast={toasts.add}
          />
        </VisitedPanel>

        <VisitedPanel active={section === "Convert"} className="min-h-0 flex-1 overflow-y-auto">
          <ConvertView convert={remux} onAddToast={toasts.add} />
        </VisitedPanel>

        <VisitedPanel active={section === "Discord"} className="min-h-0 flex-1 overflow-y-auto">
          <DiscordSettings active={section === "Discord"} addToast={toasts.add} />
        </VisitedPanel>

        <VisitedPanel active={section === "Rules"} className="min-h-0 flex-1 overflow-y-auto">
          <CategoriesView />
        </VisitedPanel>

        <VisitedPanel active={section === "Settings"} className="min-h-0 flex-1 overflow-y-auto">
          <SettingsShell
            tab={settingsTab}
            onTabChange={setSettingsTab}
            onOpenExtension={() => setExtensionGuideOpen(true)}
            extensionUpdates={extensionUpdates}
            onAddToast={toasts.add}
          />
        </VisitedPanel>
      </main>
      </div>

      <Suspense fallback={null}>
      {extensionGuideMounted && <ExtensionGuideModal
        updates={extensionUpdates}
        open={extensionGuideOpen}
        onClose={() => setExtensionGuideOpen(false)}
        onOpenSettings={() => {
          setExtensionGuideOpen(false);
          setSettingsTab("Extension");
          setSection("Settings");
        }}
      />}
      {discordSendIds && <DiscordSendDialog items={notificationItems.filter(item => discordSendIds.includes(item.id))}
        onClose={() => setDiscordSendIds(null)} onSettings={() => { setDiscordSendIds(null); setSection("Discord"); }}
        onQueued={count => {
          setDiscordSendIds(null);
          toasts.add({ id: `discord-${Date.now()}`, type: "info", title: `${count} queued for Discord`,
            message: "Delivery progress is available in Discord history.", action: { label: "View", onClick: () => setSection("Discord") } });
        }} />}
      {shortcutsMounted && <KeyboardShortcutsModal open={shortcutsOpen} onClose={() => setShortcutsOpen(false)} />}
      </Suspense>
      <ReleaseNotesDialog autoOpen />
    </div>
  );
}
