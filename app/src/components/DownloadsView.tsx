import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { SlideGroup } from "./SlideGroup";
import { AddDownload } from "./AddDownload";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";
import { SelectInput } from "./Controls";
import { DownloadRow } from "./DownloadRow";
import { ChevronDown, Copy, Discord, ExternalLink, FolderOpen, Pause, Play, RotateCcw, Search, Transfer, Trash } from "./Icons";
import { FaultSlug, Legend } from "./Panel";
import { TheLine } from "./TheLine";
import { TooltipGroup } from "./Tooltip";
import { COLUMNS, type ColumnSpec, type ColumnId } from "../lib/columns";
import { DEFAULT_VIEW, VIEW_KEY, groupDownloads, moveColumn, readDownloadView, sortDownloads, type AddResult, type DownloadFilter, type RecoveryKind } from "../lib/downloadUx";
import { engine, type Container, type DownloadSnapshot, type MusicJob, type VideoJob } from "../lib/engine";
import { screenDroppedLinks } from "../lib/dropIntake";
import { classifyLink } from "../lib/linkKind";
import { extensionIsContainer, extensionOf, isMediaPath, isVideoPath } from "../lib/media";
import { applyClick, pruneSelection } from "../lib/selection";
import { mergeItems, type QueueItem } from "../lib/snapshots";
import { useFileStatuses } from "../lib/useFileStatuses";
import { useAppearance } from "../lib/useAppearance";
import { useFocusedDismiss } from "../lib/useFocusedDismiss";
import { flashFileOpen } from "../lib/fileOpenFeedback";
import { urlsFromDataTransfer } from "../lib/urls";
import { useShortcuts } from "../lib/useShortcuts";
import { isTypingTarget, matchesShortcut, shortcutLabel, shortcutsBlocked } from "../lib/shortcuts";

type Filter = DownloadFilter;

/**
 * One filter vocabulary for the whole app. The dashboard, the video view and
 * the history view previously each had their own — "Errors" in one place and
 * "Failed" in another for the same state, with three separate implementations.
 */
const FILTERS: { label: string; value: Filter }[] = [
  { label: "All", value: "all" },
  { label: "Downloading", value: "downloading" },
  { label: "Waiting", value: "waiting" },
  { label: "Paused", value: "paused" },
  { label: "Finished", value: "finished" },
  { label: "Failed", value: "failed" },
  { label: "Videos", value: "videos" },
  { label: "Music", value: "music" },
];

export type QueueActions = {
  toggle: (item: QueueItem) => void;
  pause: (item: QueueItem) => Promise<unknown>;
  resume: (item: QueueItem) => Promise<unknown>;
  recover?: (item: QueueItem, kind: RecoveryKind) => Promise<void>;
  /** Soft removal — the caller owns the undo window. */
  remove: (items: QueueItem[], deleteFile?: boolean) => void;
  reveal: (id: string) => void;
  open?: (id: string) => void;
  /** Absent outside the desktop app, where there is no engine to send with. */
  sendToDiscord?: (ids: string[]) => void;
  /** Repackage finished rows into another container. Same caveat. */
  remux?: (ids: string[], container: Container) => void;
  /** Stage completed local media in Convert for editable settings. */
  convert?: (items: QueueItem[]) => void;
  /** Native-only: copy a finished file by dragging its row outside the app. */
  dragOut?: (item: QueueItem) => void;
};

/**
 * Whether this row is a finished video file in some container other than the
 * one named — so "Quick convert to MP4" is offered on the `.webm` that will not play
 * on a phone, and not on the `.mp4` beside it.
 *
 * The path is the only evidence there is: a video job's title carries no
 * extension, and the engine is the one that knows what the bytes really are.
 * Being wrong here shows a menu entry that turns out to be a no-op, which the
 * engine reports back as "already a .mp4" rather than doing anything.
 */
function canRemux(item: QueueItem, container: Container): boolean {
  if (item.state !== "finished") return false;
  const path = item.path || item.filename;
  if (!isVideoPath(path)) return false;
  return !extensionIsContainer(extensionOf(path), container);
}

function canConvert(item: QueueItem): boolean {
  return item.state === "finished" && !!item.path && isMediaPath(item.path);
}

function BulkButton({
  onClick,
  icon,
  children,
  tone = "neutral",
  tooltip,
}: {
  onClick: () => void;
  icon: React.ReactNode;
  children: React.ReactNode;
  tone?: "neutral" | "danger";
  tooltip?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      data-tooltip={tooltip}
      className={`t-tt-trigger focus-ring legend flex items-center gap-1.5 rounded-hw border px-2.5 py-1 text-[11px] transition-colors ${
        tone === "danger"
          ? "border-danger/40 text-danger hover:bg-danger hover:text-on-signal"
          : "border-edge-strong text-ink-dim hover:bg-face hover:text-ink"
      }`}
    >
      {icon}
      {children}
    </button>
  );
}

/**
 * The queue, and everything the queue has ever held.
 *
 * These used to be two destinations ("Downloads" and "History") rendering the
 * same component, alongside a separate dashboard that showed only the five
 * most recent rows with no way to reach the rest. They are one view: the
 * register fills the window, and history is a filter over it rather than a
 * place.
 */
export const DownloadsView = memo(function DownloadsView({
  active = true,
  items,
  videoJobs,
  musicJobs = [],
  pendingRemoval,
  status,
  error,
  onClearError,
  onAdd,
  actions,
  deleteFilesOnRemove = true,
  destination,
  onWorkingDirectoryChange,
  composing,
  onComposingChange,
  prefill,
  onInvalidIntake,
  focusRequest,
}: {
  active?: boolean;
  items: DownloadSnapshot[];
  videoJobs: VideoJob[];
  musicJobs?: MusicJob[];
  /** Ids hidden while their undo window is open. */
  pendingRemoval: Set<string>;
  status: "connecting" | "live" | "detached";
  error: string | null;
  onClearError: () => void;
  onAdd: (urls: string[], directory?: string) => Promise<AddResult>;
  actions: QueueActions;
  deleteFilesOnRemove?: boolean;
  destination: string | null;
  onWorkingDirectoryChange?: (directory: string | null) => void;
  composing: boolean;
  onComposingChange: (open: boolean) => void;
  prefill?: { url: string; headers: [string, string][]; nonce: number; directory?: string } | null;
  onInvalidIntake?: (message: string) => void;
  focusRequest?: { ids: string[]; nonce: number } | null;
}) {
  useFocusedDismiss(error, onClearError);
  const [view, setView] = useState(readDownloadView);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());
  const shortcuts = useShortcuts();
  const { query, filter } = view;
  const setQuery = useCallback((query: string) => setView(prev => ({ ...prev, query })), []);
  const setFilter = useCallback((filter: Filter) => setView(prev => ({ ...prev, filter })), []);
  const [arrangeColumn, setArrangeColumn] = useState<ColumnId>("aspect");
  const [dropPrefill, setDropPrefill] = useState<{ url: string; headers: [string, string][]; nonce: number } | null>(null);
  const lastFocus = useRef<number | null>(null);
  const skipSelectionPrune = useRef(false);
  useEffect(() => { try { localStorage.setItem(VIEW_KEY, JSON.stringify(view)); } catch { /* Preferences are optional when storage is unavailable. */ } }, [view]);
  useEffect(() => { setDropPrefill(null); }, [prefill?.nonce]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<string | null>(null);
  const cursorId = useRef<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    item: QueueItem;
  } | null>(null);
  /** The header's own menu, which is about the table rather than about a row. */
  const [headerMenu, setHeaderMenu] = useState<{ x: number; y: number } | null>(null);

  const { settings, toggleColumn, resetColumns, toggleTelemetry } = useAppearance();
  const chosenColumns = useMemo(
    () => new Set<ColumnId>(settings.columns),
    [settings.columns],
  );
  // Registry order, so the legends and the cells can never drift apart.
  const visibleColumns = useMemo(
    () => view.columnOrder.map(id => COLUMNS.find(column => column.id === id)).filter((column): column is ColumnSpec => Boolean(column) && chosenColumns.has(column!.id)),
    [chosenColumns, view.columnOrder],
  );

  const searchRef = useRef<HTMLInputElement>(null);
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const anchorRef = useRef<string | null>(null);
  const dragDepth = useRef(0);

  const detached = status === "detached";
  const handleRemove = useCallback((item: QueueItem) => actions.remove([item], deleteFilesOnRemove), [actions.remove, deleteFilesOnRemove]);
  const handleRemoveEntry = useCallback((item: QueueItem) => actions.remove([item], false), [actions.remove]);

  const allItems = useMemo(
    () => mergeItems(items, videoJobs, musicJobs).filter((item) => !pendingRemoval.has(item.id)),
    [items, videoJobs, musicJobs, pendingRemoval],
  );

  const fileStatuses = useFileStatuses(allItems);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sortDownloads(allItems.filter((item) => {
      if (filter === "videos") {
        if (!item.isVideo) return false;
      } else if (filter === "music") {
        if (!item.isMusic) return false;
      } else if (filter !== "all" && item.state !== filter) {
        return false;
      }
      if (!q) return true;
      return item.filename.toLowerCase().includes(q) || item.url.toLowerCase().includes(q);
    }), view);
  }, [allItems, query, filter, view.sort, view.direction]);

  const groups = useMemo(() => groupDownloads(filtered, view.groupBy), [filtered, view.groupBy]);
  const visibleRows = useMemo(() => groups.flatMap(group => collapsedGroups.has(group.key) ? [] : group.items), [groups, collapsedGroups]);
  const toggleGroup = (key: string) => {
    setContextMenu(null);
    setCollapsedGroups(previous => {
      const next = new Set(previous);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  };

  useEffect(() => {
    if (!focusRequest || lastFocus.current === focusRequest.nonce) return;
    const ids = focusRequest.ids.filter(id => allItems.some(item => item.id === id));
    if (!ids.length) return;
    lastFocus.current = focusRequest.nonce;
    skipSelectionPrune.current = true;
    const first = ids[0]!;
    setQuery(""); setFilter("all"); setSelected(new Set(ids)); setExpanded(first); anchorRef.current = first;
    setCollapsedGroups(new Set());
    const frame = requestAnimationFrame(() => {
      const row = listRef.current?.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(first)}"]`);
      row?.scrollIntoView({ block: "center" }); row?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [focusRequest, allItems, setQuery, setFilter]);

  // Selection is pruned whenever the visible set changes, because a
  // filtered-out id left in the set would make a bulk action operate on
  // something the user can no longer see.
  useEffect(() => {
    if (skipSelectionPrune.current) { skipSelectionPrune.current = false; return; }
    setSelected((prev) => {
      const next = pruneSelection(prev, visibleRows);
      return next.size === prev.size ? prev : next;
    });
    setExpanded((prev) => (prev && visibleRows.some((item) => item.id === prev) ? prev : null));
  }, [visibleRows]);

  const selectedItems = useMemo(
    () => visibleRows.filter((item) => selected.has(item.id)),
    [visibleRows, selected],
  );

  const handleSelect = useCallback(
    (id: string, event: React.MouseEvent) => {
      cursorId.current = id;

      setSelected((prev) => {
        const result = applyClick({
          rows: visibleRows,
          selected: prev,
          anchor: anchorRef.current,
          id,
          shiftKey: event.shiftKey,
        });
        anchorRef.current = result.anchor;
        return result.selected;
      });
    },
    [visibleRows],
  );

  const handleExpand = useCallback((id: string) => {
    setExpanded((prev) => (prev === id ? null : id));
  }, []);

  const clearSelection = useCallback(() => {
    setSelected(new Set());
    anchorRef.current = null;
  }, []);

  const moveCursor = useCallback(
    (delta: number, extend: boolean) => {
      if (visibleRows.length === 0) return;

      const cursor = Math.max(0, visibleRows.findIndex(item => item.id === cursorId.current));
      const target = selected.size === 0 ? cursor : cursor + delta;
      const next = Math.min(Math.max(target, 0), visibleRows.length - 1);
      const row = visibleRows[next];
      if (!row) return;

      const id = row.id;
      cursorId.current = id;
      setSelected((prev) => {
        if (!extend) {
          anchorRef.current = id;
          return new Set([id]);
        }
        const merged = new Set(prev);
        merged.add(id);
        return merged;
      });

      listRef.current
        ?.querySelector(`[data-row-id="${CSS.escape(id)}"]`)
        ?.scrollIntoView({ block: "nearest" });
    },
    [visibleRows, selected],
  );

  const anyRunning = selectedItems.some(
    (item) => item.state === "downloading" || item.state === "waiting",
  );

  const bulkToggle = useCallback(() => {
    for (const item of selectedItems) {
      if (anyRunning) {
        if (item.state === "downloading" || item.state === "waiting") void actions.pause(item);
      } else if (item.state !== "finished") {
        void actions.resume(item);
      }
    }
  }, [selectedItems, anyRunning, actions]);

  const bulkRemove = useCallback(() => {
    actions.remove(selectedItems, deleteFilesOnRemove);
    clearSelection();
  }, [actions, selectedItems, clearSelection, deleteFilesOnRemove]);

  const handleContextMenu = useCallback((item: QueueItem, event: React.MouseEvent) => {
    setContextMenu({ x: event.clientX, y: event.clientY, item });
  }, []);

  const rowMenuItems = useCallback((item: QueueItem): ContextMenuItem[] => {
    const isDone = item.state === "finished";
    const isRunning = item.state === "downloading" || item.state === "waiting";
    const isPaused = item.state === "paused";
    const isFailed = item.state === "failed";
    const itemsList: ContextMenuItem[] = [
      {
        id: "open",
        label: "Open file",
        icon: <ExternalLink className="size-4" />,
        disabled: !isDone,
        bold: isDone,
        action: () => {
          flashFileOpen(document.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(item.id)}"] .download-row`), settings);
          if (actions.open) actions.open(item.id);
          else actions.reveal(item.id);
        },
      },
      {
        id: "reveal",
        label: "Show in folder",
        icon: <FolderOpen className="size-4" />,
        action: () => actions.reveal(item.id),
      },
    ];

    if (actions.convert && canConvert(item)) {
      itemsList.push({
        id: "convert",
        label: "Convert…",
        icon: <Transfer className="size-4" />,
        action: () => actions.convert?.([item]),
      });
    }

    // Only on a finished video that is not already an MP4 — the entry exists
    // to answer "this will not play here", and on an .mp4 there is nothing to
    // answer.
    if (actions.remux && canRemux(item, "mp4")) {
      itemsList.push({
        id: "remux",
        label: "Quick convert to MP4",
        icon: <Transfer className="size-4" />,
        action: () => actions.remux?.([item.id], "mp4"),
      });
    }

    if (actions.sendToDiscord) {
      itemsList.push({
        id: "discord",
        label: "Send to Discord",
        icon: <Discord className="size-4" />,
        disabled: !isDone,
        action: () => actions.sendToDiscord?.([item.id]),
      });
    }
    itemsList.push({ type: "separator" });

    if (isRunning) {
      itemsList.push({
        id: "pause",
        label: "Pause",
        icon: <Pause className="size-4" />,
        action: () => void actions.pause(item),
      });
    } else if (isPaused) {
      itemsList.push({
        id: "resume",
        label: "Resume",
        icon: <Play className="size-4" />,
        action: () => void actions.resume(item),
      });
    } else if (isFailed) {
      itemsList.push({
        id: "retry",
        label: "Retry download",
        icon: <RotateCcw className="size-4" />,
        action: () => void actions.resume(item),
      });
    }

    itemsList.push(
      { type: "separator" },
      {
        id: "copy-link",
        label: "Copy download link",
        icon: <Copy className="size-4" />,
        action: () => {
          void navigator.clipboard.writeText(item.url);
        },
      },
      {
        id: "copy-path",
        label: "Copy file path",
        icon: <Copy className="size-4" />,
        action: () => {
          void navigator.clipboard.writeText(item.path || item.filename);
        },
      },
      { type: "separator" },
      {
        id: "remove",
        label: "Remove from list",
        icon: <Trash className="size-4" />,
        danger: true,
        action: () => actions.remove([item], false),
      },
      {
        id: "delete-file",
        label: "Delete entry and file",
        icon: <Trash className="size-4" />,
        danger: true,
        action: () => actions.remove([item], true),
      },
    );

    return itemsList;
  }, [actions, settings]);

  const contextMenuItems: ContextMenuItem[] = useMemo(() => {
    if (!contextMenu) return [];
    const item = contextMenu.item;
    const hasMultiple = selectedItems.length > 1;

    if (hasMultiple && selected.has(item.id)) {
      return [
        {
          id: "bulk-toggle",
          label: anyRunning ? `Pause (${selectedItems.length})` : `Resume (${selectedItems.length})`,
          icon: anyRunning ? <Pause className="size-4" /> : <Play className="size-4" />,
          action: bulkToggle,
        },
        {
          id: "bulk-reveal",
          label: "Show in folder",
          icon: <FolderOpen className="size-4" />,
          action: () => actions.reveal(item.id),
        },
        ...(actions.sendToDiscord
          ? [
              {
                id: "bulk-discord",
                label: `Send to Discord (${selectedItems.filter((i) => i.state === "finished").length})`,
                icon: <Discord className="size-4" />,
                disabled: !selectedItems.some((i) => i.state === "finished"),
                action: () =>
                  actions.sendToDiscord?.(
                    selectedItems.map((i) => i.id),
                  ),
              } as ContextMenuItem,
            ]
          : []),
        ...(actions.convert && selectedItems.some(canConvert)
          ? [{
              id: "bulk-convert",
              label: `Convert… (${selectedItems.filter(canConvert).length})`,
              icon: <Transfer className="size-4" />,
              action: () => actions.convert?.(selectedItems.filter(canConvert)),
            } as ContextMenuItem]
          : []),
        ...(actions.remux && selectedItems.some((i) => canRemux(i, "mp4"))
          ? [
              {
                id: "bulk-remux",
                label: `Quick convert to MP4 (${selectedItems.filter((i) => canRemux(i, "mp4")).length})`,
                icon: <Transfer className="size-4" />,
                action: () =>
                  actions.remux?.(
                    selectedItems.filter((i) => canRemux(i, "mp4")).map((i) => i.id),
                    "mp4",
                  ),
              } as ContextMenuItem,
            ]
          : []),
        { type: "separator" },
        {
          id: "bulk-copy-links",
          label: `Copy download links (${selectedItems.length})`,
          icon: <Copy className="size-4" />,
          action: () => {
            const links = selectedItems.map((i) => i.url).join("\n");
            void navigator.clipboard.writeText(links);
          },
        },
        { type: "separator" },
        {
          id: "bulk-remove",
          label: `Remove from list (${selectedItems.length})`,
          icon: <Trash className="size-4" />,
          danger: true,
          action: () => { actions.remove(selectedItems, false); clearSelection(); },
        },
        {
          id: "bulk-delete-file",
          label: `Delete entries and files (${selectedItems.length})`,
          icon: <Trash className="size-4" />,
          danger: true,
          action: () => actions.remove(selectedItems, true),
        },
      ];
    }

    return rowMenuItems(item);
  }, [contextMenu, selected, selectedItems, anyRunning, actions, bulkToggle, clearSelection, rowMenuItems]);

  /**
   * The header menu.
   *
   * Right-clicking the legends asks about the table, not about a row, so this
   * is where the visible columns are chosen. The entries keep the menu open —
   * turning four columns on should be one trip rather than four.
   */
  const headerMenuItems: ContextMenuItem[] = useMemo(
    () => [
      { type: "heading", label: "Columns" },
      ...COLUMNS.map(
        (column): ContextMenuItem => ({
          id: `column-${column.id}`,
          label: column.menuLabel,
          checked: chosenColumns.has(column.id),
          keepOpen: true,
          action: () => toggleColumn(column.id),
        }),
      ),
      { type: "separator" },
      {
        id: "relative-dates",
        label: "Relative timestamps",
        checked: settings.telemetry.dateFormat === "relative",
        keepOpen: true,
        action: () => toggleTelemetry("dateFormat"),
      },
      { type: "separator" },
      {
        id: "reset-columns",
        label: "Reset columns",
        icon: <RotateCcw className="size-4" />,
        action: resetColumns,
      },
    ],
    [chosenColumns, settings.telemetry.dateFormat, toggleColumn, toggleTelemetry, resetColumns],
  );

  const openHeaderMenu = useCallback((event: React.MouseEvent) => {
    event.preventDefault();
    setContextMenu(null);
    setHeaderMenu({ x: event.clientX, y: event.clientY });
  }, []);

  // ---------------------------------------------------------------- keyboard
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!active || shortcutsBlocked(event)) return;

      // Escape works from inside a field too — it is the way out of one.
      if (event.key === "Escape") {
        if (composing) onComposingChange(false);
        else if (expanded) setExpanded(null);
        else if (selected.size > 0) clearSelection();
        return;
      }

      if (isTypingTarget(event.target)) return;
      // Native activation of a focused control takes precedence over row actions.
      if (event.target instanceof HTMLElement && event.target.closest("button,a,[role='switch']") && [" ", "Enter"].includes(event.key)) return;
      if (matchesShortcut(shortcuts, "newDownload", event)) {
        event.preventDefault();
        onComposingChange(true);
        setTimeout(() => composerRef.current?.focus(), 0);
        return;
      }

      if (matchesShortcut(shortcuts, "search", event)) {
        event.preventDefault();
        if (selected.size > 0) {
          clearSelection();
          setTimeout(() => searchRef.current?.focus(), 0);
        } else searchRef.current?.focus();
        return;
      }
      if (matchesShortcut(shortcuts, "selectAll", event)) {
        event.preventDefault();
        setSelected(new Set(visibleRows.map((item) => item.id)));
        return;
      }
      const down = matchesShortcut(shortcuts, "moveDown", event);
      const up = matchesShortcut(shortcuts, "moveUp", event);
      const extendDown = matchesShortcut(shortcuts, "extendDown", event);
      const extendUp = matchesShortcut(shortcuts, "extendUp", event);
      if (down || up || extendDown || extendUp) {
        event.preventDefault();
        moveCursor(down || extendDown ? 1 : -1, extendDown || extendUp);
        return;
      }
      if (selected.size === 0) return;

      if (matchesShortcut(shortcuts, "togglePause", event)) {
        event.preventDefault();
        bulkToggle();
      } else if (matchesShortcut(shortcuts, "openDownload", event)) {
        event.preventDefault();
        for (const item of selectedItems) {
          flashFileOpen(document.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(item.id)}"] .download-row`), settings);
          if (item.state === "finished" && actions.open) actions.open(item.id);
          else actions.reveal(item.id);
        }
      } else if (matchesShortcut(shortcuts, "remove", event)) {
        event.preventDefault();
        bulkRemove();
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    active,
    shortcuts,
    settings,
    composing,
    expanded,
    selected,
    selectedItems,
    actions,
    visibleRows,
    moveCursor,
    bulkToggle,
    bulkRemove,
    clearSelection,
    onComposingChange,
  ]);

  // ------------------------------------------------------------- drag & drop
  const handleDrop = useCallback(
    async (event: React.DragEvent) => {
      event.preventDefault();
      dragDepth.current = 0;
      setDragging(false);
      const urls = await urlsFromDataTransfer(event.dataTransfer);
      if (urls.length === 0) {
        onInvalidIntake?.("No download link found. Drop a web link, web image, or a text file containing http(s) URLs.");
        return;
      }
      // A drop is the request: queue it now, pausing only for links that
      // would save a web page instead of a file.
      // A single link for another tab is offered there by onAdd; checking it
      // for HTML first would ask about the very page that tab handles.
      const [only] = urls;
      const accepted = urls.length === 1 && only && classifyLink(only)
        ? urls
        : await screenDroppedLinks(urls, url => engine.inspectLink(url), message => window.confirm(message));
      if (accepted.length === 0) return;
      let failed = accepted;
      try {
        const result = await onAdd(accepted);
        failed = typeof result === "boolean" ? (result ? [] : accepted) : result.failed.map(failure => failure.url);
      } catch { /* Every link stays in the entry below so it can be corrected. */ }
      if (failed.length > 0) {
        setDropPrefill({ url: failed.join("\n"), headers: [], nonce: Date.now() });
        onComposingChange(true);
      }
    },
    [onAdd, onComposingChange, onInvalidIntake],
  );

  const countLabel =
    filtered.length === allItems.length
      ? `${allItems.length} in the register`
      : `${filtered.length} of ${allItems.length}`;

  return (
    <div
      className="relative flex h-full w-full flex-col"
      onDragEnter={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
        dragDepth.current += 1;
        setDragging(true);
      }}
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }}
      onDragLeave={() => {
        // Depth counting, because dragleave also fires when crossing between
        // child elements inside the drop zone.
        dragDepth.current -= 1;
        if (dragDepth.current <= 0) setDragging(false);
      }}
      onDrop={(e) => void handleDrop(e)}
    >
      {dragging && (
        <div className="pointer-events-none absolute inset-2 z-30 flex items-center justify-center border-2 border-dashed border-route bg-panel/85">
          <p className="legend text-[12px] text-route">Drop links or web images to download</p>
        </div>
      )}

      <TheLine
        items={allItems}
        onShowActive={() => {
          // Whatever was narrowing the register is what hid them.
          setQuery("");
          setFilter("all");
          listRef.current?.scrollTo({ top: 0 });
        }}
      />

      {error && (
        <div className="mx-6 mb-3 flex shrink-0 items-start gap-3 border border-danger/45 bg-danger/10 px-3 py-2.5">
          <FaultSlug>fault</FaultSlug>
          <p data-selectable className="flex-1 text-[12px] leading-relaxed text-ink">
            {error}
          </p>
          <button
            type="button"
            onClick={onClearError}
            className="focus-ring legend shrink-0 text-[11px] text-ink-dim transition-colors hover:text-ink"
          >
            Dismiss
          </button>
        </div>
      )}

      {/* THE REGISTER ------------------------------------------------------ */}
      <section
        aria-label="The register"
        className="flex min-h-0 flex-1 flex-col border-t border-edge-strong bg-panel"
      >
        {/* The selection bar replaces the filter row rather than stacking on
            top of it: while a selection is live, acting on it is the only
            thing the header needs to offer. */}
        {selected.size > 0 ? (
          <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-edge bg-route/12 px-4 py-2">
            <Legend className="mr-1 text-ink">{selected.size} selected</Legend>
            <TooltipGroup side="bottom" className="items-center gap-1.5">
              <BulkButton
                onClick={bulkToggle}
                icon={anyRunning ? <Pause className="size-3" /> : <Play className="size-3" />}
                tooltip={anyRunning ? "Pause selected transfers" : "Resume selected transfers"}
              >
                {anyRunning ? "Pause" : "Resume"}
              </BulkButton>
              <BulkButton
                onClick={bulkRemove}
                icon={<Trash className="size-3" />}
                tone="danger"
                tooltip={deleteFilesOnRemove ? "Remove selected entries and delete their local files after Undo expires" : "Remove selected entries; keep local files on disk"}
              >
                Remove
              </BulkButton>
              {selectedItems.length === 1 && selectedItems[0]?.state === "finished" && (
                <BulkButton
                  onClick={() => selectedItems[0] && actions.reveal(selectedItems[0].id)}
                  icon={<FolderOpen className="size-3" />}
                  tooltip="Reveal downloaded file in system explorer"
                >
                  Show in folder
                </BulkButton>
              )}
            </TooltipGroup>
            <TooltipGroup side="bottom" className="ml-auto">
              <button
                type="button"
                onClick={clearSelection}
                data-tooltip="Clear selection (Esc)"
                className="t-tt-trigger focus-ring legend text-[11px] text-ink-dim transition-colors hover:text-ink"
              >
                Clear
              </button>
            </TooltipGroup>
          </div>
        ) : (
          <div className="flex shrink-0 flex-col gap-2 border-b border-edge px-4 py-2 lg:flex-row lg:items-center lg:justify-between">
            <Legend className="text-ink-dim">{countLabel}</Legend>

            <div className="flex flex-1 flex-wrap items-center gap-2 lg:justify-end">
              <div className="relative min-w-0 flex-1 lg:max-w-[230px]">
                <Search className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-ink-faint" />
                <input
                  ref={searchRef}
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder={`Search filename or URL${shortcuts.enabled && shortcuts.actions.search.enabled ? `   ${shortcutLabel(shortcuts, "search")}` : ""}`}
                  aria-label="Search downloads" autoComplete="off" spellCheck={false}
                  className="focus-ring recess w-full rounded-hw py-1 pl-7 pr-2 text-[12px] text-ink outline-none placeholder:text-ink-faint"
                />
              </div>

              <SlideGroup
                role="group"
                aria-label="Filter by state"
                className="flex flex-wrap items-center overflow-hidden rounded-hw border border-edge-strong"
              >
                {FILTERS.map((f) => (
                  <button
                    key={f.value}
                    type="button"
                    onClick={() => setFilter(f.value)}
                    aria-pressed={filter === f.value}
                    className={`focus-ring legend border-r border-edge px-2.5 py-1 text-[11px] transition-colors last:border-r-0 ${
                      filter === f.value
                        ? "text-on-signal"
                        : "text-ink-dim hover:bg-face hover:text-ink"
                    }`}
                  >
                    {f.label}
                  </button>
                ))}
              </SlideGroup>
            </div>
          </div>
        )}

        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-edge px-4 py-2 text-[11px] text-ink-dim">
          <div className="flex items-center gap-1.5"><span>Sort</span>
            <SelectInput size="sm" ariaLabel="Sort downloads" value={view.sort} onChange={sort => setView(prev => ({ ...prev, sort }))} options={[
              { value: "date", label: "Date added" }, { value: "name", label: "Name" }, { value: "size", label: "Size" }, { value: "speed", label: "Speed" }, { value: "progress", label: "Progress" },
            ]} />
          </div>
          <button type="button" aria-label="Reverse sort direction" className="focus-ring rounded-hw border border-edge-strong px-2 py-1 text-ink" onClick={() => setView(prev => ({ ...prev, direction: prev.direction === "asc" ? "desc" : "asc" }))}>{view.direction === "asc" ? "Ascending" : "Descending"}</button>
          <div className="flex items-center gap-1.5"><span>Group by</span>
            <SelectInput size="sm" ariaLabel="Group downloads" className="max-w-44" value={view.groupBy}
              onChange={groupBy => { setCollapsedGroups(new Set()); setContextMenu(null); setView(prev => ({ ...prev, groupBy })); }} options={[
              { value: "none", label: "None" }, { value: "type", label: "File type" }, { value: "status", label: "Status" }, { value: "source", label: "Source website" }, { value: "folder", label: "Destination folder" },
            ]} />
          </div>
          {view.groupBy !== "none" && groups.length > 0 && <>
            <button type="button" className="focus-ring rounded-hw border border-edge-strong px-2 py-1 text-ink disabled:opacity-40" disabled={visibleRows.length === filtered.length} onClick={() => setCollapsedGroups(new Set())}>Expand all groups</button>
            <button type="button" className="focus-ring rounded-hw border border-edge-strong px-2 py-1 text-ink disabled:opacity-40" disabled={visibleRows.length === 0} onClick={() => { setContextMenu(null); setCollapsedGroups(new Set(groups.map(group => group.key))); }}>Collapse all groups</button>
            <span className="font-mono text-[11px]">{groups.length} {groups.length === 1 ? "group" : "groups"} · {visibleRows.length} shown</span>
          </>}
          <div className="flex items-center gap-1.5"><span>Rows</span>
            <SelectInput size="sm" ariaLabel="Download row density" value={view.density} onChange={density => setView(prev => ({ ...prev, density }))} options={[{ value: "compact", label: "Compact" }, { value: "comfortable", label: "Comfortable" }]} />
          </div>
          <details className="ml-auto min-w-0"><summary className="focus-ring cursor-pointer rounded-hw border border-edge-strong px-2 py-1 text-ink">Arrange columns</summary>
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <SelectInput size="sm" ariaLabel="Column to move" value={(visibleColumns.some(column => column.id === arrangeColumn) ? arrangeColumn : visibleColumns[0]?.id ?? "") as ColumnId} className="max-w-48" onChange={value => setArrangeColumn(value)} options={visibleColumns.map(column => ({ value: column.id, label: column.menuLabel }))} />
              {([-1, 1] as const).map(offset => {
                const id = visibleColumns.some(column => column.id === arrangeColumn) ? arrangeColumn : visibleColumns[0]?.id;
                const index = visibleColumns.findIndex(column => column.id === id);
                return <button key={offset} type="button" disabled={index < 0 || index + offset < 0 || index + offset >= visibleColumns.length} className="focus-ring rounded-hw border border-edge-strong px-2 py-1 text-ink disabled:opacity-40" onClick={() => id && setView(prev => ({ ...prev, columnOrder: [...moveColumn(visibleColumns.map(column => column.id), id, offset), ...prev.columnOrder.filter(column => !chosenColumns.has(column))] }))}>{offset < 0 ? "Move left" : "Move right"}</button>;
              })}
              <button type="button" className="focus-ring rounded-hw border border-edge-strong px-2 py-1 text-ink" onClick={event => { const rect = event.currentTarget.getBoundingClientRect(); setHeaderMenu({ x: rect.left, y: rect.bottom }); }}>Choose visible columns</button>
              <button type="button" className="focus-ring rounded-hw border border-edge-strong px-2 py-1 text-ink" onClick={() => { setCollapsedGroups(new Set()); setView({ ...DEFAULT_VIEW }); resetColumns(); }}>Reset view</button>
            </div>
          </details>
        </div>

        {/* Column legends, cut into the panel above the record. Right-clicking
            them is how the table is configured — the legends are the only part
            of the register that is about the columns rather than the rows. */}
        {filtered.length > 0 && (
          <div
            onContextMenu={openHeaderMenu}
            title="Right-click to choose columns"
            className="flex shrink-0 cursor-context-menu select-none items-center gap-3 border-b border-edge bg-face px-4 py-1"
          >
            <span className="w-[15px] shrink-0" />
            <span className="w-[9px] shrink-0" />
            <Legend className="flex-[3]">description</Legend>
            {visibleColumns.map((column) => (
              <Legend
                key={column.id}
                className={`shrink-0 truncate ${column.show} ${column.width} ${
                  column.align === "right" ? "text-right" : ""
                }`}
              >
                {column.label}
              </Legend>
            ))}
            <span className={`shrink-0 ${actions.sendToDiscord ? "w-[122px]" : "w-[92px]"}`} />
          </div>
        )}

        {/* The record owns the remaining height. There is no row cap — the old
            dashboard sliced to five and offered no way to reach the rest. */}
        <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto">
          <TooltipGroup side="top" className="block min-w-full">
            {filtered.length === 0 ? (
              allItems.length === 0 ? (
                <EmptyRegister detached={detached} />
              ) : (
                <div className="px-4 py-16 text-center">
                  <p className="text-[13px] text-ink-dim">Nothing in the register matches.</p>
                  <button
                    type="button"
                    onClick={() => {
                      setQuery("");
                      setFilter("all");
                    }}
                    className="focus-ring legend mt-3 text-[11px] text-route transition-colors hover:text-ink"
                  >
                    Clear filters
                  </button>
                </div>
              )
            ) : (
              groups.map(group => <section key={group.key} aria-label={view.groupBy === "none" ? undefined : `${group.label} downloads`}>
                {view.groupBy !== "none" && <h3>
                  <button type="button" aria-expanded={!collapsedGroups.has(group.key)}
                    aria-controls={`download-group-${encodeURIComponent(group.key)}`}
                    onClick={() => toggleGroup(group.key)}
                    className="focus-ring flex w-full min-w-0 items-center gap-2 border-b border-edge bg-recess px-4 py-2 text-left text-[11px] text-ink hover:bg-face">
                    <ChevronDown className={`size-3.5 shrink-0 ${collapsedGroups.has(group.key) ? "-rotate-90" : ""}`} />
                    <span className="min-w-0 truncate font-medium" title={group.label}>{group.label}</span>
                    <span className="shrink-0 font-mono text-[11px] text-ink-dim">{group.items.length} {group.items.length === 1 ? "entry" : "entries"}</span>
                  </button>
                </h3>}
                <div id={`download-group-${encodeURIComponent(group.key)}`} hidden={collapsedGroups.has(group.key)}>
              {!collapsedGroups.has(group.key) && group.items.map((item) => (
                <DownloadRow
                  key={item.id}
                  item={item}
                  fileStatus={fileStatuses[item.id]?.status}
                  selected={selected.has(item.id)}
                  expanded={expanded === item.id}
                  dimmed={expanded !== null && expanded !== item.id}
                  onSelect={handleSelect}
                  onExpand={handleExpand}
                  onToggle={actions.toggle}
                  onRemove={handleRemove}
                  onRemoveEntry={handleRemoveEntry}
                  onRetry={actions.resume}
                  onRecover={actions.recover}
                  columns={visibleColumns}
                  density={view.density}
                  deleteFilesOnRemove={deleteFilesOnRemove}
                  onReveal={actions.reveal}
                  onOpen={actions.open}
                  onSendToDiscord={actions.sendToDiscord}
                  detailActions={expanded === item.id ? rowMenuItems(item) : undefined}
                  onDragOut={actions.dragOut}
                  onContextMenu={handleContextMenu}
                />
              ))}
                </div>
              </section>)
            )}
          </TooltipGroup>
        </div>
      </section>

      {/* THE ENTRY --------------------------------------------------------- */}
      <AddDownload
        open={composing}
        onOpenChange={onComposingChange}
        onSubmit={onAdd}
        destination={destination}
        onWorkingDirectoryChange={onWorkingDirectoryChange}
        disabled={detached}
        prefill={dropPrefill ?? prefill}
        existingDownloads={allItems}
        inputRef={composerRef}
      />

      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          items={contextMenuItems}
          onClose={() => setContextMenu(null)}
        />
      )}

      {headerMenu && (
        <ContextMenu
          x={headerMenu.x}
          y={headerMenu.y}
          items={headerMenuItems}
          onClose={() => setHeaderMenu(null)}
        />
      )}
    </div>
  );
});

/**
 * Nothing is on the railway yet.
 *
 * This is an operate surface, so the empty state says what to do next. The
 * previous build ran a marketing headline here — "Every byte, in parallel." —
 * which is a line for a landing page, not for the one moment a user is looking
 * for an instruction.
 */
function EmptyRegister({ detached }: { detached: boolean }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 px-6 py-16 text-center">
      <span className="h-px w-24 bg-edge" />
      <Legend className="text-ink-dim">{detached ? "engine detached" : "line clear"}</Legend>
      <p className="max-w-sm text-[13px] leading-relaxed text-ink-dim">
        {detached
          ? "This view is running in a browser. Launch the desktop app to reach the download engine."
          : "Press Ctrl+V anywhere, or drop links and web images here. Each file uses as many connections as the server allows."}
      </p>
    </div>
  );
}
