import { useCallback, useEffect, useRef, useState } from "react";
import { Section, Switch } from "./Controls";
import { Check, Copy, Plug, Reload, Trash } from "./Icons";
import { ExtensionGuideModal } from "./ExtensionGuideModal";
import { Lamp, Legend } from "./Panel";
import { SessionsSettings } from "./SessionsSettings";
import type { ExtensionUpdates } from "../lib/useExtensionUpdates";
import { extensionUpdateRows } from "../lib/extensionUpdates";
import type { ToastItem } from "./Toast";
import {
  engine,
  hasEngine,
  type ApiStatus,
  type DetectedBrowser,
  type ExtensionInstallResult,
  type ExtensionSourceState,
} from "../lib/engine";

/** One of the things a browser row can be asked to do. */
type BrowserAction = "install" | "update" | "reinstall" | "remove" | "relaunch" | "import";

/**
 * A per-row command.
 *
 * Words, not pictograms: "remove" and "reinstall" are one misread icon apart,
 * and this row is the only place in the app where a wrong click tears out a
 * working install.
 */
function RowButton({
  label,
  title,
  onClick,
  disabled,
  busy,
  tone = "ghost",
}: {
  label: string;
  title: string;
  onClick: () => void;
  disabled?: boolean;
  busy?: boolean;
  tone?: "ghost" | "danger" | "route";
}) {
  const skin =
    tone === "danger"
      ? "border-danger/45 text-danger hover:bg-danger hover:text-on-signal"
      : tone === "route"
        ? "border-route/55 bg-route/10 text-route hover:bg-route hover:text-white"
      : "border-edge-strong text-ink-dim hover:bg-face hover:text-ink";

  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={title}
      title={title}
      className={`focus-ring legend shrink-0 rounded-hw border px-2 py-1 text-[11px] transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${busy ? "animate-pulse" : ""} ${skin}`}
    >
      {label}
    </button>
  );
}

/**
 * Pairing and browser integration in one place.
 *
 * These used to be two overlapping surfaces — a dashboard card with its own
 * auto-install button, and a settings section with per-browser toggles — each
 * able to install the extension by a different route. One list, one master
 * toggle, and the token beside them.
 */
export function ExtensionSettings({ updates, onAddToast }: { updates?: ExtensionUpdates; onAddToast?: (toast: ToastItem) => void }) {
  const [status, setStatus] = useState<ApiStatus | null>(null);
  const [browsers, setBrowsers] = useState<DetectedBrowser[]>([]);
  const [copied, setCopied] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState<{ browserId: string; action: BrowserAction } | null>(null);
  const [guideOpen, setGuideOpen] = useState(false);
  const [guideBrowserId, setGuideBrowserId] = useState<string | undefined>(undefined);
  const [extensionVersion, setExtensionVersion] = useState<string | null>(null);
  const [firefoxSignedVersion, setFirefoxSignedVersion] = useState<string | null>(null);
  const [sources, setSources] = useState<ExtensionSourceState | null>(null);
  const [sourceBusy, setSourceBusy] = useState<string | null>(null);
  const [sourceMessage, setSourceMessage] = useState<string | null>(null);
  const [purging, setPurging] = useState(false);
  const previousActiveSource = useRef<string | null | undefined>(undefined);
  const actionBusy = busy !== null || updates?.busy != null;
  useEffect(() => {
    if (!updates) return;
    setBrowsers(updates.browsers); setExtensionVersion(updates.bundledVersion); setFirefoxSignedVersion(updates.signedVersion);
  }, [updates?.browsers, updates?.bundledVersion, updates?.signedVersion]);

  async function handlePurgeAndRegenerate() {
    if (actionBusy || purging) return;
    setPurging(true);
    setMessage(null);
    try {
      const res = await engine.purgeAndRegenerateExtensions();
      setMessage(res.message);
      onAddToast?.({ id: "extension-rebuilt", type: res.success ? "info" : "error", title: res.success ? "Extension files rebuilt" : "Extension rebuild failed", message: res.message });
    } catch (e) {
      setMessage((e as Error).message || "Purge and regenerate failed.");
    } finally {
      setPurging(false);
      await refreshBrowsers();
    }
  }

  const refreshSources = useCallback(async () => {
    if (!hasEngine()) return;
    try {
      const next = await engine.extensionSourceState();
      if (
        previousActiveSource.current !== undefined &&
        previousActiveSource.current !== next.activeClientId
      ) {
        const active = next.clients.find((client) => client.clientId === next.activeClientId);
        setSourceMessage(
          active
            ? `${active.browserName} is now the ${active.temporary ? "temporary " : ""}cookie source. Shared sessions are refreshing.`
            : "No browser profile is currently allowed to supply cookies.",
        );
      }
      previousActiveSource.current = next.activeClientId;
      setSources(next);
    } catch {
      // Keep the last honest heartbeat list on screen during a transient IPC failure.
    }
  }, []);

  useEffect(() => {
    if (!hasEngine()) return;
    engine.apiStatus().then(setStatus).catch(() => {});
    engine.detectBrowsers().then(setBrowsers).catch(() => {});
    engine.getExtensionVersion().then(setExtensionVersion).catch(() => {});
    engine.getFirefoxExtensionVersion().then(setFirefoxSignedVersion).catch(() => {});
    void refreshSources();
    const sourceTimer = setInterval(() => void refreshSources(), 2_000);
    return () => clearInterval(sourceTimer);
  }, [refreshSources]);

  async function selectSource(clientId: string) {
    if (sourceBusy) return;
    setSourceBusy(clientId);
    setSourceMessage(null);
    try {
      const next = await engine.selectExtensionSource(clientId);
      previousActiveSource.current = next.activeClientId;
      setSources(next);
      const selected = next.clients.find((client) => client.clientId === clientId);
      setSourceMessage(`${selected?.browserName ?? "Browser"} selected. Shared sessions are refreshing.`);
    } catch (error) {
      setSourceMessage((error as Error).message || "Could not select that browser profile.");
    } finally {
      setSourceBusy(null);
    }
  }

  const refreshBrowsers = async () => {
    if (!hasEngine()) return;
    try {
      const [detected, signedVersion] = await Promise.all([
        engine.detectBrowsers(),
        engine.getFirefoxExtensionVersion(),
      ]);
      setBrowsers(detected);
      setFirefoxSignedVersion(signedVersion);
      await updates?.refresh(true);
    } catch {
      // A failed refresh leaves the previous list on screen, which is more
      // useful than emptying it.
    }
  };

  /**
   * Re-read the pairing state and say what it means, after an update.
   *
   * An update re-bakes the current token into `api.js`, which is the app's
   * half of reconnecting — a browser that picks the files up is paired again
   * without the user copying anything. What it cannot do is reach into the
   * extension's storage, where a rejected token outranks the baked one, so the
   * extension has the matching half on its own reload button.
   *
   * The API status is re-read because it is the precondition for any of this:
   * a token is worthless while nothing is listening on the port, and that is
   * worth saying plainly rather than leaving a stale "listening" lamp up.
   */
  async function pairingNote(): Promise<string | null> {
    const current = await engine.apiStatus().catch(() => null);
    if (!current) return null;
    setStatus(current);
    return current.listening
      ? "Token re-issued. Pairing window active for 60s — extension will auto-pair on its next request or check."
      : `The local API is not listening${current.detail ? ` — ${current.detail}` : ""}. Nothing can pair until it is.`;
  }

  /**
   * Run one action against one browser, or against every detected one when
   * `browserId` is "auto".
   *
   * Reinstall is remove-then-install rather than its own engine call: the
   * difference from update is that it clears what is on disk first, so a
   * half-written or hand-edited copy cannot survive into the new install. The
   * removal is allowed to fail — there may be nothing there to remove, and the
   * install that follows is the part that matters.
   */
  async function run(browserId: string, action: BrowserAction) {
    if (actionBusy) return;
    setBusy({ browserId, action });
    setMessage(null);
    try {
      let results: ExtensionInstallResult[];
      if (action === "import") {
        results = await engine.importFirefoxExtension();
      } else if (action === "remove") {
        results = await engine.removeExtension(browserId);
      } else if (action === "update") {
        results = updates ? await updates.update(browserId) : await engine.reloadExtension(browserId);
      } else if (action === "reinstall") {
        // Clear old extension files first, then perform clean installation
        await engine
          .removeExtension(browserId)
          .catch(() => [] as ExtensionInstallResult[]);
        results = await engine.installExtension(browserId);
      } else if (action === "relaunch") {
        const res = await engine.relaunchBrowser(browserId);
        results = [res];
      } else {
        results = await engine.installExtension(browserId);
      }

      // Automatically copy extension directory path to clipboard for instant Ctrl+V into file dialog
      if (results && results.length > 0) {
        const path = results.find((r) => !r.success && r.extensionPath)?.extensionPath;
        if (path) {
          void navigator.clipboard.writeText(path).catch(() => {});
        }
      }

      const msgs = (results ?? []).map((r) => r.message).filter(Boolean);
      const note = action === "update" && results.some(result => result.extensionPath) ? await pairingNote() : null;
      setMessage([...msgs, note].filter(Boolean).join(" | ") || null);
      if (action === "import" && updates) await updates.trackResults(results);
      if (action !== "update" && action !== "import") {
        const failed = results.some(result => !result.success);
        onAddToast?.({ id: `extension-${action}-${browserId}`, type: failed ? "error" : "info",
          title: failed ? "Browser action needs attention" : action === "remove" ? "Extension removal requested" : "Browser action complete", message: msgs.join(" | ") || "No browsers were changed." });
        if ((action === "install" || action === "reinstall") && updates) await updates.trackResults(results.filter(result => result.success));
      }
    } catch (e) {
      setMessage((e as Error).message || "Action failed.");
    } finally {
      setBusy(null);
      await refreshBrowsers();
    }
  }

  /** True while `action` is in flight for `browserId` specifically. */
  const running = (browserId: string, action: BrowserAction) =>
    busy?.browserId === browserId && busy.action === action;

  async function copyToken() {
    if (!status?.token) return;
    await navigator.clipboard.writeText(status.token);
    setCopied(true);
    setTimeout(() => setCopied(false), 1800);
  }

  const anyInstalled = browsers.some((b) => b.installed || sources?.clients.some(client => client.browserId === b.id && client.online));
  const hasFirefox = browsers.some((browser) => browser.kind === "firefox");
  const availableUpdates = updates?.rows.filter(row => row.updateAvailable && !updates.pending.some(p => p.browserId === row.browser.id)) || [];

  return (
    <div className="flex flex-col gap-7">
      <Section
        title="Local API"
        aside={
          <span className="flex items-center gap-2">
            <Lamp fill={status?.listening ? "bg-clear" : null} size={7} />
            <Legend>{status?.listening ? "listening" : "closed"}</Legend>
          </span>
        }
        description="The token is generated once and reused across restarts. Every route except /health requires it, and an Origin allowlist rejects web pages outright — loopback on its own is not an authorization boundary."
      >
        <div className="flex flex-col gap-3 border border-edge bg-face px-4 py-3.5">
          <div className="flex items-baseline gap-3">
            <Legend>address</Legend>
            <span data-measure className="describer text-[12px] text-ink">
              {!status
                ? "checking…"
                : status.listening
                  ? `127.0.0.1:${status.port}`
                  : "port unavailable"}
            </span>
          </div>

          {status && !status.listening && (
            <p className="text-[12px] leading-relaxed text-ink-dim">
              {status.detail ?? "Another Zoltraak process may already be running."}
            </p>
          )}

          {status?.token && (
            <div className="flex flex-col gap-1.5">
              <Legend>pairing token</Legend>
              <button
                type="button"
                onClick={() => void copyToken()}
                title="Copy the pairing token"
                className="recess focus-ring flex w-full items-center justify-between gap-3 rounded-hw px-2.5 py-1.5 text-left transition-colors hover:bg-sunk"
              >
                <span data-selectable className="describer truncate text-[11px] text-ink-dim">
                  {status.token.slice(0, 8)}••••••••••••••••{status.token.slice(-4)}
                </span>
                {copied ? (
                  <Check className="size-3.5 shrink-0 text-clear" />
                ) : (
                  <Copy className="size-3.5 shrink-0 text-ink-faint" />
                )}
              </button>
              <Legend className={copied ? "text-clear" : undefined}>
                {copied
                  ? "copied — paste it into the extension popup"
                  : "paste this into the extension popup to pair"}
              </Legend>
            </div>
          )}
        </div>
      </Section>

      <Section
        title="Cookie source"
        aside={
          sources?.activeClientId ? (
            <span className="flex items-center gap-2">
              <Lamp fill="bg-clear" size={7} />
              <Legend>one source active</Legend>
            </span>
          ) : undefined
        }
        description="Choose the browser profile allowed to share sign-in cookies and YouTube player tokens. Other extension profiles can still send public links, but they cannot read or replace the shared cookie vault."
      >
        {!sources || sources.clients.length === 0 ? (
          <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-10 text-center">
            <span className="h-px w-24 bg-edge" />
            <Legend className="text-ink-dim">no extension profiles connected</Legend>
            <p className="max-w-sm text-[12px] leading-relaxed text-ink-dim">
              Open a browser with the Zoltraak extension installed. It will appear here after it connects.
            </p>
          </div>
        ) : (
          <fieldset className="border border-edge bg-face">
            <legend className="sr-only">Preferred browser cookie source</legend>
            {sources.clients.map((client) => {
              const duplicateCount = sources.clients.filter(
                (candidate) => candidate.browserId === client.browserId,
              ).length;
              const status = client.temporary
                ? "temporary active"
                : client.active
                  ? "active cookie source"
                  : client.preferred
                    ? "preferred — offline"
                    : client.online
                      ? "available"
                      : "offline";
              return (
                <label
                  key={client.clientId}
                  className={`flex min-h-12 cursor-pointer items-center gap-3 border-b border-edge px-4 py-2.5 transition-colors last:border-b-0 ${
                    client.active ? "bg-route/10" : "hover:bg-panel"
                  } ${client.online ? "" : "cursor-not-allowed opacity-60"}`}
                >
                  <input
                    type="radio"
                    name="preferred-cookie-source"
                    value={client.clientId}
                    checked={client.preferred}
                    disabled={!client.online || sourceBusy !== null}
                    onChange={() => void selectSource(client.clientId)}
                    className="focus-ring size-4 shrink-0 accent-route"
                  />
                  <Lamp fill={client.active ? "bg-clear" : client.online ? "bg-caution" : null} size={7} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13px] text-ink">
                      {client.browserName}
                      {duplicateCount > 1 && (
                        <span data-measure className="describer ml-2 text-[11px] text-ink-faint">
                          instance {client.clientId.slice(0, 8)}
                        </span>
                      )}
                    </span>
                    <span className="flex flex-wrap items-baseline gap-2">
                      <Legend className={client.active ? "text-clear" : undefined}>{status}</Legend>
                      {client.extensionVersion && <Legend className="text-ink-faint">v{client.extensionVersion}</Legend>}
                    </span>
                  </span>
                </label>
              );
            })}
          </fieldset>
        )}
        {sourceMessage && (
          <p
            role="status"
            aria-live="polite"
            className="border border-route/45 bg-route/10 px-3 py-2.5 text-[12px] leading-relaxed text-ink"
          >
            {sourceMessage}
          </p>
        )}
      </Section>

      <Section
        title="Browser integration"
        aside={
          <div className="flex max-w-[65%] flex-wrap items-center justify-end gap-2">
            {updates && <RowButton label={updates.checking ? "checking…" : "check for updates"} title="Check extension versions and connected browser profiles" onClick={() => void updates.refresh(true)} disabled={updates.checking || actionBusy} />}
            {extensionVersion && <Legend className="text-ink-faint">latest v{extensionVersion}</Legend>}
            <button
              type="button"
              onClick={() => {
                setGuideBrowserId(undefined);
                setGuideOpen(true);
              }}
              title="Open browser extension setup & loading guide"
              className="focus-ring flex items-center gap-1.5 rounded-hw border border-edge-strong bg-face px-2.5 py-1 text-[11px] font-semibold text-ink hover:bg-panel transition-colors"
            >
              <Plug className="size-3 text-route" />
              <span>Setup Guide</span>
            </button>
            <button
              type="button"
              onClick={() => void handlePurgeAndRegenerate()}
              disabled={actionBusy || purging}
              title="Purge cached/broken extension files, reset reload states, and regenerate fresh unpacked extensions"
              className="focus-ring flex items-center gap-1.5 rounded-hw border border-edge-strong bg-face px-2.5 py-1 text-[11px] font-semibold text-ink hover:bg-panel transition-colors disabled:opacity-45"
            >
              <Trash className={`size-3 text-danger ${purging ? "animate-spin" : ""}`} />
              <span>{purging ? "Purging..." : "Purge & Rebuild"}</span>
            </button>
            {browsers.length > 0 && (
              <Switch
                label={anyInstalled ? "Remove from all browsers" : "Install in all browsers"}
                checked={anyInstalled}
                disabled={actionBusy}
                onChange={(next) => void run("auto", next ? "install" : "remove")}
              />
            )}
            {hasFirefox && (
              <RowButton
                label="import XPI"
                title="Import a Mozilla-signed Firefox or Zen extension package"
                onClick={() => void run("firefox", "import")}
                disabled={actionBusy}
                busy={running("firefox", "import")}
              />
            )}
            <button
              type="button"
              onClick={() => void run("auto", "update")}
              disabled={actionBusy || !anyInstalled}
              aria-label="Update the extension on all browsers"
              title="Update extension on all browsers"
              className="focus-ring flex size-7 items-center justify-center rounded-hw border border-edge-strong text-ink-dim transition-colors hover:bg-face hover:text-ink disabled:opacity-45"
            >
              <Reload
                className={`size-3.5 ${running("auto", "update") ? "animate-spin" : ""}`}
              />
            </button>
          </div>
        }
        description="Update Chromium in place. For Zen and Firefox, confirm the signed add-on installation in the browser. Success is verified from the connected extension version; writing files alone does not confirm activation."
      >
        {updates?.checkedAt && <p className="text-[12px] text-ink-dim">Last checked {new Date(updates.checkedAt).toLocaleTimeString()}. Versions are confirmed from browser connections.</p>}
        {updates?.error && <p role="alert" className="border border-danger/45 bg-danger/10 px-3 py-2.5 text-[12px] text-ink">Extension updates need attention: {updates.error}</p>}
        {updates && availableUpdates.length > 0 && (
          <div role="status" className="flex flex-wrap items-center justify-between gap-3 border border-route/45 bg-route/10 px-3 py-2.5">
            <p className="text-[12px] text-ink">Extension updates available for {availableUpdates.map(row => row.browser.name).join(", ")}.</p>
            <RowButton label="update extensions" title="Update installed browser extensions" onClick={() => void run("auto", "update")} disabled={actionBusy} tone="route" />
          </div>
        )}
        {updates?.pending.map(update => (
          <div key={update.browserId} role="status" className="flex flex-wrap items-center justify-between gap-3 border border-caution/45 bg-caution/10 px-3 py-2.5">
            <p className="min-w-0 flex-1 text-[12px] leading-relaxed text-ink">
              {update.browserName} v{update.targetVersion}: {update.phase === "needs_attention" ? "not yet confirmed" : "waiting for confirmation"}.
              {update.nextStep === "browser_confirmation" ? " Approve the add-on installation in the browser." : update.nextStep === "restart_browser" ? " Restart the browser or install the XPI from Add-ons." : " Keep the browser open so its extension can reconnect."}
            </p>
            <RowButton label="check again" title={`Verify the extension running in ${update.browserName}`} onClick={() => void updates.refresh(true)} disabled={updates.checking} />
            <RowButton label="retry update" title={`Retry the ${update.browserName} extension update`} onClick={() => void run(update.browserId, "update")} disabled={actionBusy} />
            {(update.nextStep === "browser_confirmation" || update.nextStep === "restart_browser") && <>
              <RowButton label="open add-ons" title={`Open ${update.browserName}'s Add-ons manager`} onClick={() => void engine.openExtensionManager(update.browserId).catch(error => setMessage((error as Error).message))} />
              {update.packagePath && <RowButton label="copy XPI path" title="Copy the signed XPI path for Install Add-on From File" onClick={() => void navigator.clipboard.writeText(update.packagePath!).then(() => onAddToast?.({ id: "extension-path-copied", type: "info", title: "Signed extension path copied", message: "In the browser's Add-ons manager, choose Install Add-on From File and paste this path." })).catch(() => setMessage(`Copy this signed XPI path: ${update.packagePath}`))} />}
            </>}
          </div>
        ))}
        {browsers.length === 0 ? (
          <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-12 text-center">
            <span className="h-px w-24 bg-edge" />
            <Legend className="text-ink-dim">no browsers detected</Legend>
            <p className="max-w-sm text-[12px] leading-relaxed text-ink-dim">
              Either no supported browser is installed, or this view is running outside the desktop
              app.
            </p>
          </div>
        ) : (
          <div className="border border-edge bg-face">
            {browsers.map((b) => {
              const summary = extensionUpdateRows([b], sources?.clients || [], extensionVersion, firefoxSignedVersion)[0]!;
              const { availableVersion, updateAvailable, needsSignedPackage: signedPackageOutdated } = summary;
              const installedVersion = summary.versions.length ? summary.versions.map(version => `v${version}`).join(", ") : null;
              const pendingUpdate = updates?.pending.find(update => update.browserId === b.id);
              const lifecycleStatus = pendingUpdate
                ? pendingUpdate.phase === "needs_attention" ? "update needs confirmation" : "waiting for browser confirmation"
                : !summary.installed ? "not installed"
                : signedPackageOutdated ? firefoxSignedVersion ? "signed package outdated" : "signed package missing"
                : updateAvailable ? "update available"
                : summary.verifiedCurrent ? "up to date — browser verified"
                : "open browser to verify";
              return (
              <div
                key={b.id}
                className={`flex flex-wrap items-center justify-between gap-4 border-b border-edge px-4 py-2.5 transition-colors last:border-b-0 hover:bg-panel ${updateAvailable ? "bg-route/10" : ""}`}
              >
                <div className="flex min-w-0 items-center gap-3">
                  <Lamp fill={summary.installed ? "bg-clear" : null} size={7} />
                  <div className="min-w-0">
                    <p className="truncate text-[13px] text-ink">{b.name}</p>
                    <span className="flex flex-wrap items-baseline gap-2">
                      <Legend className={updateAvailable ? "text-route" : undefined}>{lifecycleStatus}</Legend>
                      {b.running && <Legend className="text-caution">running</Legend>}
                    </span>
                    {summary.installed && (
                      <span data-measure className="describer mt-0.5 block text-[11px] text-ink-faint">
                        observed {installedVersion || "version unknown"}
                        {availableVersion ? ` · available v${availableVersion}` : " · signed package unavailable"}
                      </span>
                    )}
                  </div>
                </div>
                <div className="flex w-full flex-wrap items-center gap-1.5 sm:w-auto sm:justify-end">
                  {summary.installed ? (
                    <>
                      <RowButton
                        label="guide"
                        title={`Open setup guide for ${b.name}`}
                        onClick={() => {
                          setGuideBrowserId(b.id);
                          setGuideOpen(true);
                        }}
                        disabled={actionBusy}
                      />
                      <RowButton
                        label={pendingUpdate ? "retry update" : "update"}
                        title={
                          b.kind === "firefox"
                            ? `Open the signed extension update in ${b.name} and verify activation`
                            : `Rewrite the extension files for ${b.name} and reload once`
                        }
                        onClick={() => void run(b.id, "update")}
                        disabled={actionBusy}
                        busy={running(b.id, "update") || updates?.busy === b.id || updates?.busy === "auto"}
                        tone={updateAvailable || pendingUpdate ? "route" : "ghost"}
                      />
                      <RowButton
                        label="reinstall"
                        title={`Remove and install Zoltraak again in ${b.name}`}
                        onClick={() => void run(b.id, "reinstall")}
                        disabled={actionBusy}
                        busy={running(b.id, "reinstall")}
                      />
                      {b.kind === "chromium" && b.running && (
                        <RowButton
                          label="relaunch"
                          title={`Relaunch ${b.name} with Zoltraak pre-loaded`}
                          onClick={() => void run(b.id, "relaunch")}
                          disabled={actionBusy}
                          busy={running(b.id, "relaunch")}
                        />
                      )}
                      <RowButton
                        label="uninstall"
                        tone="danger"
                        title={`Remove Zoltraak from ${b.name}`}
                        onClick={() => void run(b.id, "remove")}
                        disabled={actionBusy}
                        busy={running(b.id, "remove")}
                      />
                    </>
                  ) : (
                    <>
                      <RowButton
                        label="guide"
                        title={`Open step-by-step setup guide for ${b.name}`}
                        onClick={() => {
                          setGuideBrowserId(b.id);
                          setGuideOpen(true);
                        }}
                        disabled={actionBusy}
                      />
                      <RowButton
                        label="install"
                        title={`Install Zoltraak in ${b.name}`}
                        onClick={() => void run(b.id, "install")}
                        disabled={actionBusy}
                        busy={running(b.id, "install")}
                      />
                      {b.kind === "chromium" && b.running && (
                        <RowButton
                          label="relaunch & load"
                          title={`Relaunch ${b.name} to immediately load the unpacked extension`}
                          onClick={() => void run(b.id, "relaunch")}
                          disabled={actionBusy}
                          busy={running(b.id, "relaunch")}
                        />
                      )}
                    </>
                  )}
                </div>
              </div>
              );
            })}
          </div>
        )}

        {/* Automated workflow card */}
        <div className="flex flex-col gap-2 rounded-hw border border-edge/80 bg-panel/60 p-3 text-[12px] leading-relaxed text-ink-dim">
          <div className="flex items-center gap-1.5 font-semibold text-ink text-[11px] uppercase tracking-wide">
            <span>Automated Chromium & Gecko Setup</span>
          </div>
          <p>
            Zoltraak automatically loads and synchronizes unpacked extensions:
          </p>
          <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-3 pt-1 text-[11px]">
            <div className="flex flex-col gap-1 rounded-hw border border-edge bg-face/70 p-2.5">
              <span className="font-semibold text-ink">1. Auto-Load on Launch</span>
              <span>Supported Chromium browsers restart and load the extension from one click. Google Chrome requires manual unpacked loading until a store build is available.</span>
            </div>
            <div className="flex flex-col gap-1 rounded-hw border border-edge bg-face/70 p-2.5">
              <span className="font-semibold text-ink">2. Live Auto-Reload</span>
              <span>Update acknowledges the revision before Chromium reloads, so each browser reloads once. Firefox and Zen deploy the signed XPI and then ask for a restart.</span>
            </div>
            <div className="flex flex-col gap-1 rounded-hw border border-edge bg-face/70 p-2.5">
              <span className="font-semibold text-ink">3. One-Click Relaunch</span>
              <span>Install restarts supported Chromium browsers and restores tabs. Save any unfinished work first. Use relaunch to load again after a normal browser restart.</span>
            </div>
          </div>
        </div>

        {message && (
          <p
            role="status"
            className="border border-route/45 bg-route/10 px-3 py-2.5 text-[12px] leading-relaxed text-ink"
          >
            {message}
          </p>
        )}
      </Section>

      <SessionsSettings />

      <ExtensionGuideModal
        updates={updates}
        open={guideOpen}
        onClose={() => {
          setGuideOpen(false);
          void refreshBrowsers();
        }}
        onOpenSettings={() => {
          setGuideOpen(false);
          void refreshBrowsers();
        }}
        initialBrowserId={guideBrowserId}
      />
    </div>
  );
}
