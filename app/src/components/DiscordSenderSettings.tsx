import { useState } from "react";
import type { DiscordSettings } from "../lib/engine";
import { Button, Field, Section, SelectInput, TextInput, ToggleRow } from "./Controls";
import { WebhookAvatarHelp } from "./WebhookAvatarHelp";

export function DiscordSenderSettings({ settings, commit, disabled = false }: { settings: DiscordSettings; disabled?: boolean; commit: (patch: Partial<DiscordSettings>) => Promise<boolean> }) {
  const [draft, setDraft] = useState<Partial<DiscordSettings>>({});
  const [saving, setSaving] = useState(false);
  const [avatarModeDraft, setAvatarModeDraft] = useState<"default" | "url" | null>(null);
  const name = draft.webhookUsername ?? settings.webhookUsername ?? "";
  const avatar = draft.webhookAvatarUrl !== undefined ? draft.webhookAvatarUrl ?? "" : settings.webhookAvatarUrl ?? "";
  const avatarMode = avatarModeDraft ?? (settings.webhookAvatarUrl?.trim() ? "url" : "default");
  const userId = draft.discordUserId ?? settings.discordUserId ?? "";
  const validId = !userId.trim() || (/^[0-9]+$/.test(userId.trim()) && BigInt(userId.trim()) > 0n && BigInt(userId.trim()) <= 18446744073709551615n);
  let validAvatar = false;
  try { const url = new URL(avatar.trim()); validAvatar = ["http:", "https:"].includes(url.protocol); } catch { /* Empty is a default; malformed URLs cannot be saved. */ }
  const pingTemplate = draft.pingMessageTemplate ?? settings.pingMessageTemplate ?? "{discord user id}";
  const valid = [...pingTemplate].length <= 500 && validId && (settings.mode === "bot" || ((avatarMode === "default" || validAvatar) && [...name.trim()].length <= 80));
  return <Section title="Sender and mentions" description="Choose how messages from Zoltraak appear and who they notify.">
    <div className="discord-sender-fields grid gap-4 sm:grid-cols-2">
      {settings.mode === "webhook" && <>
        <Field label="Sender name" hint="Blank uses the webhook's default name. Server webhook details stay unchanged.">
          <TextInput ariaLabel="Sender name" value={name} onChange={value => setDraft(d => ({ ...d, webhookUsername: value }))} disabled={disabled || saving} />
        </Field>
        <Field label="Webhook avatar" info={<WebhookAvatarHelp />} hint={avatarMode === "default" ? "Uses the image configured for this webhook in Discord." : "Use an image link that Discord can access. Open the info button for a step-by-step guide."}>
          <SelectInput ariaLabel="Webhook avatar" value={avatarMode} disabled={disabled || saving}
            options={[{ value: "default", label: "Use webhook default" }, { value: "url", label: "Use public image URL" }]}
            onChange={value => {
              setAvatarModeDraft(value as "default" | "url");
              setDraft(d => ({ ...d, webhookAvatarUrl: value === "default" ? null : avatar || settings.webhookAvatarUrl || "", webhookAvatarAssetId: null }));
            }} />
          {avatarMode === "url" && <div className="mt-2"><TextInput ariaLabel="Avatar image URL" type="url" value={avatar} placeholder="https://cdn.discordapp.com/attachments/…"
            onChange={value => setDraft(d => ({ ...d, webhookAvatarUrl: value, webhookAvatarAssetId: null }))} disabled={disabled || saving} /></div>}
        </Field>
      </>}
      <Field label="Discord user ID" hint="Discord Settings → Advanced → Developer Mode. Right-click your profile and choose Copy User ID.">
        <TextInput ariaLabel="Discord user ID" inputMode="numeric" mono value={userId} onChange={value => setDraft(d => ({ ...d, discordUserId: value }))} disabled={disabled || saving} />
      </Field>
      <Field label="Ping message template" hint="Use {discord user id} (or {discord_user_id}) for your mention. Text without the placeholder gets your mention appended. Used only when ping is enabled.">
        <TextInput ariaLabel="Ping message template" value={pingTemplate} placeholder="sent by {discord user id}" onChange={value => setDraft(d => ({ ...d, pingMessageTemplate: value }))} disabled={disabled || saving} />
      </Field>
      <div className="flex items-end justify-end sm:col-span-2">
        <Button disabled={disabled || saving || !valid || !Object.keys(draft).length} onClick={() => {
          setSaving(true);
          void commit({ ...draft, ...(userId.trim() ? {} : { pingSender: false }) }).then(saved => { if (saved) { setDraft({}); setAvatarModeDraft(null); } }).finally(() => { setSaving(false); });
        }}>{saving ? "Saving…" : "Save sender"}</Button>
      </div>
    </div>
    {!valid && <p role="alert" className="text-[12px] text-danger">Use a valid numeric user ID, an HTTP/HTTPS image URL, a sender name of at most 80 characters, and a ping template of at most 500 characters.</p>}
    <ToggleRow title="Ping me when sending" description="Mention your saved user ID on each Discord message. Discord controls notification delivery."
      checked={settings.pingSender ?? false} disabled={disabled || saving || !settings.discordUserId || Object.keys(draft).length > 0 && draft.discordUserId !== undefined && draft.discordUserId !== settings.discordUserId}
      onChange={value => void commit({ pingSender: value })} />
  </Section>;
}
