import type { DiscordBotProfile, DiscordProfileFields, DiscordProfileScope, DiscordSettings } from "./engine";

export function profileImage(scope: DiscordProfileScope, profile: DiscordBotProfile, field: "avatar" | "banner" | "icon", guildId?: string): string | null {
  const hash = profile[field];
  if (!hash) return null;
  if (hash.startsWith("data:image/")) return hash;
  const extension = hash.startsWith("a_") ? "gif" : "png";
  const path = scope === "application" ? `app-icons/${profile.id}`
    : scope === "server" ? `guilds/${guildId}/users/${profile.id}/${field === "avatar" ? "avatars" : "banners"}`
    : `${field === "avatar" ? "avatars" : "banners"}/${profile.id}`;
  return `https://cdn.discordapp.com/${path}/${hash}.${extension}?size=1024`;
}

/** Omitted fields preserve Discord's values; explicit null removes an override. */
export function profileDraft(base: DiscordBotProfile, draft: DiscordProfileFields, field: keyof DiscordProfileFields, value: string | string[] | null): DiscordProfileFields {
  const next = { ...draft, [field]: value };
  const saved = base[field] ?? (field === "tags" ? [] : ["username", "description"].includes(field) ? "" : null);
  if (JSON.stringify(saved) === JSON.stringify(value)) delete next[field];
  return next;
}

export function messagePreview(settings: DiscordSettings): string {
  const source = "https://example.com/downloads/sample-video.mp4";
  let caption = settings.messageTemplate?.trim()
    ? settings.messageTemplate.replaceAll("{filename}", "sample-video.mp4").replaceAll("{size}", "8.0 MiB")
      .replaceAll("{url}", source).replaceAll("{host}", "").replaceAll("{category}", "") : "";
  if (settings.includeSourceUrl && caption !== source) caption = [caption, source].filter(Boolean).join("\n");
  if (settings.pingSender && settings.discordUserId) {
    const mention = `<@${settings.discordUserId}>`;
    const template = settings.pingMessageTemplate ?? "{discord user id}";
    let ping = template.replaceAll("{discord user id}", mention).replaceAll("{discord_user_id}", mention);
    if (!ping.includes(mention)) ping = ping.trim() ? `${ping} ${mention}` : mention;
    caption = [ping, caption].filter(Boolean).join("\n");
  }
  return caption;
}

/** Crop coordinates in source pixels. Positions are 0–100; zoom starts at 1. */
export function imageCrop(width: number, height: number, ratio: number, zoom: number, x: number, y: number) {
  const cropWidth = Math.min(width, height * ratio) / zoom;
  const cropHeight = cropWidth / ratio;
  return { x: (width - cropWidth) * x / 100, y: (height - cropHeight) * y / 100, width: cropWidth, height: cropHeight };
}
