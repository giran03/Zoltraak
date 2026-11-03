import type { DetectedBrowser, ExtensionClientSummary } from "./engine";

export function compareExtensionVersions(left: string, right: string): number | null {
  if (![left, right].every(value => /^\d+(?:\.\d+){0,3}$/.test(value))) return null;
  const a = left.split(".").map(Number), b = right.split(".").map(Number);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const difference = (a[i] ?? 0) - (b[i] ?? 0);
    if (difference) return Math.sign(difference);
  }
  return 0;
}

export type ExtensionUpdateRow = {
  browser: DetectedBrowser;
  clients: ExtensionClientSummary[];
  availableVersion: string | null;
  versions: string[];
  updateAvailable: boolean;
  needsSignedPackage: boolean;
  installed: boolean;
  verifiedCurrent: boolean;
};

export function extensionUpdateRows(browsers: DetectedBrowser[], clients: ExtensionClientSummary[], bundled: string | null, signed: string | null): ExtensionUpdateRow[] {
  return browsers.map(browser => {
    const profiles = clients.filter(client => client.browserId === browser.id);
    const online = profiles.filter(client => client.online);
    const versions = [...new Set((online.length ? online : profiles).map(client => client.extensionVersion).filter(Boolean))];
    const availableVersion = browser.kind === "firefox" ? signed : bundled;
    const installed = browser.installed || online.length > 0;
    return { browser, clients: profiles, availableVersion, versions, installed,
      needsSignedPackage: browser.kind === "firefox" && Boolean(bundled && (!signed || compareExtensionVersions(signed, bundled) === -1)),
      updateAvailable: installed && Boolean(availableVersion && versions.some(version => compareExtensionVersions(version, availableVersion) === -1)),
      verifiedCurrent: online.length > 0 && Boolean(availableVersion && online.every(client => compareExtensionVersions(client.extensionVersion, availableVersion) !== null && compareExtensionVersions(client.extensionVersion, availableVersion)! >= 0)),
    };
  });
}

export type PendingExtensionUpdate = {
  browserId: string;
  browserName: string;
  targetVersion: string;
  startedAt: number;
  profileIds: string[];
  phase: "waiting" | "needs_attention";
  nextStep?: string;
  packagePath?: string;
};

/** Every profile that was online must reconnect on the target version. A
 * successful file copy, stale heartbeat or one updated profile is not enough. */
export function extensionUpdateConfirmed(pending: PendingExtensionUpdate, clients: ExtensionClientSummary[]): boolean {
  const fresh = clients.filter(client => client.browserId === pending.browserId && client.online
    && (client.lastSeenAtMs ?? 0) > pending.startedAt
    && compareExtensionVersions(client.extensionVersion, pending.targetVersion) !== null
    && compareExtensionVersions(client.extensionVersion, pending.targetVersion)! >= 0);
  if (!fresh.length) return false;
  return pending.profileIds.every(id => fresh.some(client => client.clientId === id));
}

const PENDING_KEY = "zoltraak.extension-updates.pending.v1";
export function readPendingExtensionUpdates(): PendingExtensionUpdate[] {
  try {
    const data: unknown = JSON.parse(localStorage.getItem(PENDING_KEY) || "[]");
    return Array.isArray(data) ? data.filter((p): p is PendingExtensionUpdate => p && typeof p.browserId === "string"
      && typeof p.browserName === "string" && typeof p.targetVersion === "string" && compareExtensionVersions(p.targetVersion, p.targetVersion) === 0
      && Number.isFinite(p.startedAt) && p.startedAt > Date.now() - 7 * 86400000 && Array.isArray(p.profileIds) && p.profileIds.every((id: unknown) => typeof id === "string")
      && ["waiting", "needs_attention"].includes(p.phase)).slice(0, 20) : [];
  } catch { return []; }
}
export function writePendingExtensionUpdates(pending: PendingExtensionUpdate[]) {
  try { localStorage.setItem(PENDING_KEY, JSON.stringify(pending)); } catch { /* Live status still works when storage is unavailable. */ }
}
