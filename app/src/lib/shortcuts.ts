export type Modifier = "Mod" | "Ctrl" | "Alt" | "Shift" | "Meta";
export type Binding = { key: string; modifiers: Modifier[] };
const binding = (key: string, ...modifiers: Modifier[]): Binding => ({ key, modifiers });
export const SHORTCUTS = [
  { id: "newDownload", label: "Add downloads", description: "Focus the link entry.", scope: "Downloads", defaults: [binding("n"), binding("n", "Mod")] },
  { id: "search", label: "Search downloads", description: "Focus the filename and URL search.", scope: "Downloads", defaults: [binding("/")] },
  { id: "selectAll", label: "Select all downloads", description: "Select every row in the current filter.", scope: "Downloads", defaults: [binding("a", "Mod")] },
  { id: "moveDown", label: "Next download", description: "Move the selection down one row.", scope: "Downloads", defaults: [binding("ArrowDown")] },
  { id: "moveUp", label: "Previous download", description: "Move the selection up one row.", scope: "Downloads", defaults: [binding("ArrowUp")] },
  { id: "extendDown", label: "Extend selection down", description: "Include the next row in the selection.", scope: "Downloads", defaults: [binding("ArrowDown", "Shift")] },
  { id: "extendUp", label: "Extend selection up", description: "Include the previous row in the selection.", scope: "Downloads", defaults: [binding("ArrowUp", "Shift")] },
  { id: "togglePause", label: "Pause or resume selected", description: "Toggle the selected transfers.", scope: "Downloads", defaults: [binding(" ")] },
  { id: "remove", label: "Remove selected downloads", description: "Use the current file-removal preference.", scope: "Downloads", defaults: [binding("Delete"), binding("Backspace")] },
  { id: "openDownload", label: "Open download file", description: "Open a clicked or selected download; reveal unfinished entries. Double-click also opens entries.", scope: "Downloads", defaults: [binding("MouseMiddle")] },
  { id: "openFolder", label: "Open download folder", description: "Reveal the working folder on disk.", scope: "Application", defaults: [binding("e", "Mod")] },
  { id: "openShortcuts", label: "Keyboard shortcuts", description: "Open this shortcut editor.", scope: "Application", defaults: [binding("/", "Mod")] },
] as const;
export type ShortcutId = typeof SHORTCUTS[number]["id"];
export type ShortcutSettings = { enabled: boolean; actions: Record<ShortcutId, { enabled: boolean; bindings: Binding[] }> };
export const SHORTCUTS_KEY = "zoltraak.keyboard-shortcuts.v1";
export const isMac = () => typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
export function defaultShortcuts(): ShortcutSettings {
  return { enabled: true, actions: Object.fromEntries(SHORTCUTS.map(action => [action.id, { enabled: true, bindings: action.defaults.map(b => ({ key: b.key, modifiers: [...b.modifiers] })) }])) as ShortcutSettings["actions"] };
}
export function normalizeKey(key: string) { return key.length === 1 ? key.toLowerCase() : key; }
function physicalModifiers(value: Binding, mac = isMac()) {
  return [...new Set(value.modifiers.map(mod => mod === "Mod" ? (mac ? "Meta" : "Ctrl") : mod))].sort();
}
export function sameBinding(a: Binding, b: Binding, mac = isMac()) {
  return normalizeKey(a.key) === normalizeKey(b.key) && physicalModifiers(a, mac).join() === physicalModifiers(b, mac).join();
}
type BindingEvent = Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey"> | Pick<MouseEvent, "button" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">;
export function bindingFromEvent(event: BindingEvent): Binding | null {
  const key = "key" in event ? event.key : ({ 1: "MouseMiddle", 3: "MouseBack", 4: "MouseForward" } as Record<number, string>)[event.button];
  if (!key || ["Control", "Meta", "Alt", "Shift", "AltGraph", "Dead", "Unidentified", "Process"].includes(key)) return null;
  const modifiers: Modifier[] = [];
  if (event.ctrlKey) modifiers.push("Ctrl");
  if (event.altKey) modifiers.push("Alt");
  if (event.shiftKey) modifiers.push("Shift");
  if (event.metaKey) modifiers.push("Meta");
  return { key: normalizeKey(key), modifiers };
}
export function shortcutConflict(settings: ShortcutSettings, id: ShortcutId, value: Binding) {
  return SHORTCUTS.find(action => action.id !== id && settings.actions[action.id].bindings.some(b => sameBinding(b, value)));
}
export function matchesShortcut(settings: ShortcutSettings, id: ShortcutId, event: KeyboardEvent | MouseEvent) {
  if (!settings.enabled || !settings.actions[id].enabled || event.defaultPrevented || ("isComposing" in event && event.isComposing)
    || ("repeat" in event && event.repeat && !["moveDown", "moveUp", "extendDown", "extendUp"].includes(id))) return false;
  const value = bindingFromEvent(event);
  return Boolean(value && settings.actions[id].bindings.some(b => sameBinding(b, value)));
}
export function formatBinding(value: Binding) {
  const names: Record<string, string> = { MouseMiddle: "Middle click", MouseBack: "Mouse back", MouseForward: "Mouse forward", " ": "Space", ArrowDown: "↓", ArrowUp: "↑", ArrowLeft: "←", ArrowRight: "→", Meta: "Cmd", Mod: isMac() ? "Cmd" : "Ctrl" };
  return [...value.modifiers.map(mod => names[mod] || mod), names[value.key] || (value.key.length === 1 ? value.key.toUpperCase() : value.key)].join(" + ");
}
export function shortcutLabel(settings: ShortcutSettings, id: ShortcutId) {
  return settings.enabled && settings.actions[id].enabled ? settings.actions[id].bindings.map(formatBinding).join(" / ") : "Off";
}
export function readShortcuts(raw: string | null): ShortcutSettings {
  const defaults = defaultShortcuts();
  try {
    const data = JSON.parse(raw || "null");
    if (!data || typeof data !== "object") return defaults;
    defaults.enabled = data.enabled !== false;
    for (const action of SHORTCUTS) {
      const saved = data.actions?.[action.id];
      if (!saved || !Array.isArray(saved.bindings) || !saved.bindings.length || saved.bindings.length > 2) continue;
      const valid = saved.bindings.every((b: Binding) => b && typeof b.key === "string" && b.key.length > 0 && b.key.length < 32
        && !["Escape", "Tab", "Enter"].includes(b.key) && Array.isArray(b.modifiers)
        && b.modifiers.length <= 4 && b.modifiers.every(m => ["Mod", "Ctrl", "Alt", "Shift", "Meta"].includes(m)));
      if (!valid) continue;
      const bindings = saved.bindings.map((b: Binding) => ({ key: normalizeKey(b.key), modifiers: [...new Set(b.modifiers)] }));
      defaults.actions[action.id] = { enabled: saved.enabled !== false, bindings };
    }
    if (SHORTCUTS.some(action => defaults.actions[action.id].bindings.some(b => shortcutConflict(defaults, action.id, b)))) return defaultShortcuts();
  } catch { /* Malformed preferences fall back to working defaults. */ }
  return defaults;
}
export function isTypingTarget(target: EventTarget | null) {
  return target instanceof HTMLElement && (target.isContentEditable || Boolean(target.closest("input,textarea,select,[contenteditable='true']")));
}
export function shortcutsBlocked(event: KeyboardEvent) {
  return event.defaultPrevented || event.isComposing || Boolean(document.querySelector("dialog[open], [aria-modal='true']"));
}
