import { DiscordSenderSettings } from "./DiscordSenderSettings";
import { DiscordBotSettings, DiscordMessagePreview } from "./DiscordBotSettings";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button, ErrorStrip, Field, NumberInput, Section, SelectInput, Switch, TextInput, ToggleRow } from "./Controls";
import { ArrowUpRight, Discord } from "./Icons";
import { Lamp, Legend } from "./Panel";
import {
  type Delivery,
  type DeliveryState,
  type DiscordChannel,
  type DiscordGuild,
  type DiscordMode,
  engine,
  hasEngine,
  openDiscordPortal,
  openDiscordMessage,
} from "../lib/engine";
import type { AddToast } from "../lib/useToasts";
import { humanBytes, useDiscord } from "../lib/useDiscord";

/**
 * Discord delivery.
 *
 * Two things about Discord shape this whole screen. First, there is no way to
 * post as the signed-in user — no OAuth scope grants it, and the only thing
 * that would is a user account token, which is against Discord's terms — so
 * the choice is a webhook or a bot the user owns. Second, the upload ceiling
 * belongs to the *server* and is small, so the size readout and the
 * fit-to-limit lever are not an advanced option tucked away somewhere; they
 * are most of what makes the feature usable.
 */

const STATE_LAMP: Record<DeliveryState, string | null> = {
  pending: null,
  preparing: "bg-caution",
  uploading: "bg-route",
  sent: "bg-clear",
  deleted: null,
  skipped: null,
  failed: "bg-danger",
};

type DeleteFeedback = { id: string; messageUrl: string | null; kind: "progress" | "success" | "error"; message: string };
function DeliveryRow({ delivery, onRetry, onDelete, deleting, feedback, groupedCount }: { delivery: Delivery; onRetry: () => void; onDelete: () => void; deleting: boolean; feedback?: DeleteFeedback; groupedCount: number }) {
  const [confirming, setConfirming] = useState(false);
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState<string | null>(null);
  const rowRef = useRef<HTMLDivElement>(null);
  const deleteRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (confirming) confirmRef.current?.focus(); }, [confirming]);
  const resized = delivery.sentBytes !== null && delivery.sentBytes !== delivery.bytes;
  return (
    <div ref={rowRef} tabIndex={-1} className="focus-ring flex items-start justify-between gap-4 border-b border-edge py-2.5 last:border-b-0">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <Lamp fill={STATE_LAMP[delivery.state]} />
          <p className="truncate text-[13px] text-ink" title={delivery.filename}>
            {delivery.filename}
          </p>
        </div>
        <p className="mt-0.5 text-[12px] leading-relaxed text-ink-dim">
          <span className="legend text-[11px]">{delivery.state}</span>
          {" · "}
          {resized
            ? `${humanBytes(delivery.bytes)} → ${humanBytes(delivery.sentBytes as number)}`
            : humanBytes(delivery.bytes)}
          {delivery.reason ? ` · ${delivery.reason}` : ""}
        </p>
        {feedback && <p role={feedback.kind === "error" ? "alert" : "status"} aria-live="polite" className={`mt-1 text-[12px] ${feedback.kind === "error" ? "text-danger" : "text-ink-dim"}`}>{feedback.message}</p>}
        {openError && <p role="alert" className="mt-1 text-[12px] text-danger">{openError}</p>}
        {confirming && delivery.state === "sent" && !deleting && (
          <div className="mt-2 border border-edge bg-recess p-3" onKeyDown={event => {if (event.key === "Escape") {event.preventDefault();setConfirming(false);deleteRef.current?.focus();}}}>
            <p className="text-[12px] leading-relaxed text-ink">Delete this Discord message and all its attachments? {groupedCount} file{groupedCount === 1 ? " is" : "s are"} shown in this history. Downloaded files will be preserved.</p>
            <div className="mt-2 flex flex-wrap gap-3">
              <button ref={confirmRef} type="button" className="focus-ring legend text-[11px] text-danger" onClick={() => {rowRef.current?.focus({preventScroll:true});setConfirming(false);onDelete();}}>Delete message</button>
              <button type="button" className="focus-ring legend text-[11px] text-ink-dim" onClick={() => {setConfirming(false);deleteRef.current?.focus();}}>Cancel</button>
            </div>
          </div>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {delivery.messageUrl && delivery.state !== "deleted" && (
          <button
            type="button"
            disabled={opening}
            title="Open in Discord if it is running, otherwise in your browser"
            onClick={() => {
              setOpening(true); setOpenError(null);
              void openDiscordMessage(delivery.messageUrl!).catch(e => setOpenError(e instanceof Error ? e.message : String(e))).finally(() => setOpening(false));
            }}
            className="focus-ring legend text-[11px] text-ink-dim transition-colors hover:text-ink"
          >
            Open
          </button>
        )}
        {delivery.state === "sent" && delivery.messageUrl && (
          <button
            type="button"
            disabled={deleting}
            ref={deleteRef}
            onClick={() => setConfirming(true)}
            className="focus-ring legend text-[11px] text-danger/70 transition-colors hover:text-danger"
          >
            {deleting && feedback?.messageUrl === delivery.messageUrl ? "Deleting..." : "Delete"}
          </button>
        )}
        {["failed", "skipped"].includes(delivery.state) && (
          <button
            type="button"
            onClick={onRetry}
            className="focus-ring legend text-[11px] text-ink-dim transition-colors hover:text-ink"
          >
            Retry
          </button>
        )}
      </div>
    </div>
  );
}

export function DiscordSettings({ active = true, addToast }: { active?: boolean; addToast?: AddToast }) {
  const { status, settings, deliveries, loading, error, setError, refresh, commit } = useDiscord(active);
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const reconnecting = useRef(false);
  const deleteInFlight = useRef(false);
  const [deleteFeedback, setDeleteFeedback] = useState<DeleteFeedback | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [inviteUrl, setInviteUrl] = useState<string | null>(null);
  const [guilds, setGuilds] = useState<DiscordGuild[]>([]);
  const [channels, setChannels] = useState<DiscordChannel[]>([]);
  const [uploadLimitDraft, setUploadLimitDraft] = useState<string | null>(null);
  /** Set once the user starts replacing a stored credential. */
  const [replacing, setReplacing] = useState(false);
  const [guideOpen, setGuideOpen] = useState<boolean | null>(null);

  const mode: DiscordMode = settings?.mode ?? "webhook";
  const isBot = mode === "bot";
  useEffect(() => setUploadLimitDraft(null), [mode]);
  const stored = status?.hasCredential ?? false;
  const isGuideExpanded = guideOpen ?? !stored;

  /** Run an action, showing what it was doing if it fails. */
  const run = useCallback(
    async (label: string, action: () => Promise<string | null>) => {
      setBusy(label);
      setError(null);
      setNotice(null);
      try {
        const message = await action();
        if (message) setNotice(message);
        await refresh();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(null);
      }
    },
    [refresh, setError],
  );

  const deleteMessage = async (delivery: Delivery) => {
    if (deleteInFlight.current) return;
    deleteInFlight.current = true;setDeleting(true);
    setDeleteFeedback({id:delivery.id,messageUrl:delivery.messageUrl,kind:"progress",message:"Deleting message from Discord..."});
    try {
      const deleted = await engine.discordDeleteMessage(delivery.id);
      if (!deleted) throw new Error("Discord did not confirm deletion. Try again.");
      const message = "Message and its attachments deleted from Discord. Downloaded files were preserved.";
      setDeleteFeedback({id:delivery.id,messageUrl:delivery.messageUrl,kind:"success",message});
      addToast?.({id:`discord-delete-${delivery.id}`,type:"success",title:"Discord message deleted",message});
      await refresh();
    } catch(e) {
      const message = e instanceof Error ? e.message : String(e);
      setDeleteFeedback({id:delivery.id,messageUrl:delivery.messageUrl,kind:"error",message:`Could not delete message: ${message}`});
      addToast?.({id:`discord-delete-${delivery.id}`,type:"error",title:"Discord deletion failed",message});
    } finally {deleteInFlight.current=false;setDeleting(false);}
  };

  const loadGuilds = useCallback(async () => {
    await run("servers", async () => {
      setGuilds(await engine.discordGuilds());
      return null;
    });
  }, [run]);

  const loadChannels = useCallback(
    async (guildId: string) => {
      await run("channels", async () => {
        setChannels(await engine.discordChannels(guildId));
        return null;
      });
    },
    [run],
  );

  // Bot mode reopened with a server already chosen: fill the pickers so the
  // current target is visible without the user re-selecting it.
  useEffect(() => {
    if (isBot && stored && guilds.length === 0 && !loading) {
      void loadGuilds();
      if (settings?.guildId) void loadChannels(settings.guildId);
    }
    // Only on the transition into a connected bot; re-running on every render
    // would hammer Discord's rate limit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isBot, stored, loading]);

  if (!hasEngine()) {
    return (
      <div className="mx-auto w-full max-w-6xl px-4 py-5 sm:px-6">
        <Section title="Discord delivery" description="Discord delivery is only available in the desktop app." >
          <span />
        </Section>
      </div>
    );
  }

  if (loading || !settings || !status) {
    return (
      <div className="mx-auto w-full max-w-6xl px-4 py-5 sm:px-6">
        <p className="text-[12px] text-ink-faint">Reading Discord delivery state…</p>
      </div>
    );
  }

  const connectLabel = isBot ? "Bot token" : "Webhook URL";
  const showSecretField = !stored || replacing;
  const uploadLimitMb = uploadLimitDraft ?? String((isBot ? settings.botUploadLimitBytes ?? 20_000_000 : settings.uploadLimitBytes ?? 20_000_000) / 1_000_000);
  const uploadLimitValue = Number(uploadLimitMb);
  const validUploadLimit = uploadLimitMb.trim() !== "" && Number.isFinite(uploadLimitValue) && uploadLimitValue >= 1 && uploadLimitValue <= 1000;

  return (
    <div className="mx-auto w-full max-w-6xl px-4 py-5 sm:px-6">
      <header className="mb-7 grid gap-5 border-b border-edge-strong pb-5 lg:grid-cols-[minmax(0,1fr)_minmax(30rem,0.9fr)] lg:items-end">
        <div className="min-w-0">
          <div className="mb-2 flex items-center gap-2.5">
            <Discord className="size-5 text-route" />
            <h1 className="text-[20px] font-semibold leading-tight text-ink">Discord delivery</h1>
          </div>
          <p className="max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
            Route finished downloads to a channel, test the connection, and inspect every delivery attempt from one desk.
          </p>
        </div>

        <div className="grid grid-cols-3 border border-edge-strong bg-face">
          <div className="min-w-0 border-r border-edge px-3 py-2.5">
            <Legend>connection</Legend>
            <div className="mt-1.5 flex items-center gap-2">
              <Lamp fill={status.connected ? "bg-clear" : stored ? "bg-caution" : null} />
              <span className="truncate text-[12px] text-ink">
                {status.connected ? "Configured" : stored ? "Incomplete" : "Not set up"}
              </span>
            </div>
          </div>
          <div className="min-w-0 border-r border-edge px-3 py-2.5">
            <Legend>target</Legend>
            <p className="mt-1.5 truncate text-[12px] text-ink" title={status.channelName ?? undefined}>
              {status.channelName ? `#${status.channelName}` : isBot ? "Choose channel" : "Webhook channel"}
            </p>
          </div>
          <div className="min-w-0 px-3 py-2.5">
            <Legend>delivery</Legend>
            <p className="mt-1.5 truncate text-[12px] text-ink">
              {settings.enabled ? "Armed" : "Held"} · {settings.autoSend ? "Automatic" : "Manual"}
            </p>
          </div>
        </div>
      </header>

      <div className="flex flex-col gap-7">
        {error && <ErrorStrip message={error} onDismiss={() => setError(null)} />}
        {notice && (
          <p aria-live="polite" className="border border-clear/40 bg-clear/10 px-3 py-2 text-[12px] leading-relaxed text-ink">{notice}</p>
        )}

        <Section
        title="Setup Guide & Developer Portal"
        description="Configure automated Discord delivery. Use the official Developer Portal to create a bot with server and channel discovery, or use a webhook URL for direct single-channel delivery."
        aside={
          <div className="flex flex-wrap items-center justify-end gap-2">
            <button
              type="button"
              onClick={() => setGuideOpen(!isGuideExpanded)}
              className="focus-ring legend text-[11px] text-ink-dim transition-colors hover:text-ink"
            >
              {isGuideExpanded ? "Hide guide" : "Show guide"}
            </button>
            <button
              type="button"
              onClick={() => void openDiscordPortal()}
              className="focus-ring flex shrink-0 items-center gap-2 border border-edge-strong bg-recess px-3 py-1 text-[11px] legend text-ink hover:bg-face transition-colors"
            >
              <Discord className="size-3.5 text-route" />
              <span>Developer Portal</span>
              <ArrowUpRight className="size-3 text-ink-faint" />
            </button>
          </div>
        }
      >
        <div className="flex flex-col border border-edge bg-face p-4">
          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
            <div className="min-w-0">
              <div className="flex items-center gap-2.5">
                <div className="flex size-7 items-center justify-center rounded-hw border border-route/40 bg-route/10 text-route">
                  <Discord className="size-4" />
                </div>
                <div>
                  <p className="text-[13px] font-medium text-ink">Discord Developer Portal</p>
                  <p className="text-[11px] font-mono text-ink-faint">discord.com/developers/applications</p>
                </div>
              </div>
              <p className="mt-2 max-w-[70ch] text-[12px] leading-relaxed text-ink-dim">
                Create applications, register bot accounts, reset bot authentication tokens, and configure Discord OAuth permissions.
              </p>
            </div>
            <button
              type="button"
              onClick={() => void openDiscordPortal()}
              className="focus-ring flex shrink-0 items-center gap-2 border border-route bg-route px-4 py-2 text-[11px] legend text-on-signal hover:brightness-110 transition-colors"
            >
              <Discord className="size-3.5" />
              <span>Open Discord Developer Portal</span>
              <ArrowUpRight className="size-3 text-on-signal/75" />
            </button>
          </div>

          {isGuideExpanded && (
            <div className="mt-4 border-t border-edge pt-4">
              <div className="mb-3 flex items-center justify-between">
                <Legend className="text-ink">BOT CONFIGURATION WALKTHROUGH</Legend>
                <span className="text-[11px] text-ink-faint">Estimated time: ~2 minutes</span>
              </div>
              <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
                <div className="flex flex-col border border-edge bg-recess/40 p-3 rounded-hw">
                  <div className="flex items-center justify-between">
                    <span className="font-mono text-[11px] font-bold text-route">01</span>
                    <span className="text-[11px] text-ink-faint uppercase font-mono tracking-wider">Portal</span>
                  </div>
                  <p className="mt-1.5 text-[12px] font-medium text-ink">Create Application</p>
                  <p className="mt-1 text-[11px] leading-relaxed text-ink-dim">
                    Click <strong className="text-ink">Open Discord Developer Portal</strong> above, sign in, and click <strong className="text-ink">New Application</strong>. Name it <code className="font-mono text-[11px] bg-face px-1 py-0.5 rounded-hw-xl text-ink">Zoltraak</code>.
                  </p>
                </div>

                <div className="flex flex-col border border-edge bg-recess/40 p-3 rounded-hw">
                  <div className="flex items-center justify-between">
                    <span className="font-mono text-[11px] font-bold text-route">02</span>
                    <span className="text-[11px] text-ink-faint uppercase font-mono tracking-wider">Bot Token</span>
                  </div>
                  <p className="mt-1.5 text-[12px] font-medium text-ink">Generate Token</p>
                  <p className="mt-1 text-[11px] leading-relaxed text-ink-dim">
                    In the left navigation, select <strong className="text-ink">Bot</strong>. Click <strong className="text-ink">Reset Token</strong> (confirm if prompted) and <strong className="text-ink">Copy</strong> the generated token.
                  </p>
                </div>

                <div className="flex flex-col border border-edge bg-recess/40 p-3 rounded-hw">
                  <div className="flex items-center justify-between">
                    <span className="font-mono text-[11px] font-bold text-route">03</span>
                    <span className="text-[11px] text-ink-faint uppercase font-mono tracking-wider">Connect</span>
                  </div>
                  <p className="mt-1.5 text-[12px] font-medium text-ink">Pair in Zoltraak</p>
                  <p className="mt-1 text-[11px] leading-relaxed text-ink-dim">
                    Under <strong className="text-ink">Connection</strong> below, ensure Method is set to <strong className="text-ink">Discord bot</strong>, paste your token into the field, and click <strong className="text-ink">Connect</strong>.
                  </p>
                </div>

                <div className="flex flex-col border border-edge bg-recess/40 p-3 rounded-hw">
                  <div className="flex items-center justify-between">
                    <span className="font-mono text-[11px] font-bold text-route">04</span>
                    <span className="text-[11px] text-ink-faint uppercase font-mono tracking-wider">Invite</span>
                  </div>
                  <p className="mt-1.5 text-[12px] font-medium text-ink">Invite & Target</p>
                  <p className="mt-1 text-[11px] leading-relaxed text-ink-dim">
                    Click <strong className="text-ink">Generate</strong> on the Invite link below to add the bot to your server, then choose your target server and channel from the dropdowns.
                  </p>
                </div>
              </div>

              <div className="mt-3 flex items-start gap-2.5 rounded-hw border border-edge bg-recess/20 px-3 py-2 text-[11.5px] leading-relaxed text-ink-dim">
                <span className="legend text-[11px] text-ink-faint shrink-0 mt-0.5">ALTERNATIVE</span>
                <p>
                  <strong className="text-ink">Webhook URL</strong> requires no Developer Portal or bot application: in Discord, right-click any channel → <strong className="text-ink">Edit Channel</strong> → <strong className="text-ink">Integrations</strong> → <strong className="text-ink">Webhooks</strong> → <strong className="text-ink">New Webhook</strong>, then copy and paste the URL.
                </p>
              </div>
            </div>
          )}
        </div>
      </Section>

      <Section
        title="Connection"
        description="Send finished downloads to a Discord channel. A webhook is the quickest route and needs nothing but a URL. A bot takes one extra step and lets Zoltraak list your servers, read the real upload limit, and post anywhere it has been invited."
        aside={
          <div className="flex items-center gap-2">
            <Lamp fill={status.connected ? "bg-clear" : stored ? "bg-caution" : null} />
            <Legend>{status.connected ? "configured" : stored ? "incomplete" : "not set up"}</Legend>
          </div>
        }
      >
        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="Method" hint="Switching keeps each method's own saved credential.">
            <SelectInput<DiscordMode>
              value={mode}
              onChange={(next) => void commit({ mode: next })}
              options={[
                { value: "webhook", label: "Webhook URL" },
                { value: "bot", label: "Discord bot" },
              ]}
              ariaLabel="Discord delivery method"
              disabled={busy !== null}
            />
          </Field>

          <Field
            label={connectLabel}
            hint={
              isBot ? (
                <span>
                  Create an application in the{" "}
                  <button
                    type="button"
                    onClick={() => void openDiscordPortal()}
                    className="focus-ring text-route underline underline-offset-2 hover:brightness-125 cursor-pointer font-medium"
                  >
                    Discord Developer Portal
                  </button>
                  , add a bot to it, and paste the token here. Zoltraak stores it encrypted and never shows it again.
                </span>
              ) : (
                "Channel settings → Integrations → Webhooks → New Webhook → Copy Webhook URL."
              )
            }
          >
            {showSecretField ? (
              <div className="flex gap-2">
                <TextInput
                  value={secret}
                  onChange={setSecret}
                  type="password"
                  mono
                  placeholder={isBot ? "MTIz…" : "https://discord.com/api/webhooks/…"}
                  ariaLabel={connectLabel}
                />
                <Button
                  disabled={!secret.trim() || busy !== null}
                  onClick={() =>
                    void run("connect", async () => {
                      const value = secret.trim();
                      const result = isBot
                        ? await engine.discordConnectBot(value)
                        : await engine.discordConnectWebhook(value);
                      setSecret("");
                      setReplacing(false);
                      setInviteUrl(result.inviteUrl);
                      if (isBot) await loadGuilds();
                      return isBot
                        ? `Connected as ${result.botUsername ?? "the bot"}. Invite it to a server, then choose a channel below.`
                        : "Webhook checked and saved.";
                    })
                  }
                >
                  {busy === "connect" ? "Checking…" : "Connect"}
                </Button>
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <span data-selectable className="recess flex-1 rounded-hw px-2.5 py-1.5 font-mono text-[12px] text-ink-dim">
                  {status.credentialHint}
                </span>
                <Button tone="ghost" onClick={() => setReplacing(true)}>
                  Replace
                </Button>
                <Button
                  tone="danger"
                  disabled={busy !== null}
                  onClick={() =>
                    void run("disconnect", async () => {
                      await engine.discordDisconnect();
                      setGuilds([]);
                      setChannels([]);
                      setInviteUrl(null);
                      return "Disconnected. The stored credential has been forgotten.";
                    })
                  }
                >
                  Disconnect
                </Button>
              </div>
            )}
          </Field>
        </div>

        {status.lastError && !error && (
          <p className="text-[12px] leading-relaxed text-caution">Last attempt: {status.lastError}</p>
        )}

        <div className="flex flex-wrap items-center gap-3">
          <Button
            tone="ghost"
            disabled={!stored || busy !== null}
            onClick={() => {
              if (reconnecting.current) return;
              reconnecting.current = true;
              void run("reconnect", async () => {
                await engine.discordReconnect();
                if (isBot) {
                  const [nextGuilds, nextChannels] = await Promise.all([
                    engine.discordGuilds(),
                    settings.guildId ? engine.discordChannels(settings.guildId) : Promise.resolve([]),
                  ]);
                  setGuilds(nextGuilds);
                  setChannels(nextChannels);
                }
                return "Connection verified with Discord. Delivery settings are unchanged.";
              }).finally(() => { reconnecting.current = false; });
            }}
          >
            {busy === "reconnect" ? "Reconnecting…" : "Reconnect"}
          </Button>
          <Button
            tone="ghost"
            disabled={!status.connected || busy !== null}
            onClick={() => void run("test", async () => `Test message posted. ${await engine.discordTest()}`)}
          >
            {busy === "test" ? "Sending…" : "Send test message"}
          </Button>
          <Switch
            label="Discord delivery enabled"
            checked={settings.enabled}
            onChange={(next) => void commit({ enabled: next })}
          />
          <Legend>delivery {settings.enabled ? "armed" : "held"}</Legend>
        </div>
      </Section>

      {isBot && (
        <Section
          title="Bot"
          description="Zoltraak manages the bot for you: naming it, generating an invite link with only the permissions it needs, and finding the channels it can post to."
        >
          {(inviteUrl || stored) && (
            <Field
              label="Invite link"
              hint="Open this to add the bot to a server. It asks only to view channels, send messages, embed links and attach files."
            >
              <div className="flex gap-2">
                <TextInput
                  value={inviteUrl ?? ""}
                  onChange={() => {}}
                  mono
                  placeholder="Generate to see the link"
                  ariaLabel="Bot invite link"
                />
                <Button
                  tone="ghost"
                  disabled={busy !== null}
                  onClick={() =>
                    void run("invite", async () => {
                      const url = await engine.discordInviteUrl();
                      setInviteUrl(url);
                      await navigator.clipboard.writeText(url).catch(() => {});
                      return "Invite link generated and copied to the clipboard.";
                    })
                  }
                >
                  {inviteUrl ? "Copy" : "Generate"}
                </Button>
              </div>
            </Field>
          )}

          <div className="grid gap-4 sm:grid-cols-2">
            <Field label="Server">
              <div className="flex gap-2">
                <SelectInput
                  value={settings.guildId ?? ""}
                  onChange={(next) => {
                    void commit({ guildId: next || null, channelId: null, channelName: null });
                    if (next) void loadChannels(next);
                  }}
                  options={[
                    { value: "", label: guilds.length ? "Choose a server…" : "No servers found" },
                    ...guilds.map((g) => ({ value: g.id, label: g.name })),
                  ]}
                  ariaLabel="Discord server"
                  disabled={busy !== null}
                />
                <Button tone="ghost" disabled={busy !== null} onClick={() => void loadGuilds()}>
                  {busy === "servers" ? "…" : "Refresh"}
                </Button>
              </div>
            </Field>

            <Field
              label="Channel"
              hint="Only channels a file can actually be posted to are listed."
            >
              <SelectInput
                value={settings.channelId ?? ""}
                onChange={(next) => {
                  if (!next || !settings.guildId) return;
                  void run("target", async () => {
                    await engine.discordSetTarget(settings.guildId as string, next);
                    return null;
                  });
                }}
                options={[
                  { value: "", label: channels.length ? "Choose a channel…" : "Choose a server first" },
                  ...channels.map((c) => ({ value: c.id, label: `#${c.name ?? c.id}` })),
                ]}
                ariaLabel="Discord channel"
                disabled={busy !== null}
              />
            </Field>


          </div>
        </Section>
      )}

      {isBot && <DiscordBotSettings key={status.credentialHint ?? "unconnected"} settings={settings} enabled={stored} disabled={busy !== null} onSaved={refresh} />}

      <Section title="Upload budget">
            <Field
              label="Upload limit (megabytes)"
              hint={
                "Upload preparation budget in MB. Default: 20 MB. Discord may enforce a lower server limit."
              }
            >
              <div className="flex items-center gap-2">
                <NumberInput ariaLabel="Upload limit (megabytes)" min={1} max={1000} required
                  value={uploadLimitMb} onChange={setUploadLimitDraft} disabled={busy !== null} className="w-24 shrink-0" />
                <span className="text-[12px] text-ink-dim">MB</span>
                <Button disabled={!validUploadLimit || uploadLimitDraft === null || busy !== null}
                  onClick={() => void run("upload-limit", async () => {
                    await engine.updateDiscordSettings({ ...settings, ...(isBot ? { botUploadLimitBytes: Math.round(uploadLimitValue * 1_000_000) } : { uploadLimitBytes: Math.round(uploadLimitValue * 1_000_000) }) });
                    setUploadLimitDraft(null);
                    return `Upload limit saved: ${uploadLimitValue} MB.`;
                  })}>
                  {busy === "upload-limit" ? "Saving…" : "Save limit"}
                </Button>
              </div>
            </Field>
      </Section>
      <DiscordSenderSettings key={mode} settings={settings} commit={commit} disabled={busy !== null} />

      <Section
        title="Delivery"
        description={`Files are prepared for an upload limit of ${isBot ? `${(settings.botUploadLimitBytes ?? 20_000_000) / 1_000_000} MB` : status.uploadLimitLabel}. Larger video, audio and images can be re-encoded to fit — the copy that is sent is smaller, and the download on your disk is left exactly as it was.`}
      >
        <div>
          <ToggleRow title="Include source link" description="Add the source URL to the message when it is available."
            checked={settings.includeSourceUrl} onChange={next => void commit({ includeSourceUrl: next })} />
          <ToggleRow
            title="Send every finished download"
            description="Deliver each completed download and video job automatically, without asking."
            checked={settings.autoSend}
            onChange={(next) => void commit({ autoSend: next })}
            disabled={!settings.enabled}
          />
          <ToggleRow
            title="Re-encode oversized media to fit"
            description="Shrink video, audio and images that are over the limit instead of skipping them. Archives and documents are always skipped — they cannot be made smaller without damage."
            checked={settings.compressToFit}
            onChange={(next) => void commit({ compressToFit: next })}
          />
        </div>

        <div className="grid gap-4 sm:grid-cols-2">
          <Field
            label="Re-encode up to"
            hint="The starting height. A long clip is stepped down automatically when the bitrate its length allows would not support this."
          >
            <SelectInput
              value={String(settings.compressMaxHeight)}
              onChange={(next) => void commit({ compressMaxHeight: Number(next) })}
              options={[
                { value: "1080", label: "1080p" },
                { value: "720", label: "720p" },
                { value: "480", label: "480p" },
                { value: "360", label: "360p" },
              ]}
              ariaLabel="Maximum re-encode height"
            />
          </Field>

          <Field
            label="Message"
            hint="Leave empty to post the files on their own. {filename} and {size} are filled in; anything else is sent as typed."
          >
            <TextInput
              value={settings.messageTemplate}
              onChange={(next) => void commit({ messageTemplate: next })}
              placeholder="No caption"
              ariaLabel="Message template"
            />
          </Field>
        </div>
        <DiscordMessagePreview settings={settings} />
      </Section>

      <Section
        title="Activity"
        aside={
          deliveries.length > 0 && (
            <button
              type="button"
              onClick={() => void run("clear", async () => {
                await engine.discordClearDeliveries();
                return null;
              })}
              className="focus-ring legend text-[11px] text-ink-dim transition-colors hover:text-ink"
            >
              Clear
            </button>
          )
        }
      >
        {deliveries.length === 0 ? (
          <p className="text-[12px] leading-relaxed text-ink-faint">
            Nothing has been sent yet. Finished downloads can also be sent one at a time from the
            right-click menu on the Dispatch Board.
          </p>
        ) : (
          <div>
            {deliveries.map((delivery) => (
              <DeliveryRow
                key={delivery.id}
                delivery={delivery}
                onRetry={() =>
                  void run("retry", async () => {
                    await engine.discordRetryDelivery(delivery.id);
                    return null;
                  })
                }
                onDelete={() => void deleteMessage(delivery)}
                deleting={deleting}
                groupedCount={deliveries.filter(row => row.messageUrl === delivery.messageUrl).length}
                feedback={deleteFeedback?.messageUrl === delivery.messageUrl ? deleteFeedback : undefined}
              />
            ))}
          </div>
        )}
        </Section>
      </div>
    </div>
  );
}
