import { useEffect, useRef, useState, type ReactElement } from "react";
import { SlideGroup } from "./SlideGroup";
import { ArrowUpRight, Bell, BookOpen, Check, Close, Discord, DownloadGlyph, Film, Frame, Keyboard, ListChecks, Menu, Note, Plug, Sliders, Trash, Transfer } from "./Icons";
import { formatRate } from "../lib/format";
import { engine as apiEngine, hasEngine, openGuide, onUpdateReady } from "../lib/engine";
import { useAppearance } from "../lib/useAppearance";
import type { NotificationRecord } from "../lib/useToasts";
import { TooltipGroup } from "./Tooltip";
import type { ExtensionUpdates } from "../lib/useExtensionUpdates";
import { WindowControls } from "./WindowControls";

export const SECTIONS = ["Downloads", "Video", "Images", "Music", "Convert", "Discord", "Rules", "Settings"] as const;

export type Section = (typeof SECTIONS)[number];
const sectionLabel = (section: Section) => section === "Images" ? "Posts" : section;

const GLYPH: Record<Section, (props: { className?: string }) => ReactElement> = {
  Downloads: DownloadGlyph,
  Video: Film,
  Images: Frame,
  Music: Note,
  Convert: Transfer,
  Discord,
  Rules: ListChecks,
  Settings: Sliders,
};

export type EngineStatus = "connecting" | "live" | "detached";

/**
 * The control desk header.
 *
 * Flush to the window edge and part of the panel, not a floating island above
 * the content. The old island cost 4.75rem of top padding on a fixed-height
 * window and put the brand inside the navigation; here the identity is a works
 * plate and the destinations are engraved legends with an illuminated
 * indicator under the one in use.
 */
export function PanelHeader({
  active,
  onSelect,
  status,
  throughput,
  onOpenExtension,
  onOpenShortcuts,
  extensionUpdates,
  notifications = [],
  unreadCount = 0,
  onMarkAllRead,
  onClearNotifications,
  onRemoveNotification,
  onOpenNotification,
  onRetryFailedNotifications,
  failedDownloadIds,
  onNotificationVisibilityChange,
}: {
  active: Section;
  onSelect: (section: Section) => void;
  status: EngineStatus;
  /** Aggregate bytes per second across everything currently running. */
  throughput: number;
  onOpenExtension?: () => void;
  onOpenShortcuts?: () => void;
  extensionUpdates?: ExtensionUpdates;
  notifications?: NotificationRecord[];
  unreadCount?: number;
  onMarkAllRead?: () => void;
  onClearNotifications?: () => void;
  onRemoveNotification?: (id: string) => void;
  onOpenNotification?: (notification: NotificationRecord) => void;
  onRetryFailedNotifications?: (ids: string[]) => Promise<void>;
  failedDownloadIds?: string[];
  onNotificationVisibilityChange?: (open: boolean) => void;
}) {
  const extensionConnected = status === "live" && Boolean(extensionUpdates?.sources?.clients.some(client => client.online));
  const extensionAttention = Boolean(extensionUpdates?.busy || extensionUpdates?.pending.length
    || extensionUpdates?.rows.some(row => row.installed && (row.updateAvailable || row.needsSignedPackage)));
  const extensionLabel = extensionUpdates?.error ? "Extension status unavailable — open setup"
    : extensionAttention ? "Extension update needs attention — open setup"
    : extensionConnected ? "Browser extension connected — open setup"
    : "No browser extension connected — open setup";
  const extensionTone = extensionUpdates?.error ? "text-danger"
    : extensionAttention ? "text-caution" : extensionConnected ? "text-clear" : "text-danger";
  const [updateReady, setUpdateReady] = useState<string | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [notifsOpen, setNotifsOpen] = useState(false);
  const [notificationFilter, setNotificationFilter] = useState<"all" | "errors">("all");
  const [retrying, setRetrying] = useState(false);
  useEffect(() => { onNotificationVisibilityChange?.(notifsOpen); }, [notifsOpen, onNotificationVisibilityChange]);
  const shownNotifications = notificationFilter === "errors" ? notifications.filter(item => item.type === "error") : notifications;
  const failedIds = [...new Set(notifications.filter(item => item.type === "error").flatMap(item => item.downloadIds ?? []))].filter(id => !failedDownloadIds || failedDownloadIds.includes(id));
  const menuRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const notifsRef = useRef<HTMLDivElement>(null);
  const { settings } = useAppearance();
  const speedUnit = settings.telemetry.speedUnit ?? "MBps";

  useEffect(() => {
    if (!hasEngine()) return;
    return onUpdateReady((p) => {
      setUpdateReady(p.version);
    });
  }, []);

  useEffect(() => {
    if (!menuOpen && !notifsOpen) return;

    const dismiss = (event: PointerEvent) => {
      const target = event.target as Node;
      if (menuOpen && !menuRef.current?.contains(target)) setMenuOpen(false);
      if (notifsOpen && !notifsRef.current?.contains(target)) setNotifsOpen(false);
    };
    const dismissWithKeyboard = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (menuOpen) {
          setMenuOpen(false);
          menuButtonRef.current?.focus();
        }
        if (notifsOpen) {
          setNotifsOpen(false);
        }
      }
    };
    const closeAtFullWidth = (event: MediaQueryListEvent) => {
      if (event.matches) setMenuOpen(false);
    };
    const fullWidth = window.matchMedia("(min-width: 1024px)");

    window.addEventListener("pointerdown", dismiss, true);
    window.addEventListener("keydown", dismissWithKeyboard);
    fullWidth.addEventListener("change", closeAtFullWidth);
    return () => {
      window.removeEventListener("pointerdown", dismiss, true);
      window.removeEventListener("keydown", dismissWithKeyboard);
      fullWidth.removeEventListener("change", closeAtFullWidth);
    };
  }, [menuOpen, notifsOpen]);

  const ActiveGlyph = GLYPH[active];
  const selectSection = (section: Section) => {
    onSelect(section);
    setMenuOpen(false);
  };

  return (
    <header data-tauri-drag-region className="app-header relative flex min-w-0 shrink-0 select-none items-stretch border-b border-edge-strong bg-face">
      <div data-tauri-drag-region className="flex shrink-0 items-center gap-2.5 border-r border-edge px-3 sm:px-5">
        <span data-tauri-drag-region className="legend text-[11px] tracking-[0.24em] text-ink">Zoltraak</span>
      </div>

      <div data-tauri-drag-region ref={menuRef} className="relative flex min-w-8 flex-1 lg:hidden">
        <button
          ref={menuButtonRef}
          type="button"
          aria-label={`${menuOpen ? "Close" : "Open"} navigation. Current section: ${sectionLabel(active)}`}
          aria-expanded={menuOpen}
          aria-controls="compact-primary-navigation"
          onClick={() => setMenuOpen((open) => !open)}
          className={`focus-ring flex min-w-0 items-center gap-2 border-r border-edge px-3 py-3 transition-colors ${menuOpen ? "bg-recess text-ink" : "text-ink-dim hover:bg-recess/60 hover:text-ink"}`}
        >
          <Menu className="size-4 shrink-0" />
          <ActiveGlyph className="size-[15px] shrink-0 text-route" />
          <span className="legend truncate text-[11px]">{sectionLabel(active)}</span>
        </button>

        {menuOpen && (
          <div
            id="compact-primary-navigation"
            className="plate lift absolute left-0 top-full z-50 max-h-[calc(100dvh-3.25rem)] w-[min(26rem,calc(100vw-1.5rem))] overflow-y-auto rounded-hw-lg p-2"
          >
            <p className="legend px-2 pb-1.5 pt-1 text-[11px] text-ink-faint">Destinations</p>
            <nav aria-label="Primary" className="grid grid-cols-2 border border-edge">
              {SECTIONS.map((section) => {
                const Glyph = GLYPH[section];
                const current = active === section;

                return (
                  <button
                    key={section}
                    type="button"
                    onClick={() => selectSection(section)}
                    aria-current={current ? "page" : undefined}
                    className={`focus-ring flex min-h-11 items-center gap-2.5 border-b border-r border-edge px-3 text-left transition-colors even:border-r-0 [&:nth-last-child(-n+2)]:border-b-0 ${current ? "bg-route text-on-signal" : "bg-face text-ink-dim hover:bg-recess hover:text-ink"}`}
                  >
                    <Glyph className="size-4 shrink-0" />
                    <span className="legend truncate text-[11px]">{sectionLabel(section)}</span>
                  </button>
                );
              })}
            </nav>

            <p className="legend px-2 pb-1.5 pt-3 text-[11px] text-ink-faint">Desk controls</p>
            <div className={`grid border border-edge ${updateReady ? "grid-cols-4" : "grid-cols-3"}`}>
              {updateReady && (
                <button
                  type="button"
                  onClick={() => {
                    setMenuOpen(false);
                    void apiEngine.installAndRestart();
                  }}
                  className="focus-ring flex min-h-11 min-w-0 items-center justify-center gap-2 border-r border-edge bg-route/15 px-2 text-route transition-colors last:border-r-0 hover:bg-route/25"
                >
                  <span className="size-1.5 shrink-0 rounded-full bg-route" />
                  <span className="legend truncate text-[11px]">Install {updateReady}</span>
                </button>
              )}
              {onOpenExtension && (
                <button
                  type="button"
                  onClick={() => {
                    setMenuOpen(false);
                    onOpenExtension();
                  }}
                  className="focus-ring flex min-h-11 min-w-0 items-center justify-center gap-2 border-r border-edge px-2 text-ink-dim transition-colors last:border-r-0 hover:bg-recess hover:text-ink"
                >
                  <Plug className="size-4 shrink-0 text-route" />
                  <span className="legend truncate text-[11px]">Extension</span>
                </button>
              )}
              <button
                type="button"
                onClick={() => {
                  setMenuOpen(false);
                  void openGuide();
                }}
                className="focus-ring flex min-h-11 min-w-0 items-center justify-center gap-2 border-r border-edge px-2 text-ink-dim transition-colors last:border-r-0 hover:bg-recess hover:text-ink"
              >
                <BookOpen className="size-4 shrink-0" />
                <span className="legend truncate text-[11px]">Guide</span>
                <ArrowUpRight className="size-3 shrink-0 text-ink-faint" />
              </button>
            </div>
          </div>
        )}
      </div>

      <SlideGroup as="nav" data-tauri-drag-region aria-label="Primary" className="hidden min-w-0 flex-1 items-stretch pr-8 lg:flex" indicatorClassName="main-tab-indicator">
        {SECTIONS.map((section) => {
          const Glyph = GLYPH[section];
          const current = active === section;

          return (
            <button
              key={section}
              type="button"
              onClick={() => selectSection(section)}
              aria-label={sectionLabel(section)}
              aria-current={current ? "page" : undefined}
              className={`focus-ring relative main-tab flex shrink-0 items-center gap-2 border-r border-edge px-4 py-3 transition-colors duration-150 ${current
                  ? "text-ink"
                  : "text-ink-dim hover:bg-recess/60 hover:text-ink"
                }`}
            >
              <Glyph className="size-[15px]" />
              <span className="legend hidden text-[11px] 2xl:inline">{sectionLabel(section)}</span>
            </button>
          );
        })}
      </SlideGroup>

      <div className="header-tools ml-auto flex shrink-0 items-center justify-end gap-3 px-3 sm:gap-4 sm:px-5">
        {throughput > 0 && (
          <span
            data-measure
            aria-label="Total download speed"
            className="describer text-[12px] text-clear"
          >
            {formatRate(throughput, speedUnit)}
          </span>
        )}

        <TooltipGroup side="bottom" className="flex items-center gap-2">
          {updateReady && (
            <button
              type="button"
              onClick={() => void apiEngine.installAndRestart()}
              data-tooltip={`Install update ${updateReady} now`}
              aria-label={`Install update ${updateReady}`}
              className="t-tt-trigger focus-ring hidden items-center gap-1.5 border border-route bg-route/20 px-2.5 py-1 text-route transition-colors hover:bg-route/30 lg:flex"
            >
              <span className="size-1.5 rounded-full bg-route" />
              <span className="legend hidden text-[11px] 2xl:inline">Update ({updateReady}) · Install</span>
            </button>
          )}

          {onOpenExtension && (
            <button
              type="button"
              onClick={onOpenExtension}
              data-tooltip={extensionLabel}
              aria-label={extensionLabel}
              className="t-tt-trigger focus-ring hidden items-center gap-1.5 border-l border-edge py-1 pl-3 text-ink-dim transition-colors hover:text-ink lg:flex"
            >
              <Plug className={`size-3.5 ${extensionTone}`} />
              <span className="legend hidden text-[11px] 2xl:inline">Extension</span>
            </button>
          )}

          {/* Notifications Inbox with transitions.dev badge */}
          <div ref={notifsRef} className="relative flex items-center">
            <button
              type="button"
              onClick={() => {
                const next = !notifsOpen;
                setNotifsOpen(next);
                if (next && onMarkAllRead) {
                  onMarkAllRead();
                }
              }}
              data-tooltip={`Notifications (${unreadCount} unread)`}
              aria-label={`Notifications: ${unreadCount} unread`}
              aria-expanded={notifsOpen}
              className="t-tt-trigger focus-ring relative flex items-center justify-center rounded-hw-lg p-1.5 text-ink-dim transition-colors hover:bg-recess hover:text-ink"
              style={{ position: "relative" }}
            >
              <Bell className="size-4" />
              <span
                className="t-badge"
                data-open={unreadCount > 0 ? "true" : "false"}
                aria-hidden="true"
              >
                <span className="t-badge-dot min-w-[15px] h-[15px] rounded-full bg-danger px-1 text-[11px] font-mono font-bold leading-none text-white shadow-sm ring-1 ring-face">
                  {unreadCount > 99 ? "99+" : unreadCount}
                </span>
              </span>
            </button>

            <div
              data-origin="top-right"
              role="region"
              aria-label="Notification history"
              aria-hidden={!notifsOpen}
              inert={!notifsOpen}
              className={`t-dropdown plate lift context-menu-glass absolute right-0 top-[calc(100%+8px)] z-50 w-80 max-w-[calc(100vw-2rem)] rounded-hw-xl p-2.5 shadow-2xl ${
                notifsOpen ? "is-open" : ""
              }`}
            >
              <div className="flex items-center justify-between border-b border-edge pb-2 px-1">
                <div className="flex items-center gap-1.5">
                  <span className="legend text-[11px] font-semibold text-ink">Notifications</span>
                  <span className="rounded-hw bg-recess px-1.5 py-0.5 text-[11px] font-mono text-ink-dim">
                    {notifications.length} / {settings.notificationLimit || 25}
                  </span>
                </div>
                <div className="flex items-center gap-1">
                  {notifications.length > 0 && onClearNotifications && (
                    <button
                      type="button"
                      onClick={onClearNotifications}
                      data-tooltip="Clear all notifications"
                      className="focus-ring legend flex items-center gap-1 rounded-hw px-1.5 py-0.5 text-[11px] text-ink-faint transition-colors hover:text-danger"
                    >
                      <Trash className="size-3" />
                      <span>Clear</span>
                    </button>
                  )}
                  {unreadCount > 0 && onMarkAllRead && (
                    <button
                      type="button"
                      onClick={onMarkAllRead}
                      data-tooltip="Mark all notifications read"
                      className="focus-ring legend flex items-center gap-1 rounded-hw px-1.5 py-0.5 text-[11px] text-ink-dim transition-colors hover:text-ink"
                    >
                      <Check className="size-3 text-clear" />
                      <span>Read all</span>
                    </button>
                  )}
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-2 border-b border-edge px-1 py-2 text-[11px]">
                <div role="group" aria-label="Filter notifications" className="flex gap-1">
                  {(["all", "errors"] as const).map(filter => <button key={filter} type="button" aria-pressed={notificationFilter === filter} onClick={() => setNotificationFilter(filter)} className={`focus-ring rounded-hw border px-2 py-1 ${notificationFilter === filter ? "border-route bg-route text-on-signal" : "border-edge-strong text-ink-dim"}`}>{filter === "all" ? "All" : "Errors only"}</button>)}
                </div>
                {onRetryFailedNotifications && <button type="button" disabled={retrying || !failedIds.length} className="focus-ring ml-auto rounded-hw border border-edge-strong px-2 py-1 text-ink disabled:opacity-40" onClick={() => { setRetrying(true); void onRetryFailedNotifications(failedIds).finally(() => setRetrying(false)); }}>{retrying ? "Retrying…" : "Retry failed"}</button>}
              </div>
              <div className="max-h-72 overflow-y-auto divide-y divide-edge/60 pt-1">
                {shownNotifications.length === 0 ? (
                  <div className="py-6 text-center">
                    <p className="legend text-[11px] text-ink-faint">{notificationFilter === "errors" ? "No error notifications" : "No notifications"}</p>
                    <p className="mt-0.5 text-[11px] text-ink-dim">
                      Completed transfers and alerts will appear here.
                    </p>
                  </div>
                ) : (
                  shownNotifications.map((item) => (
                    <div
                      key={item.id}
                      className={`group relative flex items-start gap-2.5 py-2 px-1 text-left transition-colors ${
                        !item.read ? "bg-recess/40" : ""
                      }`}
                    >
                      <span
                        className={`mt-1 size-1.5 shrink-0 rounded-full ${
                          item.type === "error"
                            ? "bg-danger"
                            : item.type === "success"
                              ? "bg-clear"
                              : "bg-route"
                        }`}
                      />
                      <button type="button" className="focus-ring min-w-0 flex-1 rounded-hw text-left disabled:cursor-default" disabled={!item.downloadIds?.length || !onOpenNotification} aria-label={`Open download notification: ${item.title}`} onClick={() => { onOpenNotification?.(item); setNotifsOpen(false); }}>
                        <div className="flex items-center justify-between gap-1">
                          <p className="truncate text-[11.5px] font-medium text-ink">
                            {item.title}
                          </p>
                          <span className="shrink-0 text-[11px] font-mono text-ink-faint">
                            {new Date(item.timestamp).toLocaleTimeString([], {
                              hour: "2-digit",
                              minute: "2-digit",
                            })}
                          </span>
                        </div>
                        <p className="line-clamp-2 text-[11px] leading-relaxed text-ink-dim">
                          {item.message}
                        </p>
                        {item.downloadIds?.length && onOpenNotification ? <span className="mt-1 block text-[11px] text-route">View {item.downloadIds.length > 1 ? "downloads" : "download"}</span> : null}
                      </button>
                      {onRemoveNotification && (
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            onRemoveNotification(item.id);
                          }}
                          aria-label="Remove notification"
                          className="opacity-0 group-hover:opacity-100 focus:opacity-100 p-0.5 text-ink-faint transition-opacity hover:text-ink"
                        >
                          <Close className="size-3" />
                        </button>
                      )}
                    </div>
                  ))
                )}
              </div>
            </div>
          </div>

          <button
            type="button"
            onClick={() => void openGuide()}
            data-tooltip="Open user guide & manual"
            aria-label="Open user guide"
            className="t-tt-trigger focus-ring hidden items-center gap-1.5 border-l border-edge py-1 pl-3 text-ink-dim transition-colors hover:text-ink lg:flex"
          >
            <BookOpen className="size-3.5" />
            <span className="legend hidden text-[11px] 2xl:inline">Guide</span>
            <ArrowUpRight className="hidden size-3 text-ink-faint 2xl:block" />
          </button>
          {onOpenShortcuts && <button type="button" onClick={onOpenShortcuts} aria-label="Open keyboard shortcuts" data-tooltip="Keyboard shortcuts — view and customize" className="t-tt-trigger focus-ring flex items-center justify-center border-l border-edge py-1 pl-3 text-ink-dim hover:text-ink">
            <Keyboard className="size-4" />
          </button>}
        </TooltipGroup>
      </div>
      <WindowControls />
    </header>
  );
}
