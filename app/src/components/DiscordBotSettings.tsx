import { useCallback, useEffect, useId, useRef, useState } from "react";
import { engine, openUrl, type DiscordBotDiagnostics, type DiscordBotProfile, type DiscordProfileFields, type DiscordProfileScope, type DiscordSettings } from "../lib/engine";
import { messagePreview, profileDraft, profileImage } from "../lib/discordProfile";
import { Button, ErrorStrip, Section } from "./Controls";
import { Discord } from "./Icons";
import { DiscordProfileImage } from "./DiscordProfileImage";

function ProfileText({ label, value, onChange, max, min, multiline = false, hint }: {
  label: string; value: string; onChange: (value: string) => void; max: number; min?: number; multiline?: boolean; hint?: string;
}) {
  const id = useId();
  const props = { id, name: label, value, maxLength: max, minLength: min, required: !!min, autoComplete: "off",
    "aria-describedby": `${id}-hint`, onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => onChange(e.target.value),
    className: "recess focus-ring w-full rounded-hw px-2.5 py-2 text-[13px] text-ink outline-none disabled:opacity-55" };
  return <div className="flex flex-col gap-1.5">
    <label htmlFor={id} className="legend text-[11px] text-ink-dim">{label}</label>
    {multiline ? <textarea {...props} rows={3} /> : <input {...props} type="text" />}
    <p id={`${id}-hint`} className="text-[12px] leading-relaxed text-ink-dim">{hint} <span>{[...value].length}/{max}</span></p>
  </div>;
}

const TITLES = { global: "Bot profile", application: "Application details", server: "Server profile" };
const DESCRIPTIONS = {
  global: "Name, avatar, and banner apply to this bot across Discord. Review the preview, then save your changes.",
  application: "The application's description, tags, and icon are separate from the bot's name and avatar.",
  server: "Overrides apply only to the selected server. Blank nickname and bio, or removed images, use Discord's defaults."
};

function ProfileEditor({ scope, guildId, guildName, enabled, disabled, globalProfile, onGlobalProfile, onSaved }: {
  scope: DiscordProfileScope; guildId?: string; guildName?: string | null; enabled: boolean;
  disabled: boolean; globalProfile: DiscordBotProfile | null; onGlobalProfile: (profile: DiscordBotProfile) => void; onSaved: () => Promise<void>;
}) {
  const [base, setBase] = useState<DiscordBotProfile | null>(null);
  const [draft, setDraft] = useState<DiscordProfileFields>({});
  const [tagsText, setTagsText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [revision, setRevision] = useState(0);
  const [editingImages, setEditingImages] = useState<Record<string, boolean>>({});
  const alive = useRef(true);
  const lock = useRef(false);
  const dirty = Object.keys(draft).length > 0;
  const cropping = Object.values(editingImages).some(Boolean);
  const available = enabled && (scope !== "server" || !!guildId);
  const load = useCallback(async () => {
    if (!available || lock.current) return;
    lock.current = true; setBusy(true); setError(""); setNotice("");
    try {
      const profile = await engine.discordBotProfile(scope, guildId);
      if (!alive.current) return;
      setBase(profile); setTagsText((profile.tags ?? []).join(", "));
      if (scope === "global") onGlobalProfile(profile);
    } catch (e) { if (alive.current) setError(e instanceof Error ? e.message : String(e)); }
    finally { if (alive.current) { lock.current = false; setBusy(false); } }
  }, [available, scope, guildId, onGlobalProfile]);
  useEffect(() => { alive.current = true; void load(); return () => { alive.current = false; }; }, [load]);
  const patch = (field: keyof DiscordProfileFields, value: string | string[] | null) => {
    if (base) setDraft(d => profileDraft(base, d, field, value));
    setNotice(""); setError("");
  };
  const discard = () => {
    setDraft({}); setTagsText((base?.tags ?? []).join(", ")); setEditingImages({});
    setRevision(n => n + 1); setError(""); setNotice("Draft discarded.");
  };
  const save = async () => {
    if (!base || !dirty || lock.current || cropping || disabled) return;
    const fields = { ...draft };
    if (fields.tags && (fields.tags.length > 5 || fields.tags.some(tag => [...tag].length > 20))) {
      setError("Use at most five tags, each up to 20 characters."); return;
    }
    lock.current = true; setBusy(true); setError(""); setNotice("");
    try {
      const saved = await engine.discordBotProfile(scope, guildId, fields);
      if (!alive.current) return;
      setBase(saved); setDraft({}); setTagsText((saved.tags ?? []).join(", ")); setRevision(n => n + 1);
      setNotice(`${TITLES[scope]} saved to Discord.`);
      if (scope === "global") onGlobalProfile(saved);
      await onSaved();
    } catch (e) { if (alive.current) setError(`${e instanceof Error ? e.message : String(e)} Your draft is preserved.`); }
    finally { if (alive.current) { lock.current = false; setBusy(false); } }
  };
  const current = { ...base, ...draft };
  const effective = { ...current, id: scope === "server" ? globalProfile?.id : current.id };
  const avatar = profileImage(scope, effective, scope === "application" ? "icon" : "avatar", guildId)
    ?? (scope === "server" && globalProfile ? profileImage("global", globalProfile, "avatar") : null);
  const banner = scope === "application" ? null : profileImage(scope, effective, "banner", guildId)
    ?? (scope === "server" && globalProfile ? profileImage("global", globalProfile, "banner") : null);
  const name = scope === "application" ? base?.name : scope === "server" ? current.nick || globalProfile?.username : current.username;
  const text = scope === "application" ? current.description : scope === "server" ? current.bio : null;
  const imageField = (field: "avatar" | "banner" | "icon", label: string) => <DiscordProfileImage key={`${field}-${revision}`}
    label={label} banner={field === "banner"} value={current[field] ?? null} disabled={busy || disabled}
    onChange={value => patch(field, value)} onError={setError} onEditing={editing => setEditingImages(v => ({ ...v, [field]: editing }))} />;
  return <Section title={TITLES[scope]} description={DESCRIPTIONS[scope]}>
    {!available ? <p className="text-[12px] text-ink-dim">{scope === "server" && enabled ? "Choose a server above to edit its profile." : "Connect your bot to edit its profile."}</p> : <>
      {scope === "server" && <p className="text-[12px] text-ink-dim">Editing server: <strong className="text-ink">{guildName || guildId}</strong></p>}
      {error && <div role="alert"><ErrorStrip message={error} onDismiss={() => setError("")} /></div>}
      {busy && !base && <p role="status" className="text-[12px] text-ink-dim">Reading profile from Discord…</p>}
      {base && <form autoComplete="off" onSubmit={e => { e.preventDefault(); void save(); }}>
        <fieldset disabled={busy || disabled} className="grid min-w-0 gap-6 lg:grid-cols-[minmax(0,1fr)_280px]">
          <legend className="sr-only">{TITLES[scope]}</legend>
          <div className="min-w-0 space-y-5">
            {scope === "global" && <ProfileText label="Bot name" value={current.username ?? ""} onChange={value => patch("username", value)} min={2} max={32} hint="Discord limits name changes. Save only when you're ready." />}
            {scope === "server" && <>
              <ProfileText label="Server nickname" value={current.nick ?? ""} onChange={value => patch("nick", value || null)} max={32} hint="Requires Change Nickname permission. Blank uses the bot name." />
              <ProfileText label="Server bio" value={current.bio ?? ""} onChange={value => patch("bio", value || null)} max={190} multiline />
            </>}
            {scope === "application" && <>
              <ProfileText label="Application description" value={current.description ?? ""} onChange={value => patch("description", value)} max={400} multiline />
              <ProfileText label="Application tags" value={tagsText} onChange={value => { setTagsText(value); patch("tags", [...new Set(value.split(",").map(t => t.trim()).filter(Boolean))]); }} max={108} hint="Up to five comma-separated tags, 20 characters each." />
              {imageField("icon", "Application icon")}
            </>}
            {scope !== "application" && <div className="grid gap-5 sm:grid-cols-2">
              {imageField("avatar", scope === "server" ? "Server avatar" : "Bot avatar")}
              {imageField("banner", scope === "server" ? "Server banner" : "Bot banner")}
            </div>}
          </div>
          <div className="min-w-0 self-start" aria-label={`${TITLES[scope]} preview`}>
            <p className="mb-2 text-[12px] text-ink-dim">{dirty || cropping ? "Draft preview" : "Current profile"}</p>
            {scope !== "application" && <div className="aspect-[5/2] overflow-hidden bg-recess">
              {banner ? <img src={banner} alt="" className="h-full w-full object-cover" /> : <div className="flex h-full items-center justify-center text-[12px] text-ink-dim">No banner</div>}
            </div>}
            <div className="border-b border-edge py-4">
              <div className="mb-3 flex items-center gap-3">
                {avatar ? <img src={avatar} alt="" className="size-14 rounded-full object-cover" /> : <div className="flex size-14 shrink-0 items-center justify-center rounded-full bg-recess"><Discord className="size-6 text-ink-dim" /></div>}
                <div className="min-w-0"><p className="break-words text-[15px] font-semibold text-ink">{name || "Bot"}</p><p className="text-[11px] text-ink-dim">{scope === "server" ? "Server override" : scope === "application" ? "Application" : "Bot"}</p></div>
              </div>
              {text && <p className="whitespace-pre-wrap break-words text-[12px] leading-relaxed text-ink-dim">{text}</p>}
              {scope === "application" && !!current.tags?.length && <p className="mt-3 break-words text-[12px] text-ink-dim">{current.tags.join(" · ")}</p>}
            </div>
            {base.id && <div className="mt-3 flex flex-wrap items-center gap-2">
              <span className="text-[11px] text-ink-dim">{scope === "application" ? "Application ID" : "Bot ID"}</span>
              <span className="break-all font-mono text-[11px] text-ink-dim">{base.id}</span>
              <Button tone="ghost" disabled={busy} onClick={() => { void navigator.clipboard.writeText(base.id!).then(() => setNotice("ID copied.")).catch(() => setError("Could not copy the ID. Select and copy it manually.")); }}>Copy ID</Button>
            </div>}
            {scope === "application" && <div className="mt-3 space-y-3">
              <p className="text-[12px] text-ink-dim">Installation: {base.bot_public ? "Public bot" : "Private bot"}</p>
              <Button tone="ghost" onClick={() => { void openUrl(`https://discord.com/developers/applications/${base.id}`).catch(e => setError(String(e))); }}>Developer Portal</Button>
            </div>}
          </div>
        </fieldset>
        <div className="mt-5 flex flex-wrap items-center gap-2 border-t border-edge pt-4">
          <Button type="submit" disabled={busy || disabled || !dirty || cropping}>{busy ? "Saving…" : "Save changes"}</Button>
          <Button tone="ghost" disabled={busy || disabled || (!dirty && !cropping)} onClick={discard}>Discard</Button>
          {scope === "server" && <Button tone="ghost" disabled={busy || disabled || cropping} onClick={() => { setDraft({ nick: null, bio: null, avatar: null, banner: null }); setNotice("Overrides cleared in the preview. Save changes to reset this server's profile."); }}>Reset server overrides</Button>}
          {cropping && <p className="text-[12px] text-ink-dim">Use or cancel each crop before saving.</p>}
        </div>
      </form>}
      <div className="mt-3 flex flex-wrap items-center gap-3">
        <Button tone="ghost" disabled={busy || disabled || dirty || cropping} onClick={() => void load()}>Refresh from Discord</Button>
        {dirty && <p className="text-[12px] text-ink-dim">Unsaved changes. Save or discard before refreshing.</p>}
      </div>
      {notice && <p role="status" className="mt-3 text-[12px] text-ink-dim">{notice}</p>}
    </>}
  </Section>;
}

export function DiscordBotSettings({ settings, enabled, disabled, onSaved }: {
  settings: DiscordSettings; enabled: boolean; disabled: boolean; onSaved: () => Promise<void>;
}) {
  const [globalProfile, setGlobalProfile] = useState<DiscordBotProfile | null>(null);
  const [diagnostics, setDiagnostics] = useState<DiscordBotDiagnostics | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [diagnosticTarget, setDiagnosticTarget] = useState("");
  const target = `${settings.guildId}:${settings.channelId}`;
  const targetRef = useRef(target); targetRef.current = target;
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const check = async () => {
    if (busy) return;
    setBusy(true); setError("");
    const checkedTarget = target;
    try {
      const result = await engine.discordBotDiagnostics();
      if (alive.current && targetRef.current === checkedTarget) { setDiagnostics(result); setDiagnosticTarget(checkedTarget); }
    } catch (e) { if (alive.current) { setDiagnostics(null); setError(e instanceof Error ? e.message : String(e)); } }
    finally { if (alive.current) setBusy(false); }
  };
  return <>
    <ProfileEditor scope="global" enabled={enabled} disabled={disabled} globalProfile={globalProfile} onGlobalProfile={setGlobalProfile} onSaved={onSaved} />
    <ProfileEditor scope="application" enabled={enabled} disabled={disabled} globalProfile={globalProfile} onGlobalProfile={setGlobalProfile} onSaved={onSaved} />
    <ProfileEditor key={settings.guildId ?? "none"} scope="server" guildId={settings.guildId ?? undefined} guildName={settings.guildName} enabled={enabled} disabled={disabled} globalProfile={globalProfile} onGlobalProfile={setGlobalProfile} onSaved={onSaved} />
    <Section title="Connection diagnostics" description="Verify the bot's credential and the selected channel's effective permissions without posting a message.">
      <Button tone="ghost" disabled={!enabled || disabled || busy} onClick={() => void check()}>{busy ? "Checking…" : "Check connection and permissions"}</Button>
      {error && <div role="alert"><ErrorStrip message={error} /></div>}
      {diagnostics && diagnosticTarget === target && <ul className="mt-4 divide-y divide-edge text-[12px]">
        <li className="flex justify-between gap-4 py-2"><span className="text-ink-dim">Bot credential</span><span className="text-ink">Valid</span></li>
        {!diagnostics.destinationSelected && <li className="py-2 text-ink-dim">Choose a server and channel to check destination permissions.</li>}
        {diagnostics.checks.map(item => <li key={item.label} className="flex justify-between gap-4 py-2"><span className="text-ink-dim">{item.label}</span><span className={item.allowed ? "text-ink" : "text-danger"}>{item.allowed ? "Allowed" : "Unavailable"}</span></li>)}
      </ul>}
      {diagnostics && diagnosticTarget !== target && <p className="text-[12px] text-ink-dim">Destination changed. Run the check again.</p>}
    </Section>
  </>;
}

export function DiscordMessagePreview({ settings }: { settings: DiscordSettings }) {
  const text = messagePreview(settings);
  return <div className="mt-4 border-t border-edge pt-4" aria-label="Discord message preview">
    <p className="mb-3 text-[12px] text-ink-dim">Message preview · sample file</p>
    <div className="flex gap-3">
      {settings.mode === "webhook" && settings.webhookAvatarUrl ? <img src={settings.webhookAvatarUrl} alt="" className="size-9 rounded-full object-cover" /> : <Discord className="size-6 shrink-0 text-ink-dim" />}
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-semibold text-ink">{settings.mode === "bot" ? settings.botUsername || "Your bot" : settings.webhookUsername || "Webhook sender"}</p>
        {text && <p className="mt-1 whitespace-pre-wrap break-words text-[12px] leading-relaxed text-ink-dim">{text}</p>}
        <p className="mt-2 text-[12px] text-ink">sample-video.mp4 <span className="text-ink-dim">· 8.0 MiB attachment</span></p>
        {text.length > 2000 && <p role="alert" className="mt-2 text-[12px] text-danger">This sample exceeds Discord's 2,000-character message limit. Shorten the template.</p>}
      </div>
    </div>
  </div>;
}
