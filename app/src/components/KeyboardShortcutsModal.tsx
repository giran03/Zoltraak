import { useEffect, useId, useRef, useState } from "react";
import { Close, Keyboard, RotateCcw, Search } from "./Icons";
import { Switch } from "./Controls";
import { SHORTCUTS, bindingFromEvent, defaultShortcuts, formatBinding, shortcutConflict, type Binding, type ShortcutId, type ShortcutSettings } from "../lib/shortcuts";
import { saveShortcuts, useShortcuts } from "../lib/useShortcuts";

export function KeyboardShortcutsModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const settings = useShortcuts();
  const titleId = useId();
  const helpId = useId();
  const [search, setSearch] = useState("");
  const [recording, setRecording] = useState<ShortcutId | null>(null);
  const [modifiers, setModifiers] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open) {
      dialog.showModal();
      dialog.querySelector<HTMLInputElement>("input[type='search']")?.focus();
      const frame = requestAnimationFrame(() => dialog.classList.add("is-open"));
      return () => cancelAnimationFrame(frame);
    }
    dialog.classList.remove("is-open");
    dialog.close();
    setRecording(null);
    setModifiers("");
  }, [open]);

  const commit = (next: ShortcutSettings, notice: string) => {
    if (!saveShortcuts(next)) { setError("Could not save shortcuts. Check that app storage is available and try again."); return false; }
    setError(""); setMessage(notice); return true;
  };
  const rows = SHORTCUTS.filter(action => `${action.label} ${action.description} ${action.scope}`.toLowerCase().includes(search.toLowerCase()));
  const assignBinding = (id: ShortcutId, value: Binding) => {
    const conflict = shortcutConflict(settings, id, value);
    if (conflict) { setError(`${formatBinding(value)} is already assigned to “${conflict.label}”. Choose another combination.`); return; }
    if (commit({ ...settings, actions: { ...settings.actions, [id]: { ...settings.actions[id], bindings: [value] } } }, `${SHORTCUTS.find(action => action.id === id)?.label} changed to ${formatBinding(value)}.`)) {
      setRecording(null); setModifiers("");
    }
  };
  const record = (event: React.KeyboardEvent, id: ShortcutId) => {
    if (recording !== id) return;
    event.preventDefault(); event.stopPropagation();
    if (event.key === "Escape") { setRecording(null); setModifiers(""); setError(""); return; }
    if (event.repeat || event.nativeEvent.isComposing) return;
    const value = bindingFromEvent(event.nativeEvent);
    if (!value) { setModifiers([event.ctrlKey && "Ctrl", event.altKey && "Alt", event.shiftKey && "Shift", event.metaKey && "Cmd"].filter(Boolean).join(" + ")); return; }
    if (["Tab", "Enter"].includes(value.key)) { setError("Tab and Enter are reserved for navigation and editing. Choose another key, or Escape to cancel."); return; }
    assignBinding(id, value);
  };

  return (
    <dialog ref={dialogRef} aria-labelledby={titleId} aria-describedby={helpId} className="shortcut-dialog t-modal"
      onCancel={event => { if (recording) { event.preventDefault(); setRecording(null); } }}
      onClose={onClose}
      onClick={event => {
        if (event.target !== event.currentTarget) return;
        const rect = event.currentTarget.getBoundingClientRect();
        if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) event.currentTarget.close();
      }}>
      <div className="flex min-h-0 flex-col">
        <header className="flex items-start gap-3 border-b border-edge px-5 py-4 sm:px-6">
          <Keyboard className="mt-1 size-5 shrink-0 text-route" />
          <div className="min-w-0 flex-1">
            <h2 id={titleId} className="text-[18px] font-semibold text-ink">Keyboard shortcuts</h2>
            <p id={helpId} className="mt-1 text-[12px] leading-relaxed text-ink-dim">Make the controls yours. Changes save immediately and update the app’s hints.</p>
          </div>
          <button type="button" aria-label="Close keyboard shortcuts" onClick={() => dialogRef.current?.close()} className="focus-ring rounded-hw p-1.5 text-ink-dim hover:bg-recess hover:text-ink"><Close className="size-4" /></button>
        </header>
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-edge bg-recess/40 px-5 py-3 sm:px-6">
          <label className="flex items-center gap-3 text-[12px] text-ink">
            <Switch label="Enable keyboard shortcuts" checked={settings.enabled} onChange={enabled => { setRecording(null); commit({ ...settings, enabled }, enabled ? "Keyboard shortcuts enabled." : "Keyboard shortcuts disabled."); }} />
            Enable shortcuts
          </label>
          <button type="button" onClick={() => { if (commit(defaultShortcuts(), "Default shortcuts restored.")) setRecording(null); }} className="focus-ring flex items-center gap-1.5 rounded-hw px-2 py-1 text-[11px] text-ink-dim hover:bg-recess hover:text-ink"><RotateCcw className="size-3.5" />Restore defaults</button>
        </div>
        <div className="min-h-0 overflow-y-auto px-5 pb-4 sm:px-6">
          <label className="relative my-4 block">
            <Search className="pointer-events-none absolute left-3 top-2.5 size-4 text-ink-dim" />
            <input autoFocus type="search" autoComplete="off" spellCheck={false} aria-label="Search keyboard shortcuts" value={search} onChange={event => { setSearch(event.target.value); setRecording(null); }} placeholder="Find a shortcut…" className="recess focus-ring w-full rounded-hw py-2 pl-9 pr-3 text-[12px] text-ink" />
          </label>
          <div className="min-h-5 text-[12px] leading-relaxed" aria-live="polite">
            {error ? <p role="alert" className="text-danger">{error}</p> : <p className="text-ink-dim">{recording === "openDownload" ? "Press a key, or middle-click/use a side mouse button on the binding. Modifiers are supported. Escape cancels." : recording ? "Hold your modifiers, then press a key. Escape cancels." : message || "Select a key binding to record a new combination."}</p>}
          </div>
          {(["Application", "Downloads"] as const).map(scope => {
            const actions = rows.filter(action => action.scope === scope);
            if (!actions.length) return null;
            return <section key={scope} aria-label={`${scope} shortcuts`} className="mt-5">
              <h3 className="legend border-b border-edge pb-2 text-[11px] text-ink-dim">{scope}</h3>
              <div className="divide-y divide-edge">
                {actions.map(action => {
                  const value = settings.actions[action.id];
                  const capturing = recording === action.id;
                  return <div key={action.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-2 py-3 sm:grid-cols-[minmax(0,1fr)_minmax(140px,auto)_44px]">
                    <div className="min-w-0"><p className={`text-[13px] ${settings.enabled && value.enabled ? "text-ink" : "text-ink-dim"}`}>{action.label}</p><p className="mt-0.5 text-[11px] leading-relaxed text-ink-dim">{action.description}</p></div>
                    <button type="button" aria-label={`Rebind ${action.label}`} aria-pressed={capturing} disabled={!settings.enabled}
                      onClick={() => { setRecording(capturing ? null : action.id); setModifiers(""); setError(""); setMessage(""); }}
                      onKeyDown={event => record(event, action.id)}
                      onMouseDown={event => {
                        if (capturing && action.id === "openDownload" && bindingFromEvent(event.nativeEvent)) event.preventDefault();
                      }}
                      onAuxClick={event => {
                        if (!capturing || action.id !== "openDownload") return;
                        const value = bindingFromEvent(event.nativeEvent);
                        if (!value) return;
                        event.preventDefault(); event.stopPropagation();
                        assignBinding(action.id, value);
                      }}
                      className={`focus-ring col-start-1 flex min-h-8 flex-wrap items-center justify-center gap-1 rounded-hw border px-2 py-1.5 text-[11px] disabled:opacity-45 sm:col-start-auto ${capturing ? "border-route bg-route/10 text-route" : "border-edge-strong bg-recess text-ink hover:border-route"}`}>
                      {capturing ? <span>{modifiers ? `${modifiers} + …` : "Press a key…"}</span> : value.bindings.map((b, i) => <span key={i} className="flex items-center gap-1">{i > 0 && <span className="text-ink-dim">or</span>}<kbd className="describer">{formatBinding(b)}</kbd></span>)}
                    </button>
                    <div className="col-start-2 row-start-1 justify-self-end sm:col-start-auto sm:row-start-auto"><Switch label={`Enable ${action.label}`} checked={value.enabled} disabled={!settings.enabled || capturing} onChange={enabled => commit({ ...settings, actions: { ...settings.actions, [action.id]: { ...value, enabled } } }, `${action.label} ${enabled ? "enabled" : "disabled"}.`)} /></div>
                  </div>;
                })}
              </div>
            </section>;
          })}
          {!rows.length && <p className="py-8 text-center text-[13px] text-ink-dim">No shortcuts match “{search}”.</p>}
          <section aria-label="Standard keyboard controls" className="mt-5 border-t border-edge pt-4">
            <h3 className="text-[12px] font-medium text-ink">Standard keyboard controls</h3>
            <p className="mt-1 text-[11px] leading-relaxed text-ink-dim">These stay available when shortcuts are off. App shortcuts pause while a dialog is open or you are typing.</p>
            <dl className="mt-3 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-[11px] text-ink-dim">
              <dt><kbd>Tab / Shift + Tab</kbd></dt><dd>Move between controls</dd>
              <dt><kbd>Escape</kbd></dt><dd>Close a dialog or menu; clear the download selection</dd>
              <dt><kbd>Enter / Space</kbd></dt><dd>Activate the focused control; Enter submits a link entry</dd>
              <dt><kbd>Shift + Enter</kbd></dt><dd>Add a new line to a link entry</dd>
              <dt><kbd>Ctrl / Cmd + V</kbd></dt><dd>Paste links or images</dd>
              <dt><kbd>Ctrl / Cmd + C / X / A</kbd></dt><dd>Copy, cut, or select text in an input</dd>
            </dl>
          </section>
        </div>
      </div>
    </dialog>
  );
}
