const UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** Human-readable byte size, binary units. */
export function formatBytes(bytes: number, precision = 1): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes === 0) return "0 B";

  const exponent = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    UNITS.length - 1,
  );
  const value = bytes / 1024 ** exponent;
  // Whole bytes never need a decimal point.
  return `${value.toFixed(exponent === 0 ? 0 : precision)} ${UNITS[exponent]}`;
}

/**
 * MBps  = megabytes per second  (binary, ÷ 1 048 576)
 * Mbps  = megabits  per second  (SI,     × 8 ÷ 1 000 000)
 */
export function formatRate(
  bytesPerSecond: number,
  unit: "MBps" | "Mbps" = "MBps",
): string {
  if (bytesPerSecond <= 0) return "—";
  if (unit === "Mbps") {
    const mbps = (bytesPerSecond * 8) / 1_000_000;
    return `${mbps >= 100 ? mbps.toFixed(0) : mbps >= 10 ? mbps.toFixed(1) : mbps.toFixed(2)} Mbps`;
  }
  return `${formatBytes(bytesPerSecond)}/s`;
}

/** Compact duration, e.g. `4m 12s`. */
export function formatEta(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return "—";
  if (seconds < 60) return `${Math.round(seconds)}s`;

  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m ${Math.round(seconds % 60)}s`;

  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

/** Format ISO timestamp into human-readable date & time (e.g. Jul 22, 2026, 5:52 PM). */
export function formatDate(raw?: string): string {
  if (!raw) return "—";
  try {
    const d = new Date(raw);
    if (isNaN(d.getTime())) return "—";
    return d.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      year: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  } catch {
    return "—";
  }
}

/** Format ISO timestamp into relative time string (e.g. '5m ago', '2h ago'). */
export function formatRelativeDate(raw?: string): string {
  if (!raw) return "—";
  try {
    const d = new Date(raw);
    if (isNaN(d.getTime())) return "—";
    const deltaSec = Math.floor((Date.now() - d.getTime()) / 1000);
    if (deltaSec < 45) return "just now";
    const min = Math.floor(deltaSec / 60);
    if (min < 60) return `${min}m ago`;
    const hr = Math.floor(min / 60);
    if (hr < 24) return `${hr}h ago`;
    const days = Math.floor(hr / 24);
    if (days < 30) return `${days}d ago`;
    return formatDate(raw);
  } catch {
    return "—";
  }
}
