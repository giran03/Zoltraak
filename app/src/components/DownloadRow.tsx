import { FailureActions } from "./FailureActions";
import { FileAvailability } from "./FileAvailability";
import type { RecoveryKind } from "../lib/downloadUx";
import { memo, useEffect, useRef, useState } from "react";
import { aspectOf } from "../lib/aspect";
import {
  COLUMNS,
  fractionOf,
  hostOf,
  kindOf,
  remainingOf,
  typeOf,
  type ColumnId,
  type ColumnSpec,
} from "../lib/columns";
import { formatBytes, formatDate, formatEta, formatRate, formatRelativeDate } from "../lib/format";
import { occupancyOf } from "../lib/occupancy";
import type { QueueItem } from "../lib/snapshots";
import { engine, hasEngine, type OutputStatus } from "../lib/engine";
import { useAppearance } from "../lib/useAppearance";
import { flashFileOpen } from "../lib/fileOpenFeedback";
import { useShortcuts } from "../lib/useShortcuts";
import { matchesShortcut } from "../lib/shortcuts";
import type { ContextMenuItem } from "./ContextMenu";
import { Check, ChevronDown, Discord, Film, FolderOpen, Note, Pause, Play, Trash } from "./Icons";
import { FaultSlug, Lamp, Legend } from "./Panel";

/**
 * Row control.
 *
 * Visible at rest, not on hover. Pause is a primary action in a download
 * manager; hiding it until the pointer arrives makes it undiscoverable, leaves
 * it unreachable on touch, and gives a keyboard user nothing to aim at.
 */
function parseDuration(val: string, fallback = 250): number {
  const raw = val.trim();
  if (!raw) return fallback;
  const num = parseFloat(raw);
  if (isNaN(num)) return fallback;
  return raw.endsWith("ms") ? num : raw.endsWith("s") ? num * 1000 : num;
}

function RowAction({
  label,
  kind,
  tooltip,
  onClick,
  ariaExpanded,
  className,
  disabled,
  children,
}: {
  label: string;
  kind: "expand" | "share" | "folder" | "cancel" | "play" | "remove";
  tooltip?: string;
  onClick: () => void;
  ariaExpanded?: boolean;
  className?: string;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      aria-label={label}
      data-action={kind}
      aria-expanded={ariaExpanded}
      data-tooltip={tooltip ?? label}
      onClick={onClick}
      className={`t-tt-trigger entry-icon-button download-entry-action focus-ring ${className ?? ""}`}
    >
      <span className="entry-icon-glyph">{children}</span>
    </button>
  );
}

/** Checkbox cut to match the panel's engraved hardware. */
function SelectBox({
  checked,
  label,
  onToggle,
}: {
  checked: boolean;
  label: string;
  onToggle: (event: React.MouseEvent) => void;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      aria-label={label}
      onClick={onToggle}
      className={`focus-ring flex size-[15px] shrink-0 items-center justify-center rounded-hw border transition-colors duration-150 ${
        checked
          ? "border-route bg-route text-on-signal"
          : "border-edge-strong bg-recess text-transparent hover:border-ink-faint"
      }`}
    >
      <Check className="size-2.5" />
    </button>
  );
}

/**
 * The row's own length of track, at register scale.
 *
 * Reads the same `occupancyOf` model THE LINE uses, so a transfer is never
 * drawn as sixteen ranges here and twenty-four two inches above.
 */
function Occupancy({ item }: { item: QueueItem }) {
  const aspect = aspectOf(item.state);
  const occupancy = occupancyOf(item);

  if (occupancy.kind === "stream") {
    return (
      <span className="flex h-[9px] w-full" aria-hidden="true">
        <span className="block relative min-w-0 flex-1 overflow-hidden rounded-hw-sm bg-block-clear">
          <span
            className={`absolute inset-y-0 left-0 ${aspect.block} ${
              occupancy.fraction === null ? "opacity-40" : ""
            }`}
            style={{ width: `${(occupancy.fraction ?? 1) * 100}%` }}
          />
        </span>
      </span>
    );
  }

  return (
    <span className="flex h-[9px] w-full gap-[2px]" aria-hidden="true">
      {Array.from({ length: occupancy.blocks }, (_, index) => (
        <span
          key={index}
          className={`block min-w-0 flex-1 rounded-hw-sm ${
            index < occupancy.lit ? aspect.block : "bg-block-clear"
          }`}
        />
      ))}
    </span>
  );
}

/**
 * One register cell.
 *
 * Every column is drawn through here so a cell can never disagree with its own
 * legend about how wide it is or which side its figures sit on — the header
 * reads the same `ColumnSpec` this does.
 *
 * A column the user has turned on is drawn even when it has nothing to say:
 * an em dash is an answer, and a cell that vanishes on some rows would put the
 * columns to its right on a different footing row by row.
 */
function Cell({
  column,
  item,
  dateFormat,
  speedUnit,
  fileStatus,
}: {
  column: ColumnSpec;
  item: QueueItem;
  dateFormat: "relative" | "absolute";
  speedUnit: "MBps" | "Mbps";
  fileStatus?: OutputStatus["status"];
}) {
  const aspect = aspectOf(item.state);
  const total = item.totalBytes && item.totalBytes > 0 ? item.totalBytes : null;
  const isMissing = item.state === "finished" && (fileStatus === "missing" || fileStatus === "inaccessible");

  const content = (() => {
    switch (column.id) {
      case "aspect":
        if (isMissing) {
          return (
            <FaultSlug>
              {fileStatus === "inaccessible" ? "UNAVAILABLE" : "MISSING"}
            </FaultSlug>
          );
        }
        return aspect.fault ? (
          <FaultSlug>{aspect.label}</FaultSlug>
        ) : (
          <Legend className={aspect.ink}>{aspect.label}</Legend>
        );

      case "kind":
        return <Legend className="text-ink-faint">{kindOf(item)}</Legend>;

      case "type":
        return <span className="describer text-[11px] text-ink-faint">{typeOf(item)}</span>;

      case "occupancy":
        return <Occupancy item={item} />;

      case "progress": {
        const fraction = fractionOf(item);
        return (
          <span data-measure className="describer text-[11px] text-ink-dim">
            {fraction === null ? "—" : `${Math.floor(fraction * 100)}%`}
          </span>
        );
      }

      case "size":
        return (
          <span data-measure className="describer text-[11px] text-ink-dim">
            {formatBytes(item.downloadedBytes)}
            {total ? ` / ${formatBytes(total)}` : ""}
          </span>
        );

      case "remaining": {
        const remaining = remainingOf(item);
        return (
          <span data-measure className="describer text-[11px] text-ink-dim">
            {remaining === null ? "—" : formatBytes(remaining)}
          </span>
        );
      }

      case "rate":
        return (
          <span
            data-measure
            className={`describer text-[11px] ${
              item.state === "downloading" ? aspect.ink : "text-ink-faint"
            }`}
          >
            {formatRate(item.bytesPerSecond, speedUnit)}
          </span>
        );

      case "eta":
        return (
          <span data-measure className="describer text-[11px] text-ink-faint">
            {formatEta(item.etaSeconds)}
          </span>
        );

      case "connections":
        return (
          <span
            data-measure
            className="t-tt-trigger describer text-[11px] text-ink-faint"
            data-tooltip={`${item.connections || 1} connections`}
          >
            {item.state === "finished" ? "—" : `${item.connections || 1}c`}
          </span>
        );

      case "added":
        return (
          <span
            data-measure
            className="t-tt-trigger describer text-[11px] text-ink-faint"
            data-tooltip={formatDate(item.createdAt)}
          >
            {dateFormat === "absolute"
              ? formatDate(item.createdAt)
              : formatRelativeDate(item.createdAt)}
          </span>
        );

      case "priority":
        return (
          <span data-measure className="describer text-[11px] text-ink-faint">
            {item.priority}
          </span>
        );

      case "host":
        return (
          <span
            className="t-tt-trigger describer text-[11px] text-ink-faint"
            data-tooltip={item.url}
          >
            {hostOf(item.url)}
          </span>
        );

      case "folder":
        return (
          <span
            className="t-tt-trigger describer inline-block max-w-full truncate text-[11px] text-ink-faint"
            data-tooltip={item.directory || "default folder"}
          >
            {item.directory || "default folder"}
          </span>
        );

      case "source":
        return (
          <span
            className="t-tt-trigger describer inline-block max-w-full truncate text-[11px] text-ink-faint"
            data-tooltip={item.url}
          >
            {item.url}
          </span>
        );
    }
  })();

  return (
    <span
      className={`shrink-0 truncate ${column.show} ${column.width} ${
        column.align === "right" ? "text-right" : ""
      }`}
    >
      {content}
    </span>
  );
}

/**
 * The cutaway.
 *
 * Opening a row slices it open in place rather than sending the user to a
 * detail pane. It shows exact figures, not the rounded ones the row carries —
 * the point of opening a black box is that what is inside is more precise than
 * what is outside.
 *
 * Per-connection rates are deliberately absent: the engine reports an
 * aggregate rate and a connection count, so dividing one by the other would be
 * a fabricated reading on the one screen whose whole promise is that its
 * numbers are real.
 */
export function DownloadDetails({ item, dateFormat, speedUnit, onRemoveEntry }: { item: QueueItem; dateFormat?: "relative" | "absolute"; speedUnit?: "MBps" | "Mbps"; onRemoveEntry?: () => void }) {
  const total = item.totalBytes && item.totalBytes > 0 ? item.totalBytes : null;
  const remaining = total ? Math.max(total - item.downloadedBytes, 0) : null;
  const source = item.thumbnail || (!item.isVideo && !item.isMusic && /\.(?:avif|gif|jpe?g|png|webp)$/i.test(item.filename) ? item.url : null);
  const [brokenSource, setBrokenSource] = useState<string | null>(null);
  const [localPreview, setLocalPreview] = useState<{ key: string; source: string } | null>(null);
  const previewKey = `${item.id}:${item.path}:${item.state}`;
  const preferred = source && source !== brokenSource && /^https?:\/\//i.test(source) ? source : null;
  const preview = preferred || (localPreview?.key === previewKey && localPreview.source !== brokenSource ? localPreview.source : null);
  useEffect(() => {
    if (preferred || item.state !== "finished" || !hasEngine()) return;
    // Completed local media also covers ordinary queue downloads and older
    // video jobs whose extractor never supplied a poster.
    if (!item.isVideo && !item.isMusic && !/\.(?:mp4|m4v|mkv|webm|mov|avi|wmv|asf|flv|f4v|3gp|3g2|ogv|mpg|mpeg|ts|m2ts|mts|vob|rm|rmvb|mxf|nut|m2v|h264|h265|hevc|mjpeg|mp3|m4a|flac|ogg|opus|aac|wav|avif|gif|jpe?g|png|webp|bmp|tiff?)$/i.test(item.path || item.filename)) return;
    let active = true;
    void engine.downloadThumbnail(item.id).then(source => {
      if (active && source) setLocalPreview({ key: previewKey, source });
    }).catch(() => {}); // No usable frame or media tools: keep full-width details.
    return () => { active = false; };
  }, [preferred, previewKey, item.id, item.isVideo, item.isMusic, item.path, item.filename]);

  return (
    <div className="@container border-t border-edge bg-recess px-4 py-3.5">
      <Occupancy item={item} />

      <div data-download-details className={preview ? "mt-3.5 grid items-start gap-4 @min-[640px]:grid-cols-[minmax(0,16rem)_minmax(0,1fr)] @min-[900px]:gap-6" : "mt-3.5"}>
      {preview && <DownloadThumbnail key={preview} item={item} source={preview} onError={() => setBrokenSource(preview)} />}
      <div className="min-w-0">

      <dl className="grid grid-cols-2 gap-x-8 gap-y-2 @min-[900px]:grid-cols-4">
        <Field label="downloaded">{item.downloadedBytes.toLocaleString()} B</Field>
        <Field label={item.isVideo ? "nominal (estimated)" : "nominal"}>
          {total
            ? item.isVideo
              ? `~${formatBytes(total)}`
              : `${total.toLocaleString()} B`
            : "not reported"}
        </Field>
        <Field label="remaining">
          {remaining === null
            ? "unknown"
            : item.isVideo
              ? `~${formatBytes(remaining)}`
              : `${remaining.toLocaleString()} B`}
        </Field>
        <Field label="connections">
          {item.isVideo ? "single stream" : item.connections}
        </Field>
        <Field label="rate">{formatRate(item.bytesPerSecond, speedUnit)}</Field>
        <Field label="due">{formatEta(item.etaSeconds)}</Field>
        <Field label="priority">{item.priority}</Field>
        <Field label="added">
          {dateFormat === "absolute" ? formatDate(item.createdAt) : formatRelativeDate(item.createdAt)}
        </Field>
      </dl>

      <dl className="mt-3 space-y-2 border-t border-edge pt-3">
        <Field label="quality" wide>{item.quality || (item.isVideo || item.isMusic ? "Not reported" : "Original file")}</Field>
        {(item.isVideo || item.isMusic) && <Field label="audio" wide>{item.validation?.hasAudio === true ? "Audio track confirmed" : item.validation?.hasAudio === false ? "No audio track" : "Not verified"}</Field>}
        {item.validation && <Field label="validation" wide>{item.validation.status}{item.validation.samplesDecoded ? " · Samples decoded" : ""}</Field>}
        {item.stage && <Field label="stage" wide>{item.stage}</Field>}
        <Field label="source" wide>
          <span data-selectable className="break-all">
            {item.url}
          </span>
        </Field>
        <Field label="destination" wide>
          <span data-selectable className="break-all">
            {item.path || item.directory || "default folder"}
          </span>
        </Field>
        {item.error && (
          <Field label="fault" wide tone="text-danger">
            <span data-selectable className="break-all">
              {item.error}
            </span>
          </Field>
        )}
      </dl>
      {item.state === "finished" && <FileAvailability id={item.id} onRemoveEntry={onRemoveEntry} />}
      </div>
      </div>
    </div>
  );
}

function DownloadThumbnail({ item, source, onError }: { item: QueueItem; source: string; onError: () => void }) {
  const [dimensions, setDimensions] = useState<{ width: number; height: number } | null>(null);
  return (
    <figure className="min-w-0">
      <img
        src={source}
        alt={`${item.isMusic ? "Cover art" : "Preview"} for ${item.filename}`}
        loading="lazy"
        decoding="async"
        referrerPolicy="no-referrer"
        onError={onError}
        onLoad={event => {
          const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
          if (width && height) setDimensions({ width, height });
        }}
        width={dimensions?.width ?? 256}
        height={dimensions?.height ?? 144}
        style={dimensions ? { aspectRatio: `${dimensions.width} / ${dimensions.height}` } : undefined}
        className="block h-auto w-full max-w-full rounded-hw bg-block-clear"
      />
    </figure>
  );
}

export function DownloadDisclosure({ item, onRemoveEntry }: { item: QueueItem; onRemoveEntry?: () => void }) {
  const [open, setOpen] = useState(false);
  return <details className="basis-full min-w-0" onToggle={e => setOpen(e.currentTarget.open)}>
    <summary className="focus-ring cursor-pointer py-1 text-[12px] text-ink-dim">Download details · {item.filename}</summary>
    {open && <DownloadDetails item={item} onRemoveEntry={onRemoveEntry} />}
  </details>;
}

function Field({
  label,
  children,
  wide,
  tone = "text-ink-dim",
}: {
  label: string;
  children: React.ReactNode;
  wide?: boolean;
  tone?: string;
}) {
  return (
    <div className={wide ? "flex gap-3" : "flex flex-col gap-0.5"}>
      <dt className={wide ? "w-24 shrink-0 pt-px" : undefined}>
        <Legend>{label}</Legend>
      </dt>
      <dd data-measure className={`describer min-w-0 text-[11px] ${tone}`}>
        {children}
      </dd>
    </div>
  );
}

export const DownloadRow = memo(
  function DownloadRow({
    item,
    selected,
    expanded,
    dimmed,
    onSelect,
    onExpand,
    onToggle,
    onRemove,
    onRemoveEntry,
    deleteFilesOnRemove = true,
    onReveal,
    onOpen,
    onSendToDiscord,
    detailActions,
    onDragOut,
    onContextMenu,
    onRetry,
    onRecover,
    columns,
    density = "compact",
    fileStatus,
  }: {
    item: QueueItem;
    selected: boolean;
    expanded: boolean;
    dimmed: boolean;
    /** `event` carries shift/meta so the list can extend or add to a range. */
    onSelect: (id: string, event: React.MouseEvent) => void;
    onExpand: (id: string) => void;
    onToggle: (item: QueueItem) => void;
    onRemove: (item: QueueItem) => void;
    onRemoveEntry?: (item: QueueItem) => void;
    deleteFilesOnRemove?: boolean;
    onReveal: (id: string) => void;
    onOpen?: (id: string) => void;
    onSendToDiscord?: (ids: string[]) => void;
    detailActions?: ContextMenuItem[];
    onDragOut?: (item: QueueItem) => void;
    onContextMenu?: (item: QueueItem, event: React.MouseEvent) => void;
    onRetry?: (item: QueueItem) => Promise<unknown>;
    onRecover?: (item: QueueItem, kind: RecoveryKind) => Promise<void>;
    columns?: ColumnSpec[];
    density?: "compact" | "comfortable";
    fileStatus?: OutputStatus["status"];
  }) {
    const { settings } = useAppearance();
    const shortcuts = useShortcuts();
    const { telemetry } = settings;
    const aspect = aspectOf(item.state);
    const running = item.state === "downloading" || item.state === "waiting";
    const done = item.state === "finished";

    const [fetchedStatus, setFetchedStatus] = useState<OutputStatus["status"] | undefined>(fileStatus);

    useEffect(() => {
      if (fileStatus !== undefined) {
        setFetchedStatus(fileStatus);
        return;
      }
      if (done && hasEngine()) {
        let active = true;
        void engine.fileStatus(item.id).then((res) => {
          if (active && res) setFetchedStatus(res.status);
        }).catch(() => {});
        return () => { active = false; };
      }
    }, [fileStatus, done, item.id]);

    const activeStatus = fileStatus ?? fetchedStatus;
    const isMissing = done && (activeStatus === "missing" || activeStatus === "inaccessible");

    // Registry order, not the stored order — see `lib/columns`.
    const chosen = new Set<ColumnId>(settings.columns);
    const visible = columns ?? COLUMNS.filter((column) => chosen.has(column.id));

    const [wasExpanded, setWasExpanded] = useState(expanded);
    const [isClosing, setIsClosing] = useState(false);

    if (expanded !== wasExpanded) {
      setWasExpanded(expanded);
      if (!expanded && wasExpanded) {
        setIsClosing(true);
      }
    }

    useEffect(() => {
      if (isClosing) {
        const rawDur =
          typeof window !== "undefined"
            ? getComputedStyle(document.documentElement).getPropertyValue("--acc-collapse")
            : "";
        const dur = parseDuration(rawDur, 250);
        const timer = setTimeout(() => {
          setIsClosing(false);
        }, dur);
        return () => clearTimeout(timer);
      }
    }, [isClosing]);

    const showPanel = expanded || isClosing;
    const lastClick = useRef<{ time: number; x: number; y: number; interval: number } | null>(null);

    const openEntry = (event: React.MouseEvent<HTMLDivElement>) => {
      if ((event.target as HTMLElement).closest("button")) return;
      event.preventDefault();
      window.getSelection()?.removeAllRanges();
      flashFileOpen(event.currentTarget, settings);
      if (done && onOpen) {
        onOpen(item.id);
      } else {
        onReveal(item.id);
      }
    };

    return (
      <div
        data-row-id={item.id}
        tabIndex={-1}
        data-open={expanded ? "true" : "false"}
        // Keep rows searchable and keyboard-accessible while the browser skips
        // layout and paint for offscreen history. Expanded details size normally.
        style={showPanel ? undefined : { contentVisibility: "auto", containIntrinsicSize: "auto 52px" }}
        className={`t-acc border-b border-edge transition-[background-color,opacity] duration-150 last:border-b-0 ${
          expanded ? "relative z-10 bg-face lift" : selected ? "bg-route/12" : "bg-transparent"
        } ${dimmed && item.state !== "failed" ? "opacity-45" : ""}`}
      >
        <div
          onClick={(event) => {
            if ((event.target as HTMLElement).closest("button, a, input, select, textarea, [data-download-name]")) {
              lastClick.current = null;
              return;
            }
            if (event.shiftKey || event.ctrlKey || event.metaKey) {
              lastClick.current = null;
              onSelect(item.id, event);
            } else {
              if (event.altKey || event.button !== 0) { lastClick.current = null; return; }
              const previous = lastClick.current;
              const interval = settings.downloadDoubleClickInterval;
              if (previous && previous.interval === interval && event.timeStamp - previous.time <= interval
                && Math.hypot(event.clientX - previous.x, event.clientY - previous.y) <= 5) {
                lastClick.current = null;
                openEntry(event);
                return;
              }
              lastClick.current = { time: event.timeStamp, x: event.clientX, y: event.clientY, interval };
              if (event.detail <= 1) onExpand(item.id);
            }
          }}
          onDoubleClick={(event) => {
            // Opening is timed by onClick; description text keeps native selection.
            if (!(event.target as HTMLElement).closest("button, a, input, select, textarea, [data-download-name]")) event.preventDefault();
          }}
          onMouseDown={(event) => {
            if (event.button !== 0) lastClick.current = null;
            // Suppress autoscroll/navigation only for the assigned mouse binding.
            if (!(event.target as HTMLElement).closest("button") && matchesShortcut(shortcuts, "openDownload", event.nativeEvent)) event.preventDefault();
          }}
          onAuxClick={(event) => {
            if (matchesShortcut(shortcuts, "openDownload", event.nativeEvent)) openEntry(event);
          }}
          onContextMenu={(event) => {
            if (onContextMenu) {
              event.preventDefault();
              onContextMenu(item, event);
            }
          }}
          className={`download-row group flex cursor-pointer select-none items-center gap-3 px-4 ${density === "comfortable" ? "py-3" : "py-[7px]"} ${
            expanded ? "" : "hover:bg-face/70"
          }`}
        >
          <SelectBox
            checked={selected}
            label={`Select ${item.filename}`}
            onToggle={(event) => onSelect(item.id, event)}
          />

          <Lamp fill={aspect.lamp} size={9} />

          <span
            data-download-name
            draggable={done && !isMissing && Boolean(onDragOut)}
            onDragStart={(event) => {
              if (!done || isMissing || !onDragOut) return;
              // Hand the gesture to the OS instead of exposing a webview URL.
              event.preventDefault();
              onDragOut(item);
            }}
            className={`flex min-w-0 flex-[3] items-center gap-2 ${
              done && !isMissing && onDragOut ? "cursor-grab active:cursor-grabbing" : ""
            }`}
          >
            <span
              data-tooltip={
                isMissing
                  ? activeStatus === "inaccessible"
                    ? "Destination folder is unavailable — reconnect drive or locate file"
                    : "File was moved or deleted outside the app — expand to locate or redownload"
                  : done && onDragOut
                    ? "Drag to copy this file"
                    : item.url
              }
              className="t-tt-trigger inline-flex min-w-0 max-w-full items-center gap-2"
            >
              {item.isVideo && (
                <Film className="size-3.5 shrink-0 text-ink-faint" aria-label="Video job" />
              )}
              {item.isMusic && (
                <Note className="size-3.5 shrink-0 text-ink-faint" aria-label="Music job" />
              )}
              <span
                data-selectable
                className={`truncate text-[13px] ${done ? "text-ink-dim" : "text-ink"}`}
              >
                {item.filename}
              </span>
              {isMissing && (
                <span
                  data-missing-tag
                  className="legend shrink-0 rounded-hw border border-danger/40 bg-danger/10 px-1.5 py-[1px] text-[11px] tracking-wider text-danger"
                >
                  {activeStatus === "inaccessible" ? "unavailable" : "missing"}
                </span>
              )}
            </span>
          </span>

          {visible.map((column) => (
            <Cell
              key={column.id}
              column={column}
              item={item}
              dateFormat={telemetry.dateFormat}
              speedUnit={telemetry.speedUnit ?? "MBps"}
              fileStatus={activeStatus}
            />
          ))}

          <span className="flex shrink-0 items-center gap-0.5 pl-1">
            <RowAction
              kind="expand"
              label={expanded ? `Close ${item.filename}` : `Open ${item.filename}`}
              tooltip={expanded ? "Collapse details" : "Expand details"}
              onClick={() => onExpand(item.id)}
              ariaExpanded={expanded}
              className="t-acc-head"
            >
              <span className="t-acc-chevron">
                <ChevronDown className="size-3.5" />
              </span>
            </RowAction>

            {onSendToDiscord && (
              <RowAction kind="share" label={`Send ${item.filename} to Discord`}
                tooltip={done ? "Send to Discord" : "Finish downloading before sending to Discord"}
                disabled={!done} onClick={() => onSendToDiscord([item.id])}>
                <Discord className="size-3.5" />
              </RowAction>
            )}

            {done ? (
              <RowAction
                kind="folder"
                label={isMissing ? `Show destination for ${item.filename}` : `Show ${item.filename} in folder`}
                tooltip={isMissing ? "File missing from disk — show folder" : "Show in folder"}
                onClick={() => onReveal(item.id)}
              >
                <FolderOpen className={`size-3.5 ${isMissing ? "text-danger" : ""}`} />
              </RowAction>
            ) : (
              <RowAction
                kind={running ? "cancel" : "play"}
                label={running ? `Pause ${item.filename}` : `Resume ${item.filename}`}
                tooltip={running ? "Pause" : "Resume"}
                onClick={() => onToggle(item)}
              >
                {running ? <Pause className="size-3.5" /> : <Play className="size-3.5" />}
              </RowAction>
            )}

            <RowAction
              kind="remove"
              label={deleteFilesOnRemove ? `Delete ${item.filename} and local file` : `Remove ${item.filename} from list`}
              tooltip={deleteFilesOnRemove ? "Delete entry and file" : "Remove from list"}
              onClick={() => onRemove(item)}
            >
              <Trash className="size-3.5" />
            </RowAction>
          </span>
        </div>

        {item.state === "failed" && onRetry && <FailureActions item={item} onRetry={() => onRetry(item)} onRecover={onRecover} />}
        {showPanel && (
          <div className="t-acc-panel" aria-hidden={!expanded} inert={!expanded}>
            <div className="t-acc-panel-inner">
              <DownloadDetails item={item} dateFormat={telemetry.dateFormat} speedUnit={telemetry.speedUnit ?? "MBps"} onRemoveEntry={onRemoveEntry ? () => onRemoveEntry(item) : undefined} />
              {detailActions && (
                <div role="group" aria-label={`Actions for ${item.filename}`} className="mx-4 mb-4 flex flex-wrap gap-2 border-t border-edge pt-3">
                  {detailActions.map(action => "type" in action ? null : (
                    <button key={action.id} type="button" disabled={action.disabled}
                      onClick={action.action}
                      className={`focus-ring inline-flex min-h-8 items-center gap-2 rounded-hw border border-edge-strong px-3 py-1.5 text-[11px] transition-colors hover:bg-face disabled:cursor-not-allowed disabled:opacity-40 ${action.danger ? "text-danger" : action.bold ? "font-medium text-route" : "text-ink-dim"}`}>
                      <span aria-hidden="true">{action.icon}</span>
                      {action.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    );
  },
  (prev, next) =>
    // Only re-render if a displayed field actually changed.
    prev.item.id === next.item.id &&
    prev.selected === next.selected &&
    prev.expanded === next.expanded &&
    prev.dimmed === next.dimmed &&
    prev.item.state === next.item.state &&
    prev.item.downloadedBytes === next.item.downloadedBytes &&
    prev.item.totalBytes === next.item.totalBytes &&
    prev.item.bytesPerSecond === next.item.bytesPerSecond &&
    prev.item.etaSeconds === next.item.etaSeconds &&
    prev.item.connections === next.item.connections &&
    prev.item.error === next.item.error &&
    prev.item.filename === next.item.filename &&
    // Read by the optional columns. Static for the life of a row in practice,
    // but a comparator that ignores a displayed field is how a cell goes stale.
    prev.item.priority === next.item.priority &&
    prev.item.createdAt === next.item.createdAt &&
    prev.item.url === next.item.url &&
    prev.item.directory === next.item.directory &&
    prev.item.path === next.item.path &&
    prev.item.thumbnail === next.item.thumbnail &&
    prev.item.quality === next.item.quality &&
    prev.item.stage === next.item.stage &&
    JSON.stringify(prev.item.validation) === JSON.stringify(next.item.validation) &&
    prev.columns === next.columns &&
    prev.density === next.density &&
    prev.onRetry === next.onRetry &&
    prev.onRecover === next.onRecover &&
    prev.deleteFilesOnRemove === next.deleteFilesOnRemove &&
    prev.onRemove === next.onRemove &&
    prev.onRemoveEntry === next.onRemoveEntry &&
    prev.onOpen === next.onOpen &&
    prev.onSendToDiscord === next.onSendToDiscord &&
    prev.detailActions === next.detailActions &&
    prev.onDragOut === next.onDragOut &&
    prev.onContextMenu === next.onContextMenu,
);
