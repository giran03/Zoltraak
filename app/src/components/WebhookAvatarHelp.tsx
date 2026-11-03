import { useEffect, useId, useRef, useState } from "react";
import { Button } from "./Controls";
import { Close, Info } from "./Icons";
import { TooltipGroup } from "./Tooltip";

function AvatarGuide({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const summary = useId();
  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    const frame = requestAnimationFrame(() => element.classList.add("is-open"));
    return () => {
      cancelAnimationFrame(frame);
      element.close();
      if (previous?.isConnected) previous.focus();
    };
  }, []);
  return <dialog ref={dialog} className="conversion-help-dialog t-modal" aria-labelledby={title} aria-describedby={summary}
    onCancel={event => { event.preventDefault(); onClose(); }}
    onClick={event => {
      if (event.target !== event.currentTarget) return;
      const rect = event.currentTarget.getBoundingClientRect();
      if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose();
    }}>
    <header className="flex items-start justify-between gap-3 border-b border-edge px-5 py-4">
      <h2 id={title} className="text-[17px] font-medium text-ink">Webhook avatar guide</h2>
      <button type="button" aria-label="Close webhook avatar guide" onClick={onClose} className="entry-icon-button focus-ring -mr-1 -mt-1"><span className="entry-icon-glyph"><Close className="size-4" /></span></button>
    </header>
    <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4 text-[13px] leading-relaxed text-ink-dim">
      <p id={summary}>Use the webhook’s existing image, or choose a public image link for messages sent by Zoltraak.</p>
      <h3 className="mt-4 font-medium text-ink">Use webhook default</h3>
      <p className="mt-1">Choose this option and save to use the avatar configured in Discord. To change that image in Discord, open Server Settings → Integrations → Webhooks and select your webhook.</p>
      <h3 className="mt-4 font-medium text-ink">Use an image uploaded to Discord</h3>
      <ol className="mt-2 list-decimal space-y-2 pl-5">
        <li>Upload and send your image in a Discord channel you control.</li>
        <li>Open the uploaded image, then right-click it and choose Copy Link or Copy Image Address. Copy the image link, rather than the message link.</li>
        <li>In Zoltraak, choose Use public image URL, paste the copied image link, and select Save sender.</li>
      </ol>
      <p className="mt-4">You can also use a direct HTTP or HTTPS image link from another host. A local file path or a webpage that requires sign-in will not work.</p>
      <p className="mt-4 border-t border-edge pt-3 text-[12px] text-ink-faint">Keep the original Discord image message. If the image stops appearing, copy a fresh image link or switch to the webhook default. The public image URL overrides messages sent by Zoltraak without changing the webhook’s stored avatar.</p>
    </div>
    <footer className="flex justify-end border-t border-edge px-5 py-3"><Button tone="ghost" onClick={onClose}>Got it</Button></footer>
  </dialog>;
}

export function WebhookAvatarHelp() {
  const [open, setOpen] = useState(false);
  const [tooltipSuppressed, setTooltipSuppressed] = useState(false);
  return <>
    <TooltipGroup className="inline-flex shrink-0" side="top" disabled={open || tooltipSuppressed}>
      <button type="button" aria-label="About webhook avatars" aria-haspopup="dialog" data-tooltip="About webhook avatars"
        onClick={() => { setTooltipSuppressed(true); setOpen(true); }}
        onPointerLeave={() => { if (!open) setTooltipSuppressed(false); }} onBlur={() => { if (!open) setTooltipSuppressed(false); }}
        className="t-tt-trigger focus-ring inline-flex size-6 items-center justify-center rounded-full text-ink-faint hover:bg-route/10 hover:text-route"><Info className="size-3.5" /></button>
    </TooltipGroup>
    {open && <AvatarGuide onClose={() => setOpen(false)} />}
  </>;
}
