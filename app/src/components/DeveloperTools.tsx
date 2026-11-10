import { useRef } from "react";
import { Button, Section } from "./Controls";
import type { ToastItem } from "./Toast";
import { triggerConfetti } from "../lib/confetti";
import { testActiveSound } from "../lib/completionSound";

const SAMPLE_TYPES: ToastItem["type"][] = ["success", "error", "info"];
const STACK_STAGGER_MS = 180;
const STACK_COUNT = 6;
const LONG_MESSAGE = "This is deliberately long so wrapping, truncation and card growth can be checked: "
  + "download-with-a-very-long-file-name-that-keeps-going-and-going_final_v2_(copy)_2026-11-10.mkv could not be written to the destination folder.";

/** One labelled test with its trigger, so each row says what it exercises. */
function ToolRow({ title, description, children }: { title: string; description: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col items-start justify-between gap-3 border-b border-edge py-3 last:border-b-0 sm:flex-row sm:items-center">
      <div className="min-w-0">
        <p className="text-[13px] font-medium text-ink">{title}</p>
        <p className="mt-0.5 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">{description}</p>
      </div>
      <div className="flex shrink-0 flex-wrap gap-2">{children}</div>
    </div>
  );
}

/** Quick triggers for app features that are otherwise hard to reach on demand. */
export function DeveloperTools({ onAddToast }: { onAddToast?: (toast: ToastItem) => void }) {
  const counter = useRef(0);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);

  const toast = (overrides: Partial<ToastItem> & Pick<ToastItem, "type">) => {
    counter.current += 1;
    onAddToast?.({
      id: `dev-test-${Date.now()}-${counter.current}`,
      title: `Test ${overrides.type} notification`,
      message: "Sent from Developer tools.",
      ...overrides,
    });
  };

  const stack = () => {
    timers.current.forEach(clearTimeout);
    timers.current = Array.from({ length: STACK_COUNT }, (_, index) =>
      setTimeout(() => toast({
        type: SAMPLE_TYPES[index % SAMPLE_TYPES.length] ?? "info",
        title: `Stacked toast ${index + 1} of ${STACK_COUNT}`,
        message: "Checks stacking, the visible limit and history.",
      }), index * STACK_STAGGER_MS));
  };

  return (
    <Section
      id="section-developer-tools"
      title="Developer tools"
      description="Trigger app features on demand to check how they look and behave. Test notifications are also added to the notification history."
    >
      <div className="border border-edge bg-face/50 px-4">
        <ToolRow title="Toast types" description="One notification of each kind.">
          {SAMPLE_TYPES.map(type => <Button key={type} tone="ghost" onClick={() => toast({ type })}>{type}</Button>)}
        </ToolRow>
        <ToolRow title="Toast stack" description={`${STACK_COUNT} toasts in quick succession to check stacking and the visible limit.`}>
          <Button tone="ghost" onClick={stack}>Fire stack</Button>
        </ToolRow>
        <ToolRow title="Toast variants" description="Long text, action and secondary buttons, and a short or long lifetime.">
          <Button tone="ghost" onClick={() => toast({ type: "error", title: "Long message", message: LONG_MESSAGE })}>Long text</Button>
          <Button tone="ghost" onClick={() => toast({
            type: "info", title: "Action toast", message: "Has an action and a dismiss label.",
            action: { label: "Undo", onClick: () => toast({ type: "success", title: "Undo clicked", message: "Action callback ran." }) },
            dismissLabel: "Keep",
          })}>With action</Button>
          <Button tone="ghost" onClick={() => toast({ type: "info", title: "Short lifetime", message: "Disappears after 1.5 seconds.", durationMs: 1500 })}>1.5 s</Button>
          <Button tone="ghost" onClick={() => toast({ type: "info", title: "Long lifetime", message: "Disappears after 15 seconds.", durationMs: 15000 })}>15 s</Button>
        </ToolRow>
        <ToolRow title="Completion effects" description="Play the finished-download confetti and sound, ignoring their on/off settings.">
          <Button tone="ghost" onClick={() => triggerConfetti(true)}>Confetti</Button>
          <Button tone="ghost" onClick={() => { testActiveSound(); }}>Sound</Button>
        </ToolRow>
      </div>
    </Section>
  );
}
