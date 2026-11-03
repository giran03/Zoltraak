import { useEffect, useId, useRef, useState } from "react";
import { engine, type DiscordSettings, type DiscordStatus, type DiscordMode, type DiscordBotProfile } from "../lib/engine";
import { profileImage } from "../lib/discordProfile";
import type { QueueItem } from "../lib/snapshots";
import { humanBytes } from "../lib/useDiscord";
import { Button, ToggleRow } from "./Controls";
import { Close, Discord, DownloadGlyph } from "./Icons";

function SenderImage({ src, label, banner = false }: { src: string | null; label: string; banner?: boolean }) {
  const [broken, setBroken] = useState(false);
  useEffect(() => setBroken(false), [src]);
  if (!src || broken) return banner ? null : <span className="flex size-10 shrink-0 items-center justify-center rounded-full bg-recess text-ink-dim"><Discord className="size-5" /></span>;
  return <img src={src} alt={label} referrerPolicy="no-referrer" onError={() => setBroken(true)}
    className={banner ? "mb-4 aspect-[5/2] w-full rounded-hw object-cover" : "size-10 shrink-0 rounded-full object-cover"} />;
}

export function DiscordSendDialog({ items, onClose, onSettings, onQueued }: {
  items: QueueItem[]; onClose: () => void; onSettings: () => void; onQueued: (count: number) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const submitting = useRef(false);
  const changingMode = useRef(false);
  const title = useId();
  const captionId = useId();
  const [settings, setSettings] = useState<DiscordSettings | null>(null);
  const [status, setStatus] = useState<DiscordStatus | null>(null);
  const [caption, setCaption] = useState("");
  const [ping, setPing] = useState(false);
  const [sourceLinks, setSourceLinks] = useState(false);
  const [compress, setCompress] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [switching, setSwitching] = useState(false);
  const [profile, setProfile] = useState<DiscordBotProfile | null>(null);
  const [serverProfile, setServerProfile] = useState<DiscordBotProfile | null>(null);
  const [profileUnavailable, setProfileUnavailable] = useState(false);
  const mounted = useRef(true);
  const eligible = items.filter(item => item.state === "finished").map(item => ({ ...item, filename: item.path?.split(/[\\/]/).pop() || item.filename }));
  const sources = [...new Set(eligible.map(item => item.url).filter(url => /^https?:\/\//.test(url)))];
  const total = eligible.reduce((sum, item) => sum + (item.totalBytes ?? item.downloadedBytes), 0);
  const mention = settings?.discordUserId ? `<@${settings.discordUserId}>` : "";
  const pingText = (settings?.pingMessageTemplate ?? "{discord user id}").replaceAll("{discord user id}", mention).replaceAll("{discord_user_id}", mention);
  const pingLine = pingText.includes(mention) && mention ? pingText : pingText.trim() ? `${pingText} ${mention}` : mention;
  const renderedLength = [...caption].length + (ping && mention ? [...pingLine].length + 1 : 0)
    + (sourceLinks ? sources.reduce((n, url) => n + [...url].length + 1, 0) : 0);
  useEffect(() => {
    mounted.current = true;
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    const frame = requestAnimationFrame(() => element.classList.add("is-open"));
    let alive = true;
    void Promise.all([engine.discordSettings(), engine.discordStatus()]).then(([config, connection]) => {
      if (!alive) return;
      setSettings(config); setStatus(connection);
      const captions = eligible.map(item => (config.messageTemplate ?? "").replaceAll("{filename}", item.filename)
        .replaceAll("{size}", humanBytes(item.totalBytes ?? item.downloadedBytes)).replaceAll("{url}", item.url)
        .replaceAll("{host}", "").replaceAll("{category}", "").trim());
      setCaption([...new Set(captions)].filter(Boolean).join("\n"));
      setPing(!!config.pingSender && !!config.discordUserId);
      setSourceLinks(config.includeSourceUrl && sources.length > 0); setCompress(config.compressToFit);
    }).catch(e => { if (alive) setError(String(e)); });
    return () => { alive = false; mounted.current = false; cancelAnimationFrame(frame); element.close(); previous?.focus(); };
    // A fresh dialog instance owns each submission and its initial draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    let alive = true;
    setProfile(null); setServerProfile(null); setProfileUnavailable(false);
    if (settings?.mode !== "bot" || !status?.hasCredential) return;
    void Promise.allSettled([
      engine.discordBotProfile("global"),
      settings.guildId ? engine.discordBotProfile("server", settings.guildId) : Promise.resolve(null),
    ]).then(([global, member]) => {
      if (!alive) return;
      if (global.status === "fulfilled") setProfile(global.value);
      else setProfileUnavailable(true);
      if (member.status === "fulfilled") setServerProfile(member.value);
    });
    return () => { alive = false; };
  }, [settings?.mode, settings?.guildId, status?.hasCredential]);
  const configured = !!settings?.enabled && !!status?.connected && !!status.destinationRef;
  const locked = busy || switching;
  const switchMode = async (mode: DiscordMode) => {
    if (!settings || mode === settings.mode || submitting.current || changingMode.current) return;
    changingMode.current = true; setSwitching(true); setError(""); setStatus(null);
    try {
      const current = await engine.discordSettings();
      const next = await engine.updateDiscordSettings({ ...current, mode });
      if (!mounted.current) return;
      setSettings(next);
      const connection = await engine.discordStatus();
      if (!mounted.current) return;
      setStatus(connection);
      if (!next.discordUserId) setPing(false);
    } catch (e) {
      if (mounted.current) setError(`${e instanceof Error ? e.message : String(e)} Open Discord settings to review the active method.`);
    } finally {
      changingMode.current = false;
      if (mounted.current) setSwitching(false);
    }
  };
  const senderName = settings?.mode === "webhook" ? settings.webhookUsername || status?.botUsername || "Webhook sender"
    : serverProfile?.nick || profile?.username || status?.botUsername || "Discord bot";
  const effectiveProfile = { ...serverProfile, id: profile?.id };
  const avatar = settings?.mode === "webhook" ? settings.webhookAvatarUrl ?? null
    : serverProfile && profile?.id && settings?.guildId ? profileImage("server", effectiveProfile, "avatar", settings.guildId) ?? (profile ? profileImage("global", profile, "avatar") : null)
    : profile ? profileImage("global", profile, "avatar") : null;
  const banner = settings?.mode === "bot" ? (serverProfile && profile?.id && settings.guildId ? profileImage("server", effectiveProfile, "banner", settings.guildId) : null)
    ?? (profile ? profileImage("global", profile, "banner") : null) : null;
  const previewMessage = [ping && mention ? pingLine : "", caption, ...(sourceLinks ? sources : [])].filter(Boolean).join("\n");
  const connectionLabel = switching ? "Checking configuration…" : !settings || !status ? error ? "Configuration unavailable" : "Loading configuration…"
    : !settings.enabled ? "Delivery off" : configured ? "Configured" : "Not configured";
  const submit = async () => {
    if (submitting.current || changingMode.current || !configured || !eligible.length || renderedLength > 2000) return;
    submitting.current = true; setBusy(true); setError("");
    try {
      const [next, connection] = await Promise.all([engine.discordSettings(), engine.discordStatus()]);
      if (connection.destinationRef !== status?.destinationRef || next.mode !== settings?.mode || next.discordUserId !== settings?.discordUserId || next.pingMessageTemplate !== settings?.pingMessageTemplate) {
        setSettings(next); setStatus(connection); if (!next.discordUserId) setPing(false);
        setError("Discord destination or ping settings changed. Review the updated details and queue again."); return;
      }
      setSettings(next); setStatus(connection);
      if (!next.enabled || !connection.connected) throw new Error("Enable and configure Discord delivery in Discord settings first.");
      const count = await engine.discordSend(eligible.map(item => item.id), { caption, pingSender: ping,
        includeSourceUrl: sourceLinks, compressToFit: compress, expectedDestination: connection.destinationRef ?? undefined });
      onQueued(count);
    } catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    finally { submitting.current = false; setBusy(false); }
  };
  return <dialog ref={dialog} aria-labelledby={title} className="shortcut-dialog discord-send-dialog t-modal"
    onCancel={event => { if (submitting.current || changingMode.current) event.preventDefault(); }} onClose={onClose}>
    <div className="flex min-h-0 flex-col">
      <header className="flex items-start gap-3 border-b border-edge px-5 py-4 sm:px-6">
        <Discord className="mt-1 size-5 shrink-0 text-route" />
        <div className="min-w-0 flex-1"><h2 id={title} className="text-[18px] font-medium text-ink">Send to Discord</h2>
          <p className="mt-1 text-[12px] text-ink-dim">{eligible.length} {eligible.length === 1 ? "attachment" : "attachments"} · {humanBytes(total)}</p></div>
        <button type="button" aria-label="Close send dialog" disabled={locked} onClick={onClose} className="focus-ring p-1 text-ink-dim hover:text-ink disabled:opacity-45"><Close className="size-4" /></button>
      </header>
      <div className="grid min-h-0 gap-6 overflow-y-auto px-5 py-4 sm:px-6 md:grid-cols-[minmax(0,1fr)_minmax(0,0.9fr)]">
        <div className="min-w-0">
          <div role="group" aria-label="Delivery method" className="flex gap-1 rounded-hw border border-edge-strong bg-recess p-1">
            {([{ mode: "webhook", label: "Webhook URL" }, { mode: "bot", label: "Discord bot" }] as const).map(method => (
              <button key={method.mode} type="button" aria-pressed={settings?.mode === method.mode}
                disabled={locked || !settings} onClick={() => void switchMode(method.mode)}
                className={`focus-ring min-h-9 min-w-0 flex-1 rounded-hw-sm px-2 text-[12px] disabled:opacity-45 ${settings?.mode === method.mode ? "bg-face font-medium text-ink shadow-sm" : "text-ink-dim hover:bg-face/60"}`}>{method.label}</button>
            ))}
          </div>
          <p className="mt-2 text-[11px] text-ink-dim">Changes the app’s active delivery method. Your message draft is preserved.</p>
          <div className="mt-4 flex flex-wrap items-center justify-between gap-2">
            <p role="status" className={`text-[12px] font-medium ${configured ? "text-clear" : "text-caution"}`}>{connectionLabel}</p>
            <Button tone="ghost" disabled={locked} onClick={onSettings}>Discord settings</Button>
          </div>
          <div className="min-w-0 space-y-1 text-[12px] text-ink-dim">
            {status && <><p className="break-words">{status.guildName ? `${status.guildName} / ` : ""}{status.channelName ? `#${status.channelName}` : status.channelId ? `Channel ${status.channelId}` : "Destination not selected"}</p>
              <p>Upload limit: {(status.uploadLimitBytes / 1_000_000).toLocaleString(undefined, { maximumFractionDigits: 2 })} MB per file</p>
              {!status.hasCredential && <p className="text-caution">Save a {settings?.mode === "bot" ? "bot token" : "webhook URL"} in Discord settings.</p>}
              {status.lastError && <p className="break-words text-caution">{status.lastError}</p>}</>}
          </div>
        {items.length > eligible.length && <p className="mt-3 text-[12px] text-caution">{items.length - eligible.length} unfinished entries excluded.</p>}
        <ul aria-label="Attachments to send" className="my-4 max-h-36 overflow-y-auto divide-y divide-edge border-y border-edge">
          {eligible.map(item => <li key={item.id} className="flex items-center justify-between gap-4 py-2 text-[12px]"><span className="truncate text-ink" title={item.filename}>{item.filename}</span><span className="shrink-0 font-mono text-ink-dim">{humanBytes(item.totalBytes ?? item.downloadedBytes)}</span></li>)}
        </ul>
        <label htmlFor={captionId} className="legend text-[11px] text-ink-dim">Message (optional)</label>
        <textarea id={captionId} rows={3} autoComplete="off" value={caption} disabled={locked} onChange={e => setCaption(e.target.value)} aria-describedby={`${captionId}-count`}
          className="recess focus-ring mt-2 w-full resize-y rounded-hw px-3 py-2 text-[13px] text-ink placeholder:text-ink-faint" placeholder="Add a message for these files…" />
        <p id={`${captionId}-count`} className={`mt-1 text-right font-mono text-[11px] ${renderedLength > 2000 ? "text-danger" : "text-ink-dim"}`}>{renderedLength} / 2000 including mentions and links</p>
        <div className="mt-4">
          <ToggleRow title="Ping me" description={settings?.discordUserId ? `Message: ${pingLine}` : "Set your Discord user ID in Discord settings to enable mentions."} checked={ping} disabled={locked || !settings?.discordUserId} onChange={setPing} />
          <ToggleRow title="Include source links" description={sources.length ? "Add the original source URLs to the message." : "No source links are available for these files."} checked={sourceLinks} disabled={locked || !sources.length} onChange={setSourceLinks} />
          {sourceLinks && <ul className="max-h-20 overflow-y-auto text-[11px] text-ink-dim">{sources.map(url => <li key={url} className="break-all py-1">{url}</li>)}</ul>}
          <ToggleRow title="Compress oversized media" description="Prepare a smaller copy when needed. Your downloaded originals stay unchanged." checked={compress} disabled={locked} onChange={setCompress} />
        </div>
        {settings && !configured && <p className="mt-3 text-[12px] text-caution">{!settings.enabled ? "Discord delivery is switched off." : "Discord is not configured for this method."} Open Discord settings to configure it.</p>}
        {!eligible.length && <p className="mt-3 text-[12px] text-caution">No finished entries are available to send.</p>}
        {error && <p role="alert" className="mt-3 border border-danger/40 bg-danger/10 px-3 py-2 text-[12px] text-danger">{error}</p>}
        </div>
        <aside aria-label="Discord message preview" className="min-w-0 border-t border-edge pt-5 md:border-l md:border-t-0 md:pl-6 md:pt-0">
          <h3 className="text-[13px] font-medium text-ink">Message preview</h3>
          <p className="mb-4 mt-1 text-[11px] leading-relaxed text-ink-dim">An approximation of your post. Discord may resize attachments and render formatting differently.</p>
          <SenderImage src={banner} label={`${senderName} profile banner`} banner />
          <div className="flex items-start gap-3">
            <SenderImage src={avatar} label={`${senderName} avatar`} />
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <p className="break-words text-[13px] font-semibold text-ink">{senderName}</p>
                <span className="rounded-hw bg-route/10 px-1.5 py-0.5 text-[11px] font-medium text-route">APP</span>
              </div>
              <p className="mt-1 text-[11px] text-ink-dim">{settings?.mode === "bot" ? "Bot profile" : "Webhook sender"}{status?.channelName ? ` · #${status.channelName}` : ""}</p>
              <div data-message-preview className="mt-3 whitespace-pre-wrap break-words text-[12px] leading-relaxed text-ink">
                {previewMessage ? mention ? previewMessage.replaceAll(mention, "@you") : previewMessage : <span className="text-ink-faint">Your attachments will appear here.</span>}
              </div>
              <ul aria-label="Attachment preview" className="mt-3 space-y-2">
                {eligible.map(item => <li key={item.id} className="overflow-hidden rounded-hw-lg border border-edge bg-recess">
                  {item.thumbnail && <img src={item.thumbnail} alt={`Preview of ${item.filename}`} referrerPolicy="no-referrer" className="max-h-40 w-full object-contain" onError={event => { event.currentTarget.style.display = "none"; }} />}
                  <div className="flex items-start gap-2 p-3"><DownloadGlyph className="mt-0.5 size-4 shrink-0 text-ink-dim" /><div className="min-w-0">
                    <p className="break-all text-[12px] font-medium text-ink">{item.filename}</p>
                    <p className="mt-1 text-[11px] text-ink-dim">{humanBytes(item.totalBytes ?? item.downloadedBytes)}{status && (item.totalBytes ?? item.downloadedBytes) > status.uploadLimitBytes ? compress && (item.isVideo || item.isMusic) ? " · Compression needed" : " · Over upload limit" : ""}</p>
                  </div></div>
                </li>)}
              </ul>
            </div>
          </div>
          {profileUnavailable && <p className="mt-3 text-[11px] text-ink-dim">Bot profile unavailable. Delivery configuration is checked separately.</p>}
          {banner && <p className="mt-3 text-[11px] text-ink-dim">The banner belongs to the sender’s profile; it is not included in the message.</p>}
        </aside>
      </div>
      <footer className="flex shrink-0 justify-end gap-2 border-t border-edge px-5 py-4 sm:px-6">
        <Button tone="ghost" disabled={locked} onClick={onClose}>Cancel</Button>
        <Button disabled={locked || !configured || !eligible.length || renderedLength > 2000} onClick={() => void submit()}>{busy ? "Queuing…" : "Queue for Discord"}</Button>
      </footer>
    </div>
  </dialog>;
}
