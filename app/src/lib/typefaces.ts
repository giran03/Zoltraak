import type { FontMode } from "./useAppearance.ts";

/**
 * The Typeface Character presets, in the order they are offered.
 *
 * `sans` and `mono` are the exact font stacks styles.css applies for the mode,
 * so what the picker previews is what selecting it does; a test holds the two in
 * step. `note` says which families are really behind the name.
 */
export type TypefacePreset = {
  id: Exclude<FontMode, "custom">;
  label: string;
  note: string;
  sans: string;
  mono: string;
};

const ARCHIVO = '"Archivo Variable", ui-sans-serif, system-ui, sans-serif';
const MARTIAN = '"Martian Mono Variable", ui-monospace, monospace';

export const TYPEFACE_PRESETS: readonly TypefacePreset[] = [
  { id: "default", label: "Industrial", note: "Archivo text with Martian Mono figures", sans: ARCHIVO, mono: MARTIAN },
  { id: "mono", label: "Full Mono", note: "Martian Mono for everything", sans: MARTIAN, mono: MARTIAN },
  {
    id: "system",
    label: "System UI",
    note: "Your operating system's own faces",
    sans: 'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", sans-serif',
    mono: 'ui-monospace, "Cascadia Mono", "SF Mono", Consolas, Menlo, monospace',
  },
  { id: "sans", label: "Modern Sans", note: "Inter text with Martian Mono figures", sans: '"Inter Variable", ui-sans-serif, system-ui, sans-serif', mono: MARTIAN },
];
