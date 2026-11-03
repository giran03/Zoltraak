/**
 * Engraved iconography on a 24px grid.
 *
 * 1.5px stroke, not the hairline the glass build used: on a painted enamel
 * panel a 1px line disappears in the light theme, and panel legends are cut,
 * not printed. One stroke weight and one cap style across the whole set.
 */
type IconProps = {
  className?: string;
};

const base = {
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

export function ArrowUpRight({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M7 17 17 7M9 7h8v8" />
    </svg>
  );
}

export function Coffee({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M4 8h12v8a4 4 0 0 1-4 4H8a4 4 0 0 1-4-4V8ZM16 9h1a3 3 0 1 1 0 6h-1M3 22h16M7 2v3M11 2v3" />
    </svg>
  );
}

export function Keyboard({ className }: IconProps) {
  return <svg {...base} className={className} aria-hidden="true">
    <rect x="2.5" y="5.5" width="19" height="13" rx="2" />
    <path d="M6 9h.5m3 0h.5m3 0h.5m3 0h.5M6 12h.5m3 0h.5m3 0h.5m3 0h.5M7.5 15h9" />
  </svg>;
}

export function DownloadGlyph({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M12 3v12m0 0 4.5-4.5M12 15l-4.5-4.5M4 19h16" />
    </svg>
  );
}

export function Pause({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M9.5 5v14M14.5 5v14" />
    </svg>
  );
}

export function Play({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M8 5.5v13l11-6.5z" />
    </svg>
  );
}

export function Layers({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="m12 3 9 5-9 5-9-5 9-5ZM3 13l9 5 9-5M3 17l9 5 9-5" />
    </svg>
  );
}

export function Gauge({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M4 18a9 9 0 1 1 16 0" />
      <path d="m12 14 4.5-4.5" />
      <circle cx="12" cy="14" r="1.2" />
    </svg>
  );
}

export function FolderOpen({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M3 8V6a1 1 0 0 1 1-1h5l2 2h7a1 1 0 0 1 1 1v2" />
      <path d="M3 9h17.2a.8.8 0 0 1 .78 1l-1.6 7a1 1 0 0 1-.98.8H4.6a1 1 0 0 1-1-.8L3 9Z" />
    </svg>
  );
}

export function Info({ className }: IconProps) {
  return <svg {...base} className={className} aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7h.01" /></svg>;
}

export function Trash({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M4 7h16M9 7V5h6v2M6 7l1 12.1a1 1 0 0 0 1 .9h8a1 1 0 0 0 1-.9L18 7" />
    </svg>
  );
}

export function Shield({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M12 3 5 6v6c0 4.2 2.9 7.6 7 9 4.1-1.4 7-4.8 7-9V6l-7-3Z" />
      <path d="m9.2 12 2 2 3.6-3.8" />
    </svg>
  );
}

export function Plug({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M9 8V3" />
      <path d="M15 8V3" />
      <path d="M6 8h12v5a6 6 0 0 1-12 0V8Z" />
      <path d="M12 19v2" />
    </svg>
  );
}

export function Copy({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <rect x="8.5" y="8.5" width="11" height="11" rx="1.5" />
      <path d="M15.5 8.5V6a1.5 1.5 0 0 0-1.5-1.5H6A1.5 1.5 0 0 0 4.5 6v8A1.5 1.5 0 0 0 6 15.5h2.5" />
    </svg>
  );
}

export function Check({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M5 12.5 9.5 17 19 6.5" />
    </svg>
  );
}

export function Reload({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" />
      <path d="M21 3v5h-5" />
    </svg>
  );
}

export function ExternalLink({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <path d="M15 3h6v6" />
      <path d="m10 14 11-11" />
    </svg>
  );
}

export function RotateCcw({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
      <path d="M3 3v5h5" />
    </svg>
  );
}

export function Search({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="m15.5 15.5 4.5 4.5" />
    </svg>
  );
}

export function Close({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M6 6l12 12M18 6 6 18" />
    </svg>
  );
}

export function Plus({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}

/** Video job marker. Replaces the 🎥 that used to be glued onto the filename. */
export function Film({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <rect x="3" y="4.5" width="18" height="15" rx="1.5" />
      <path d="M3 9.5h18M3 14.5h18" />
      <path d="M7.5 4.5v5M16.5 4.5v5M7.5 14.5v5M16.5 14.5v5" />
    </svg>
  );
}

/**
 * Images: a plate with a horizon and a sun on it.
 *
 * A contact sheet or a stack of frames would have read as "several videos"
 * next to [`Film`], which is the one thing this destination is not.
 */
/** A note: the Music destination. */
export function Note({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M9 17.5V6.2l10-2.2v11.5" />
      <circle cx="6.5" cy="17.5" r="2.5" />
      <circle cx="16.5" cy="15.5" r="2.5" />
    </svg>
  );
}

export function Frame({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="1.5" />
      <circle cx="8.5" cy="9.5" r="1.75" />
      <path d="m3 16.5 4.5-4 5 4M12.5 16.5l3.5-3 5 4" />
    </svg>
  );
}

/**
 * Remux: a file crossing into another container.
 *
 * Two plates with the exchange arrows between them — the same picture a
 * transfer siding makes. Not a wrench or a gear: nothing here is a setting.
 */
export function Transfer({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <rect x="2.5" y="4.5" width="5.5" height="15" rx="1.5" />
      <rect x="16" y="4.5" width="5.5" height="15" rx="1.5" />
      <path d="M9.5 9.5h5m-1.8-2 2 2-2 2" />
      <path d="M14.5 14.5h-5m1.8-2-2 2 2 2" />
    </svg>
  );
}

/** Rules: a checklist of active filtering conditions and category routing rules. */
export function ListChecks({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="m3 6 1.8 1.8L8 4.5" />
      <path d="m3 12 1.8 1.8L8 10.5" />
      <path d="m3 18 1.8 1.8L8 16.5" />
      <path d="M11.5 6h9.5M11.5 12h9.5M11.5 18h9.5" />
    </svg>
  );
}

export const RulesGlyph = ListChecks;
export const Junction = ListChecks;

export function Sliders({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M4 7h10M18 7h2M4 12h4M12 12h8M4 17h8M16 17h4" />
      <circle cx="16" cy="7" r="2" />
      <circle cx="10" cy="12" r="2" />
      <circle cx="14" cy="17" r="2" />
    </svg>
  );
}


export function ChevronDown({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="m6 9.5 6 6 6-6" />
    </svg>
  );
}

export function Menu({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M4 7h16M4 12h16M4 17h16" />
    </svg>
  );
}

export function Sun({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.2 5.2l1.4 1.4M17.4 17.4l1.4 1.4M18.8 5.2l-1.4 1.4M6.6 17.4l-1.4 1.4" />
    </svg>
  );
}

export function Moon({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5Z" />
    </svg>
  );
}

export function Monitor({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <rect x="3" y="4" width="18" height="12" rx="1.5" />
      <path d="M9 20h6M12 16v4" />
    </svg>
  );
}

export function Discord({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M8.5 15.5c2.3 1.1 4.7 1.1 7 0" />
      <path d="M14.5 6.5Q12 5.5 9.5 6.5L8 5C6.2 5.6 4.8 6.7 4 8c-1 2.2-1.3 4.5-1 6.8.1.6.4 1.1.9 1.4A12 12 0 0 0 8 18l1-1.6" />
      <path d="m14.5 6.5 1.5-1.5c1.8.6 3.2 1.7 4 3 1 2.2 1.3 4.5 1 6.8-.1.6-.4 1.1-.9 1.4A12 12 0 0 1 16 18l-1-1.6" />
      <path d="M9.5 11.5v.5M14.5 11.5v.5" />
    </svg>
  );
}

export function BookOpen({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z" />
      <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z" />
    </svg>
  );
}

export function Volume2({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
      <path d="M15.54 8.46a5 5 0 0 1 0 7.07" />
      <path d="M19.07 4.93a10 10 0 0 1 0 14.14" />
    </svg>
  );
}

export function VolumeX({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <polygon points="11 5 6 9 2 9 2 15 6 15 11 5" />
      <line x1="22" y1="9" x2="16" y2="15" />
      <line x1="16" y1="9" x2="22" y2="15" />
    </svg>
  );
}

export function Upload({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="17 8 12 3 7 8" />
      <line x1="12" y1="3" x2="12" y2="15" />
    </svg>
  );
}

export function Palette({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <circle cx="13.5" cy="6.5" r="1.5" fill="currentColor" stroke="none" />
      <circle cx="17.5" cy="10.5" r="1.5" fill="currentColor" stroke="none" />
      <circle cx="8.5" cy="8.5" r="1.5" fill="currentColor" stroke="none" />
      <circle cx="6.5" cy="13.5" r="1.5" fill="currentColor" stroke="none" />
      <path d="M12 2C6.48 2 2 6.48 2 12c0 4.42 3.58 8 8 8 .55 0 1-.45 1-1 0-.28-.11-.53-.29-.71-.18-.18-.29-.43-.29-.71 0-.55.45-1 1-1h1.58c4.64 0 8.42-3.78 8.42-8.42C21.42 4.97 17.18 2 12 2Z" />
    </svg>
  );
}

export function Sparkles({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="m12 3 2.5 5.5L20 11l-5.5 2.5L12 19l-2.5-5.5L4 11l5.5-2.5z" />
      <path d="M19 3v4M21 5h-4" />
    </svg>
  );
}

export function Type({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <polyline points="4 7 4 4 20 4 20 7" />
      <line x1="9" y1="20" x2="15" y2="20" />
      <line x1="12" y1="4" x2="12" y2="20" />
    </svg>
  );
}

export function LayoutGrid({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <rect x="3" y="3" width="7" height="7" rx="1" />
      <rect x="14" y="3" width="7" height="7" rx="1" />
      <rect x="14" y="14" width="7" height="7" rx="1" />
      <rect x="3" y="14" width="7" height="7" rx="1" />
    </svg>
  );
}

export function FileDown({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <polyline points="14 2 14 8 20 8" />
      <line x1="12" y1="18" x2="12" y2="12" />
      <polyline points="9 15 12 18 15 15" />
    </svg>
  );
}

export function FileUp({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <polyline points="14 2 14 8 20 8" />
      <line x1="12" y1="12" x2="12" y2="18" />
      <polyline points="9 15 12 12 15 15" />
    </svg>
  );
}

export function User({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2" />
      <circle cx="12" cy="7" r="4" />
    </svg>
  );
}

export function Key({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <circle cx="7.5" cy="15.5" r="4.5" />
      <path d="m21 3-9.5 9.5M15.5 7.5l3 3M18.5 4.5l2 2" />
    </svg>
  );
}

export function Pencil({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />
    </svg>
  );
}

export function Bell({ className }: IconProps) {
  return (
    <svg {...base} className={className} aria-hidden="true">
      <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
      <path d="M13.73 21a2 2 0 0 1-3.46 0" />
    </svg>
  );
}


