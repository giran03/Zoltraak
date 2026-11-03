import { TAB_OUTLINES, type SettingsTab } from "./settingsOutlines.ts";
export type SettingMatch = { tab: SettingsTab; id: string; label: string; keywords?: string; control?: string };
export const SETTINGS_INDEX: SettingMatch[] = [
  { tab: "Appearance", id: "section-typography-atmospheric-overlays", label: "Noise grain size", keywords: "texture analogue thermal coarse fine density underlay overlay", control: "Analogue Console Noise Grain size" },
  { tab: "About", id: "section-developer", label: "CafeTako Team & support", keywords: "developer developers creators email support-zoltraak@cafetako.com coffee donate donation buymeacoffee copyright all rights reserved" },
  { tab: "Appearance", id: "section-app-icon", label: "Sync app icon with theme", keywords: "desktop shortcut tray transparent emblem foreground lettering palette colors", control: "Sync app icon with theme" },
  ...Object.entries(TAB_OUTLINES).flatMap(([tab, sections]) => sections.map(section => ({ ...section, tab: tab as SettingsTab }))),
  { tab: "General", id: "section-behaviour", label: "Delete local file with trash button", keywords: "remove deletion disk drive", control: "Delete local file with trash button" },
  { tab: "General", id: "section-behaviour", label: "Completion sound", keywords: "audio notification chime volume", control: "Completion sound" },
  { tab: "General", id: "section-behaviour", label: "Desktop notifications", keywords: "alerts toast", control: "Desktop notifications" },
  { tab: "General", id: "section-behaviour", label: "Start on launch", keywords: "startup boot", control: "Start on launch" },
  { tab: "General", id: "section-behaviour", label: "Close to tray", keywords: "window minimize", control: "Close to tray" },
  { tab: "General", id: "section-behaviour", label: "Minimize to tray", keywords: "window taskbar", control: "Minimize to tray" },
  { tab: "General", id: "section-engine-defaults", label: "Default download folder", keywords: "destination directory drive location" },
  { tab: "General", id: "section-engine-defaults", label: "Bandwidth and connections", keywords: "speed limit network concurrent" },
  { tab: "Extension", id: "section-local-api", label: "Extension pairing", keywords: "browser token connect chrome firefox" },
  { tab: "Extension", id: "section-cookie-source", label: "Shared sign-ins", keywords: "cookies session login account authentication" },
  { tab: "General", id: "section-media-processing-codecs-ffmpeg", label: "Install media tools", keywords: "ffmpeg ffprobe codec audio video" },
  { tab: "Appearance", id: "section-register-columns-row-presentation", label: "Download columns", keywords: "table visibility rows" },
  { tab: "Appearance", id: "section-interface-geometry-sizing", label: "Scale, density, and corner radius", keywords: "zoom size compact comfortable rounded" },
  { tab: "Appearance", id: "section-panel-lighting-master-mode", label: "Light, dark, and system theme", keywords: "color mode lighting" },
  { tab: "Appearance", id: "section-custom-theme-studio", label: "Import or export a custom theme", keywords: "json palette colors" },
  { tab: "Appearance", id: "section-typography-atmospheric-overlays", label: "Fonts and visual effects", keywords: "typography scanlines grid glass noise vignette bloom warmth" },
  { tab: "Appearance", id: "section-motion-interface-animations", label: "Confetti celebration", keywords: "completion sound effects animations", control: "Confetti Celebration Sound Effect" },
  { tab: "Appearance", id: "section-motion-interface-animations", label: "File opening feedback", keywords: "download double click middle mouse shortcut flash tint animation duration", control: "File opening feedback duration" },
  { tab: "Appearance", id: "section-motion-interface-animations", label: "Download double-click interval", keywords: "mouse speed timing faster slower sensitivity file opening description", control: "Download double-click interval" },
  { tab: "Appearance", id: "section-motion-interface-animations", label: "Notification history limit", keywords: "inbox alerts maximum", control: "Notification History Limit" },
  { tab: "Appearance", id: "section-register-columns-row-presentation", label: "Relative timestamps and speed units", keywords: "date mbps megabits telemetry" },
  { tab: "General", id: "section-setup-health", label: "Setup health", keywords: "diagnostics engine extension sign-ins tools folder access" },
];

export function searchSettings(query: string): SettingMatch[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return [];
  return SETTINGS_INDEX.filter(entry => terms.every(term => `${entry.label} ${entry.tab} ${entry.keywords || ""}`.toLowerCase().includes(term)))
    .sort((a, b) => Number(b.label.toLowerCase().includes(query.trim().toLowerCase())) - Number(a.label.toLowerCase().includes(query.trim().toLowerCase())));
}
