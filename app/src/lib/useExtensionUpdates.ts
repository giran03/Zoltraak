import { useCallback, useEffect, useRef, useState } from "react";
import type { ToastItem } from "../components/Toast";
import { engine, hasEngine, type DetectedBrowser, type ExtensionInstallResult, type ExtensionSourceState } from "./engine";
import { extensionUpdateConfirmed, extensionUpdateRows, readPendingExtensionUpdates, writePendingExtensionUpdates, type PendingExtensionUpdate } from "./extensionUpdates";

const NOTICE_KEY = "zoltraak.extension-update-notices.v1";
function readNotices(): Set<string> {
  try { const values: unknown = JSON.parse(localStorage.getItem(NOTICE_KEY) || "[]"); return new Set(Array.isArray(values) ? values.filter((v): v is string => typeof v === "string").slice(-100) : []); } catch { return new Set(); }
}
export function useExtensionUpdates(addToast: (toast: ToastItem) => void, openSettings: () => void) {
  const [browsers, setBrowsers] = useState<DetectedBrowser[]>([]);
  const [sources, setSources] = useState<ExtensionSourceState | null>(null);
  const [bundledVersion, setBundledVersion] = useState<string | null>(null);
  const [signedVersion, setSignedVersion] = useState<string | null>(null);
  const [pending, setPending] = useState(readPendingExtensionUpdates);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checkedAt, setCheckedAt] = useState<number | null>(null);
  const [checking, setChecking] = useState(false);
  const mounted = useRef(false), inFlight = useRef(false), actionInFlight = useRef(false), lastCatalogue = useRef(0), notices = useRef(readNotices());
  const pendingRef = useRef(pending);
  const callbacks = useRef({ addToast, openSettings }); callbacks.current = { addToast, openSettings };
  const catalogue = useRef({ browsers, bundledVersion, signedVersion });
  const sourceRef = useRef<ExtensionSourceState | null>(null);
  const setPendingUpdates = useCallback((values: PendingExtensionUpdate[]) => {
    pendingRef.current = values; writePendingExtensionUpdates(values); if (mounted.current) setPending(values);
  }, []);
  const notice = useCallback((key: string, toast: ToastItem) => {
    if (notices.current.has(key)) return;
    notices.current.add(key);
    const latest = [...notices.current].slice(-100); notices.current = new Set(latest);
    try { localStorage.setItem(NOTICE_KEY, JSON.stringify(latest)); } catch { /* Session deduplication remains available. */ }
    callbacks.current.addToast(toast);
  }, []);
  const refresh = useCallback(async (force = true) => {
    if (!hasEngine() || inFlight.current) return;
    inFlight.current = true; if (mounted.current && force) setChecking(true);
    try {
      const readCatalogue = force || Date.now() - lastCatalogue.current >= 60000;
      const [source, detected, bundled, signed] = await Promise.all([
        engine.extensionSourceState(), readCatalogue ? engine.detectBrowsers() : catalogue.current.browsers,
        readCatalogue ? engine.getExtensionVersion() : catalogue.current.bundledVersion,
        readCatalogue ? engine.getFirefoxExtensionVersion() : catalogue.current.signedVersion,
      ]);
      if (!mounted.current) return;
      if (readCatalogue) { lastCatalogue.current = Date.now(); catalogue.current = { browsers: detected, bundledVersion: bundled, signedVersion: signed }; setBrowsers(detected); setBundledVersion(bundled); setSignedVersion(signed); }
      sourceRef.current = source; setSources(source); setCheckedAt(Date.now()); setError(null);
      const rows = extensionUpdateRows(detected, source.clients, bundled, signed);
      const remaining: PendingExtensionUpdate[] = [];
      for (const update of pendingRef.current) {
        if (extensionUpdateConfirmed(update, source.clients)) {
          const versions = [...new Set(source.clients.filter(client => client.browserId === update.browserId && client.online
            && (!update.profileIds.length || update.profileIds.includes(client.clientId))).map(client => client.extensionVersion))];
          callbacks.current.addToast({ id: `extension-updated-${update.browserId}`, type: "success", title: "Extension updated",
            message: `${update.browserName} confirmed Zoltraak extension v${versions.join(", v")}${update.profileIds.length > 1 ? ` in ${update.profileIds.length} profiles` : ""}.`, durationMs: 9000 });
        } else {
          if (update.phase === "waiting" && Date.now() - update.startedAt >= 90000) {
            remaining.push({ ...update, phase: "needs_attention" });
            callbacks.current.addToast({ id: `extension-update-pending-${update.browserId}`, type: "info", title: "Extension update needs confirmation",
              message: `${update.browserName} has not reported v${update.targetVersion}. Confirm the browser's install prompt, or reopen it and check again.`, durationMs: 12000,
              action: { label: "Review update", onClick: () => callbacks.current.openSettings() } });
          } else remaining.push(update);
        }
      }
      setPendingUpdates(remaining);
      for (const row of rows) {
        if (!row.installed || remaining.some(p => p.browserId === row.browser.id)) continue;
        if (row.needsSignedPackage) {
          notice(`${row.browser.id}:signed:${bundled}`, { id: `extension-update-available-${row.browser.id}`, type: "info", title: `${row.browser.name} extension update needs a signed package`,
            message: `Extension v${bundled} is bundled with the app. Import the matching Mozilla-signed XPI before updating ${row.browser.name}.`, durationMs: 12000, dismissLabel: "Later",
            action: { label: "Open extension settings", onClick: () => callbacks.current.openSettings() } });
        } else if (row.updateAvailable) {
          notice(`${row.browser.id}:${row.availableVersion}`, { id: `extension-update-available-${row.browser.id}`, type: "info", title: "Extension update available",
            message: `${row.browser.name}: v${row.versions.join(", v")} → v${row.availableVersion}.`, durationMs: 12000, dismissLabel: "Later",
            action: { label: "Update extension", onClick: () => void updateRef.current(row.browser.id).catch(() => {}) },
            secondaryAction: { label: "Details", onClick: () => callbacks.current.openSettings() } });
        }
      }
    } catch (cause) {
      if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally { inFlight.current = false; if (mounted.current) setChecking(false); }
  }, [notice, setPendingUpdates]);
  const trackResults = useCallback(async (results: ExtensionInstallResult[]) => {
    const live = await engine.extensionSourceState();
    const [bundled, signed] = await Promise.all([engine.getExtensionVersion(), engine.getFirefoxExtensionVersion()]);
    const values = [...pendingRef.current];
    for (const result of results) {
      const browser = catalogue.current.browsers.find(b => b.id === result.browserId);
      if (!result.success) {
        callbacks.current.addToast({ id: `extension-update-error-${result.browserId}`, type: "error", title: `Could not update ${result.browserName}`, message: result.message,
          durationMs: 12000, action: { label: "Review update", onClick: () => callbacks.current.openSettings() } });
        continue;
      }
      const version = browser?.kind === "firefox" ? signed : bundled;
      if (!version || !browser) continue;
      const index = values.findIndex(p => p.browserId === result.browserId); if (index >= 0) values.splice(index, 1);
      const profiles = live.clients.filter(c => c.browserId === result.browserId && c.online);
      values.push({ browserId: result.browserId, browserName: result.browserName, targetVersion: version, startedAt: Date.now(),
        profileIds: profiles.map(c => c.clientId), phase: "waiting", nextStep: result.nextStep, packagePath: result.extensionPath });
      callbacks.current.addToast({ id: `extension-update-pending-${result.browserId}`, type: "info",
        title: result.nextStep === "browser_confirmation" ? "Confirm the extension update in your browser" : result.nextStep === "restart_browser" ? "Extension update ready — restart browser" : "Checking extension update",
        message: result.message, durationMs: 12000, action: { label: "View status", onClick: () => callbacks.current.openSettings() } });
    }
    sourceRef.current = live; setPendingUpdates(values); if (mounted.current) setSources(live);
    await refresh(true);
  }, [refresh, setPendingUpdates]);
  const update = useCallback(async (browserId = "auto") => {
    if (actionInFlight.current) return [];
    actionInFlight.current = true; if (mounted.current) { setBusy(browserId); setError(null); }
    try {
      const current = catalogue.current;
      const row = extensionUpdateRows(current.browsers, sourceRef.current?.clients || [], current.bundledVersion, current.signedVersion).find(row => row.browser.id === browserId);
      if (row?.verifiedCurrent && !row.updateAvailable && !row.needsSignedPackage && !pendingRef.current.some(p => p.browserId === browserId)) {
        const message = `${row.browser.name} is already running extension v${row.versions.join(", v")}.`;
        callbacks.current.addToast({ id: `extension-current-${browserId}`, type: "info", title: "Extension already up to date", message });
        return [{ browserId, browserName: row.browser.name, success: true, message, extensionPath: "" }];
      }
      const results = await engine.reloadExtension(browserId); await trackResults(results); return results;
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      if (mounted.current) setError(message);
      callbacks.current.addToast({ id: `extension-update-error-${browserId}`, type: "error", title: "Extension update failed", message,
        action: { label: "Retry", onClick: () => void updateRef.current(browserId).catch(() => {}) } });
      throw cause;
    } finally { actionInFlight.current = false; if (mounted.current) setBusy(null); }
  }, [trackResults]);
  const updateRef = useRef(update); updateRef.current = update;
  useEffect(() => {
    mounted.current = true; void refresh(true);
    // Quick while an update is waiting for its browser to report back; at
    // rest, focus and visibility changes below already refresh on demand.
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = () => {
      if (!document.hidden) void refresh(false);
      timer = setTimeout(tick, pendingRef.current.length ? 5000 : 30_000);
    };
    timer = setTimeout(tick, pendingRef.current.length ? 5000 : 30_000);
    // Native dragging can focus the WebView. Refresh live reports immediately,
    // but reuse the browser catalogue until its normal expiry; explicit checks
    // still force discovery through refresh(true).
    const focus = () => { if (!document.hidden) void refresh(false); };
    window.addEventListener("focus", focus); document.addEventListener("visibilitychange", focus);
    return () => { mounted.current = false; clearTimeout(timer); window.removeEventListener("focus", focus); document.removeEventListener("visibilitychange", focus); };
  }, [refresh]);
  return { browsers, sources, bundledVersion, signedVersion, rows: extensionUpdateRows(browsers, sources?.clients || [], bundledVersion, signedVersion),
    pending, busy, checking, checkedAt, error, refresh, update, trackResults,
    dismissPending: (browserId: string) => setPendingUpdates(pendingRef.current.filter(p => p.browserId !== browserId)) };
}
export type ExtensionUpdates = ReturnType<typeof useExtensionUpdates>;
