export const SETTINGS_TABS = ["General", "Appearance", "Timetable", "Extension", "About"] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];

export interface OutlineItem {
  id: string;
  label: string;
}

export const TAB_OUTLINES: Record<SettingsTab, OutlineItem[]> = {
  General: [
    { id: "section-setup-health", label: "Setup Health" },
    { id: "section-engine-defaults", label: "Engine Defaults" },
    { id: "section-behaviour", label: "System Behaviour" },
    { id: "section-display-motion-hardware-acceleration", label: "Display & Motion" },
    { id: "section-media-processing-codecs-ffmpeg", label: "FFmpeg Media Tools" },
    { id: "section-application-updates", label: "Application Updates" },
    { id: "section-handbook-documentation", label: "Handbook & Docs" },
    { id: "section-factory-reset", label: "Factory Reset" },
  ],
  Appearance: [
    { id: "section-app-icon", label: "App Icon" },
    { id: "section-panel-lighting-master-mode", label: "Lighting Mode" },
    { id: "section-curated-theme-presets", label: "Curated Presets" },
    { id: "section-custom-theme-studio", label: "Custom Theme Studio" },
    { id: "section-interface-geometry-sizing", label: "Geometry & Sizing" },
    { id: "section-typography-atmospheric-overlays", label: "Atmospheric Overlays" },
    { id: "section-motion-interface-animations", label: "Interface Motion" },
    { id: "section-hardware-acceleration-webgpu-subsystem", label: "Hardware & WebGPU" },
    { id: "section-register-columns-row-presentation", label: "Register Columns" },
    { id: "section-reset-appearance", label: "Reset Appearance" },
  ],
  Timetable: [
    { id: "section-time-window", label: "Active Time Window" },
    { id: "section-when-the-queue-drains", label: "On Queue Drains" },
    { id: "section-timetable", label: "Timetable Status" },
  ],
  Extension: [
    { id: "section-local-api", label: "Local API Loopback" },
    { id: "section-cookie-source", label: "Cookie Vault Source" },
    { id: "section-browser-integration", label: "Browser Integration" },
  ],
  About: [
    { id: "section-software-identity", label: "Software Identity" },
    { id: "section-developer", label: "Developer" },
    { id: "section-legal-privacy-documentation", label: "Legal & Manuals" },
    { id: "section-licensing-attributions", label: "Licensing & Attributions" },
  ],
};
