import { useEffect, useRef, useState } from "react";
import { engine, hasEngine, type ApiStatus, type DetectedBrowser, type ExtensionSourceState } from "../lib/engine";
import type { ExtensionUpdates } from "../lib/useExtensionUpdates";
import { extensionUpdateRows } from "../lib/extensionUpdates";
import { Button } from "./Controls";
import { Check, Close, Copy, FolderOpen, Plug, Reload, Sliders } from "./Icons";
import { Lamp, Legend } from "./Panel";

export type ExtensionGuideModalProps = {
  open: boolean;
  onClose: () => void;
  onOpenSettings: () => void;
  initialBrowserId?: string;
  updates?: ExtensionUpdates;
};

const BROWSER_META: Record<
  string,
  { label: string; kind: "firefox" | "chromium"; storeUrl?: string; settingsUrl: string }
> = {
  zen: {
    label: "Zen Browser",
    kind: "firefox",
    settingsUrl: "about:addons",
  },
  firefox: {
    label: "Mozilla Firefox",
    kind: "firefox",
    settingsUrl: "about:addons",
  },
  chrome: {
    label: "Google Chrome",
    kind: "chromium",
    settingsUrl: "chrome://extensions",
  },
  edge: {
    label: "Microsoft Edge",
    kind: "chromium",
    settingsUrl: "edge://extensions",
  },
  brave: {
    label: "Brave Browser",
    kind: "chromium",
    settingsUrl: "brave://extensions",
  },
  vivaldi: {
    label: "Vivaldi",
    kind: "chromium",
    settingsUrl: "vivaldi://extensions",
  },
  opera: {
    label: "Opera",
    kind: "chromium",
    settingsUrl: "opera://extensions",
  },
};

export function ExtensionGuideModal({
  open,
  onClose,
  onOpenSettings,
  initialBrowserId,
  updates,
}: ExtensionGuideModalProps) {
  const [browsers, setBrowsers] = useState<DetectedBrowser[]>([]);
  const [selectedId, setSelectedId] = useState<string>("zen");
  const [status, setStatus] = useState<ApiStatus | null>(null);
  const [extensionPath, setExtensionPath] = useState<string | null>(null);
  const [pathCopied, setPathCopied] = useState(false);
  const [tokenCopied, setTokenCopied] = useState(false);
  const [urlCopied, setUrlCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [localSources, setLocalSources] = useState<ExtensionSourceState | null>(null);
  const [bundledVersion, setBundledVersion] = useState<string | null>(null);
  const [signedVersion, setSignedVersion] = useState<string | null>(null);
  const [confirmAction, setConfirmAction] = useState<"restart" | "reinstall" | null>(null);
  const actionInFlight = useRef(false);
  const operationBusy = busy || Boolean(updates?.busy);

  useEffect(() => { if (open && updates) setBrowsers(updates.browsers); }, [open, updates?.browsers]);
  useEffect(() => {
    if (!open || !hasEngine() || updates) return;
    let active = true;
    Promise.allSettled([engine.extensionSourceState(), engine.getExtensionVersion(), engine.getFirefoxExtensionVersion()]).then(([source,bundled,signed]) => {
      if (!active) return;
      if (source.status === "fulfilled") setLocalSources(source.value);
      if (bundled.status === "fulfilled") setBundledVersion(bundled.value);
      if (signed.status === "fulfilled") setSignedVersion(signed.value);
    });
    return () => { active = false; };
  }, [open, updates]);

  useEffect(() => {
    if (!open) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [open, onClose]);

  useEffect(() => {
    if (!open || !hasEngine()) return;

    let active = true;
    Promise.all([engine.detectBrowsers(), engine.apiStatus()])
      .then(([detected, apiStat]) => {
        if (!active) return;
        setBrowsers(detected);
        setStatus(apiStat);

        // Pick initial selected browser
        if (initialBrowserId && detected.some((b) => b.id === initialBrowserId)) {
          setSelectedId(initialBrowserId);
        } else if (detected.length > 0) {
          // Priority: running browser -> installed browser -> first detected
          const running = detected.find((b) => b.running);
          const installed = detected.find((b) => b.installed);
          setSelectedId(running?.id ?? installed?.id ?? detected[0]?.id ?? "zen");
        }
      })
      .catch(() => {});

    return () => {
      active = false;
    };
  }, [open, initialBrowserId]);

  // When selected browser changes, fetch its extension path
  useEffect(() => {
    if (!open || !hasEngine() || !selectedId) return;
    let active = true;
    setExtensionPath(null);
    setPathCopied(false); setUrlCopied(false); setConfirmAction(null);
    if (BROWSER_META[selectedId]?.kind === "firefox") return;
    engine
      .getExtensionDirectory(selectedId)
      .then(path => { if (active) setExtensionPath(path); })
      .catch(() => {
        // Default fallback display path
        if (active) setExtensionPath(null);
      });
    return () => { active = false; };
  }, [open, selectedId]);

  if (!open) return null;

  const currentBrowser = browsers.find((b) => b.id === selectedId);
  const meta = BROWSER_META[selectedId] || {
    label: currentBrowser?.name ?? selectedId,
    kind: currentBrowser?.kind ?? "chromium",
    settingsUrl: "chrome://extensions",
  };
  const isGecko = meta.kind === "firefox";
  const updateRow = (updates?.rows || extensionUpdateRows(browsers, localSources?.clients || [], bundledVersion, signedVersion)).find(row => row.browser.id === selectedId);
  const installed = Boolean(updateRow?.installed || currentBrowser?.installed);
  const onlineClients = updateRow?.clients.filter(client => client.online) || [];
  const pendingUpdate = updates?.pending.find(update => update.browserId === selectedId);

  const refreshStatus = async () => {
    const [detected,api] = await Promise.all([engine.detectBrowsers(),engine.apiStatus()]);
    setBrowsers(detected); setStatus(api);
    if (updates) await updates.refresh(true);
    else {
      const [source,bundled,signed] = await Promise.all([engine.extensionSourceState(),engine.getExtensionVersion(),engine.getFirefoxExtensionVersion()]);
      setLocalSources(source); setBundledVersion(bundled); setSignedVersion(signed);
    }
  };
  const manage = async (action: "update" | "reload" | "reinstall" | "restart" | "manager" | "refresh") => {
    if (operationBusy || actionInFlight.current || !hasEngine()) return;
    actionInFlight.current = true; setBusy(true); setActionMessage(null); setConfirmAction(null);
    try {
      if (action === "manager") await engine.openExtensionManager(selectedId);
      else if (action === "refresh") { await refreshStatus(); setActionMessage("Browser and extension status refreshed."); return; }
      else {
        const results = action === "update" && updates ? await updates.update(selectedId)
          : action === "reinstall" ? await engine.installExtension(selectedId)
          : action === "restart" ? [await engine.relaunchBrowser(selectedId)]
          : await engine.reloadExtension(selectedId);
        setActionMessage(results.find(result => result.browserId === selectedId)?.message || "No result returned for this browser.");
        if (updates && action !== "update") await updates.trackResults(results);
      }
      await refreshStatus();
    } catch (cause) { setActionMessage(cause instanceof Error ? cause.message : String(cause)); }
    finally { actionInFlight.current = false; setBusy(false); }
  };

  const copyPath = async () => {
    if (!extensionPath) return;
    try {
      await navigator.clipboard.writeText(extensionPath);
      setPathCopied(true);
      setTimeout(() => setPathCopied(false), 2000);
    } catch {}
  };

  const copyToken = async () => {
    if (!status?.token) return;
    try {
      await navigator.clipboard.writeText(status.token);
      setTokenCopied(true);
      setTimeout(() => setTokenCopied(false), 2000);
    } catch {}
  };

  const copySettingsUrl = async () => {
    try {
      await navigator.clipboard.writeText(meta.settingsUrl);
      setUrlCopied(true);
      setTimeout(() => setUrlCopied(false), 2000);
    } catch {}
  };

  const handleInstallGecko = () => manage("reinstall");
  const handleRelaunchChromium = () => manage("restart");

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Browser Extension Setup Guide"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="t-modal is-open plate modal-plate glass-panel flex max-h-[calc(100dvh-32px)] w-full max-w-2xl flex-col border border-edge-strong bg-face shadow-2xl overflow-hidden rounded-hw-modal">
        {/* Enamel Header */}
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-edge-strong bg-recess/40 px-5 py-3.5">
          <div className="flex items-center gap-2.5">
            <span className="flex size-5 items-center justify-center rounded-hw bg-route text-white">
              <Plug className="size-3.5" />
            </span>
            <div>
              <div className="flex flex-wrap items-center gap-2">
                <span className="legend text-[11px] tracking-[0.16em] text-ink uppercase">
                  {installed ? "Browser Extension Controls" : "Browser Extension Setup"}
                </span>
                <span className="rounded-hw border border-edge bg-face px-1.5 py-0.2 text-[11px] font-mono text-ink-dim">
                  Zoltraak Interceptor
                </span>
              </div>
              <p className="text-[11px] text-ink-dim">
                Enables 1-click video detection, media interception, and high-speed downloads.
              </p>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              onClick={onOpenSettings}
              aria-label="Open extension settings"
              className="focus-ring flex h-7 items-center gap-1.5 rounded-hw border border-edge bg-face px-2.5 text-ink-dim transition-colors hover:bg-panel hover:text-ink"
            >
              <Sliders className="size-3.5 text-route" />
              <span className="legend text-[11px]">Settings</span>
            </button>
            <button
              type="button"
              onClick={onClose}
              title="Close (Esc)"
              aria-label="Close extension setup"
              className="focus-ring flex size-7 items-center justify-center rounded-hw border border-edge text-ink-dim hover:bg-face hover:text-ink transition-colors"
            >
              <Close className="size-3.5" />
            </button>
          </div>
        </div>

        {/* Browser Selector Strip */}
        <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-edge bg-panel px-5 py-2.5">
          <Legend className="mr-1.5 text-ink-faint">Target:</Legend>
          {(browsers.length > 0
            ? browsers
            : Object.keys(BROWSER_META).map((id) => ({
                id,
                name: BROWSER_META[id]?.label ?? id,
                kind: BROWSER_META[id]?.kind ?? "chromium",
                running: false,
                executablePath: null,
                installed: false,
              }))
          ).map((b) => {
            const active = b.id === selectedId;
            return (
              <button
                key={b.id}
                type="button"
                disabled={operationBusy}
                aria-pressed={active}
                onClick={() => {
                  setSelectedId(b.id);
                  setActionMessage(null);
                }}
                className={`focus-ring flex items-center gap-2 rounded-hw border px-2.5 py-1 text-[11px] transition-colors ${
                  active
                    ? "border-route bg-recess font-semibold text-ink shadow-xs"
                    : "border-edge bg-face/70 text-ink-dim hover:bg-face hover:text-ink"
                }`}
              >
                <Lamp
                  fill={
                    b.installed
                      ? "bg-clear"
                      : b.running
                        ? "bg-caution"
                        : null
                  }
                  size={6}
                />
                <span>{b.name}</span>
                {b.running && (
                  <span className="text-[11px] text-caution font-normal">(running)</span>
                )}
              </button>
            );
          })}
        </div>

        {/* Guided Body */}
        <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-5">
          {/* Active Browser Summary Card */}
          <div className="flex items-center justify-between rounded-hw border border-edge bg-panel/60 p-3">
            <div className="flex items-center gap-3">
              <Lamp
                fill={onlineClients.length ? "bg-clear" : "bg-caution"}
                size={8}
              />
              <div>
                <p className="text-[12px] font-semibold text-ink">
                  {meta.label}{" "}
                  <span className="font-normal text-ink-dim">
                    ({isGecko ? "Gecko Engine" : "Chromium Engine"})
                  </span>
                </p>
                <p className="text-[11px] text-ink-dim">
                  {installed
                    ? onlineClients.length ? "Extension connected and reporting its version." : "Configuration found; waiting for an extension connection."
                    : "Extension needs to be loaded in this browser."}
                </p>
              </div>
            </div>

            {installed && (
              <span className="rounded-hw border border-clear/40 bg-clear/10 px-2 py-0.5 text-[11px] font-mono text-clear font-semibold">
                {onlineClients.length ? "CONNECTED" : "CONFIGURED"}
              </span>
            )}
          </div>

          {/* Action Message / Feedback */}
          {actionMessage && (
            <div
              role="alert"
              className="rounded-hw border border-route/40 bg-route/10 p-3 text-[12px] leading-relaxed text-ink"
            >
              {actionMessage}
            </div>
          )}

          {installed && <section aria-label="Installed extension controls" className="flex flex-col gap-4">
            <dl className="grid grid-cols-2 gap-x-5 gap-y-3 text-[12px]">
              <div><dt className="text-[11px] text-ink-dim">{onlineClients.length ? "Reported version" : "Last reported version"}</dt><dd className="mt-1 font-mono text-ink">{updateRow?.versions.length ? `v${updateRow.versions.join(", v")}` : "Not yet reported"}</dd></div>
              <div><dt className="text-[11px] text-ink-dim">Available package</dt><dd className="mt-1 font-mono text-ink">{updateRow?.availableVersion ? `v${updateRow.availableVersion}` : isGecko ? "Signed package unavailable" : "Unknown"}</dd></div>
              <div><dt className="text-[11px] text-ink-dim">Browser</dt><dd className="mt-1 text-ink">{currentBrowser?.running ? "Running" : "Not running"}</dd></div>
              <div><dt className="text-[11px] text-ink-dim">Connected profiles</dt><dd className="mt-1 text-ink">{onlineClients.length}</dd></div>
            </dl>
            <p role="status" className="border-y border-edge py-3 text-[12px] leading-relaxed text-ink-dim">
              {pendingUpdate ? `Update to v${pendingUpdate.targetVersion}: ${pendingUpdate.phase === "needs_attention" ? "confirmation needed" : "waiting for browser confirmation"}.`
                : updateRow?.needsSignedPackage ? "A newer extension requires a matching Mozilla-signed package. Manage signed packages in Extension settings."
                : updateRow?.updateAvailable ? `Extension v${updateRow.availableVersion} is available for this browser.`
                : updateRow?.verifiedCurrent ? "Connected profiles are up to date."
                : "Open this browser with the extension enabled, then refresh status to verify its version."}
            </p>
            <div className="flex flex-wrap gap-2">
              {updateRow?.updateAvailable && !updateRow.needsSignedPackage && <Button disabled={operationBusy || Boolean(pendingUpdate)} onClick={() => void manage("update")}>Update extension</Button>}
              {updateRow?.needsSignedPackage && <Button tone="ghost" disabled={operationBusy} onClick={onOpenSettings}>Manage signed package</Button>}
              <Button tone="ghost" disabled={operationBusy} onClick={() => void manage("reload")}>Reload extension</Button>
              <Button tone="ghost" disabled={operationBusy} onClick={() => void manage("manager")}>Open extension manager</Button>
              <Button tone="ghost" disabled={operationBusy} onClick={() => void manage("refresh")}>{busy ? "Working…" : "Refresh status"}</Button>
              <Button tone="ghost" disabled={operationBusy} onClick={() => isGecko ? setActionMessage(`Save your work, close all ${meta.label} windows, then reopen the browser. Automatic restart is available for supported Chromium browsers.`) : setConfirmAction("restart")}>{isGecko ? "Restart guidance" : "Restart browser"}</Button>
              <Button tone="ghost" disabled={operationBusy} onClick={() => setConfirmAction("reinstall")}>Repair / reinstall</Button>
            </div>
            {confirmAction && <div role="group" aria-label="Confirm browser action" className="border border-edge p-3 text-[12px]">
              <p className="mb-3 leading-relaxed text-ink-dim">{confirmAction === "restart" ? `Restart ${meta.label} with the extension loaded? Save unfinished browser work first.` : `Reapply the bundled extension files and pairing configuration for ${meta.label}? ${isGecko ? "The browser may need an add-on confirmation or restart." : "This may restart the browser. Save unfinished work first."}`}</p>
              <div className="flex gap-2"><Button disabled={operationBusy} onClick={() => void manage(confirmAction)}>Confirm {confirmAction === "restart" ? "restart" : "repair"}</Button><Button tone="ghost" onClick={() => setConfirmAction(null)}>Cancel</Button></div>
            </div>}
            {updateRow?.clients.length ? <details className="border-t border-edge pt-3 text-[12px]"><summary className="focus-ring cursor-pointer text-ink">Profile connection details</summary><ul className="mt-3 space-y-2 text-ink-dim">{updateRow.clients.map((client,index) => <li key={client.clientId}>Connection {index+1}: v{client.extensionVersion || "unknown"} · {client.online ? "online" : "offline"}{client.active ? " · active capture source" : ""}{client.temporary ? " · temporary install" : ""}</li>)}</ul></details> : null}
            {!isGecko && extensionPath && <details className="border-t border-edge pt-3 text-[12px]"><summary className="focus-ring cursor-pointer text-ink">Extension files</summary><p className="my-3 break-all font-mono text-[11px] text-ink-dim">{extensionPath}</p><div className="flex flex-wrap gap-2"><Button tone="ghost" onClick={copyPath}>{pathCopied ? "Path copied" : "Copy folder path"}</Button><Button tone="ghost" disabled={operationBusy} onClick={() => void engine.revealFolder(extensionPath).catch(error => setActionMessage(String(error)))}>Open extension folder</Button></div></details>}
          </section>}

          {/* --- SCENARIO 1: GECKO (Zen Browser & Mozilla Firefox) --- */}
          {!installed && isGecko && (
            <div className="flex flex-col gap-4">
              <div className="flex flex-col gap-1.5 border-l-2 border-route pl-3.5">
                <span className="legend text-[11px] text-route font-semibold">
                  AUTOMATIC ENTERPRISE POLICY SETUP
                </span>
                <p className="text-[12px] leading-relaxed text-ink">
                  Zoltraak bundles a Mozilla-signed extension (<code>zoltraak-signed.xpi</code>). 
                  Clicking the install button writes a browser policy into{" "}
                  <code className="text-[11px] font-mono bg-recess px-1 py-0.5 rounded-hw">
                    distribution/policies.json
                  </code>{" "}
                  so the browser automatically installs and updates it on launch.
                </p>
              </div>

              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                {/* Step 1 */}
                <div className="flex flex-col justify-between gap-3 rounded-hw border border-edge bg-face p-3.5">
                  <div className="flex flex-col gap-1">
                    <span className="legend text-[11px] text-ink-dim uppercase">Step 1</span>
                    <p className="text-[12px] font-semibold text-ink">Write Policy & Register Host</p>
                    <p className="text-[11px] leading-relaxed text-ink-dim">
                      Deploys the signed package configuration and Windows native messaging pairing host.
                    </p>
                  </div>
                  <button
                    type="button"
                    onClick={handleInstallGecko}
                    disabled={operationBusy}
                    className="focus-ring flex items-center justify-center gap-2 rounded-hw bg-route px-3 py-2 text-[11px] font-semibold text-on-signal transition-colors enabled:hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-50"
                  >
                    <Plug className={`size-3.5 ${busy ? "animate-spin" : ""}`} />
                    <span>{busy ? "Deploying..." : "⚡ Auto-Install Extension"}</span>
                  </button>
                </div>

                {/* Step 2 */}
                <div className="flex flex-col justify-between gap-3 rounded-hw border border-edge bg-face p-3.5">
                  <div className="flex flex-col gap-1">
                    <span className="legend text-[11px] text-ink-dim uppercase">Step 2</span>
                    <p className="text-[12px] font-semibold text-ink">Restart {meta.label}</p>
                    <p className="text-[11px] leading-relaxed text-ink-dim">
                      Close and reopen {meta.label}. The extension will appear in your toolbar and
                      pairs itself silently over native messaging—no tokens to paste!
                    </p>
                  </div>
                  <div className="flex items-center gap-2 rounded-hw border border-edge/80 bg-panel px-2.5 py-1.5 text-[11px] text-ink-dim">
                    <span className="size-1.5 rounded-full bg-clear" />
                    <span>Zero manual token entry required</span>
                  </div>
                </div>
              </div>

              <div className="rounded-hw border border-edge/80 bg-panel/50 p-2.5 text-[11px] leading-relaxed text-ink-dim">
                <span className="font-semibold text-ink">Note for Program Files:</span> If {meta.label} was
                installed to <code>C:\Program Files</code> instead of your user profile, run Zoltraak as
                Administrator once so it can write the policy file.
              </div>
            </div>
          )}

          {/* --- SCENARIO 2: CHROMIUM (Edge, Brave, Opera, Vivaldi, Chrome) --- */}
          {!installed && !isGecko && (
            <div className="flex flex-col gap-4">
              <div className="flex flex-col gap-1.5 border-l-2 border-route pl-3.5">
                <span className="legend text-[11px] text-route font-semibold">
                  CHROMIUM UNPACKED LOADING
                </span>
                <p className="text-[12px] leading-relaxed text-ink">
                  Zoltraak prepares the unpacked extension in your local AppData directory with the
                  pairing token already baked in. You can launch it with 1 click, or load it once
                  in Developer mode for permanent persistence.
                </p>
              </div>

              {/* Folder Path Copy Bar */}
              <div className="flex flex-col gap-1.5 rounded-hw border border-edge bg-face p-3">
                <div className="flex items-center justify-between">
                  <Legend className="text-[11px] text-ink-dim">Extension Directory Path</Legend>
                  <button
                    type="button"
                    onClick={copyPath}
                    disabled={!extensionPath}
                    className="focus-ring flex items-center gap-1.5 rounded-hw border border-edge bg-panel px-2 py-0.8 text-[11px] text-ink hover:bg-recess transition-colors"
                  >
                    {pathCopied ? (
                      <>
                        <Check className="size-3 text-clear" />
                        <span className="text-clear font-semibold">Copied!</span>
                      </>
                    ) : (
                      <>
                        <Copy className="size-3 text-ink-faint" />
                        <span>Copy Folder Path</span>
                      </>
                    )}
                  </button>
                </div>
                <div
                  data-selectable
                  onClick={copyPath}
                  className="describer recess flex cursor-pointer items-center justify-between rounded-hw px-2.5 py-1.5 text-[11px] text-ink transition-colors hover:bg-sunk"
                >
                  <span className="truncate">{extensionPath ?? "Resolving path..."}</span>
                  <FolderOpen className="size-3.5 shrink-0 text-ink-faint ml-2" />
                </div>
              </div>

              {/* Two Methods: 1-Click Relaunch vs Permanent Developer Mode */}
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                {/* Method A: One-Click Relaunch (for Edge, Brave, Opera, Vivaldi) */}
                {selectedId !== "chrome" && (
                  <div className="flex flex-col justify-between gap-3 rounded-hw border border-edge bg-face p-3.5">
                    <div className="flex flex-col gap-1">
                      <span className="legend text-[11px] text-route font-semibold uppercase">
                        Option 1 · Instant
                      </span>
                      <p className="text-[12px] font-semibold text-ink">1-Click Auto-Relaunch</p>
                      <p className="text-[11px] leading-relaxed text-ink-dim">
                        Gracefully restarts {meta.label} with the extension loaded and automatically
                        restores your open tabs.
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={handleRelaunchChromium}
                      disabled={operationBusy}
                      className="focus-ring flex items-center justify-center gap-2 rounded-hw bg-route px-3 py-2 text-[11px] font-semibold text-white transition-colors hover:bg-route/90 disabled:opacity-50"
                    >
                      <Reload className={`size-3.5 ${busy ? "animate-spin" : ""}`} />
                      <span>{busy ? "Relaunching..." : `🚀 Relaunch ${meta.label}`}</span>
                    </button>
                  </div>
                )}

                {/* Method B: Permanent Load Unpacked */}
                <div
                  className={`flex flex-col justify-between gap-3 rounded-hw border border-edge bg-face p-3.5 ${
                    selectedId === "chrome" ? "sm:col-span-2" : ""
                  }`}
                >
                  <div className="flex flex-col gap-1">
                    <span className="legend text-[11px] text-clear font-semibold uppercase">
                      {selectedId === "chrome" ? "Standard Installation" : "Option 2 · Permanent"}
                    </span>
                    <p className="text-[12px] font-semibold text-ink">Load Unpacked in Browser</p>
                    <p className="text-[11px] leading-relaxed text-ink-dim">
                      {selectedId === "chrome"
                        ? "Google Chrome 137+ requires Developer mode for local extensions until the Chrome Web Store release is approved."
                        : "Persists across all future browser sessions, even when opening from desktop shortcuts."}
                    </p>
                  </div>

                  <div className="flex flex-col gap-1.5 pt-1 text-[11px] text-ink-dim">
                    <div className="flex items-start gap-2">
                      <span className="flex size-4 shrink-0 items-center justify-center rounded-full bg-recess text-[11px] font-bold text-ink">
                        1
                      </span>
                      <span>
                        Navigate to{" "}
                        <button
                          type="button"
                          onClick={copySettingsUrl}
                          className="font-mono text-route underline hover:text-ink"
                          title="Click to copy URL"
                        >
                          {meta.settingsUrl}
                        </button>{" "}
                        {urlCopied && <span className="text-clear text-[11px]">(copied)</span>}
                      </span>
                    </div>

                    <div className="flex items-start gap-2">
                      <span className="flex size-4 shrink-0 items-center justify-center rounded-full bg-recess text-[11px] font-bold text-ink">
                        2
                      </span>
                      <span>
                        Toggle <strong className="text-ink">Developer mode</strong> in the top-right
                        corner.
                      </span>
                    </div>

                    <div className="flex items-start gap-2">
                      <span className="flex size-4 shrink-0 items-center justify-center rounded-full bg-recess text-[11px] font-bold text-ink">
                        3
                      </span>
                      <span>
                        Click <strong className="text-ink">Load unpacked</strong> (top-left) and paste
                        the folder path (<kbd className="font-mono text-ink">Ctrl+V</kbd>).
                      </span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          )}
        </div>

        {/* Footer Hardware Plate: API Status & Quick Actions */}
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-t border-edge-strong bg-recess/40 px-5 py-3">
          <div className="flex items-center gap-4">
            <div className="flex items-center gap-2">
              <Lamp fill={status?.listening ? "bg-clear" : null} size={7} />
              <Legend className="text-[11px]">
                {status?.listening ? `127.0.0.1:${status.port}` : "api closed"}
              </Legend>
            </div>

            {status?.token && (
              <button
                type="button"
                onClick={copyToken}
                title="Copy API pairing token"
                className="focus-ring flex items-center gap-1.5 rounded-hw border border-edge bg-face px-2 py-1 text-[11px] text-ink hover:bg-panel transition-colors"
              >
                {tokenCopied ? (
                  <Check className="size-3 text-clear" />
                ) : (
                  <Copy className="size-3 text-ink-faint" />
                )}
                <span>{tokenCopied ? "Token Copied" : "Copy Token"}</span>
              </button>
            )}
          </div>

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onClose}
              className="focus-ring rounded-hw border border-edge-strong bg-face px-4 py-1.5 text-[11px] font-semibold text-ink hover:bg-recess transition-colors"
            >
              Done
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
