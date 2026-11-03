import type { KeyboardEvent } from "react";

/**
 * Arrow-key movement for an ARIA tablist, to pair with roving `tabIndex`
 * (`0` on the selected tab, `-1` on the rest). Selection follows focus, which
 * is what the tab pattern expects for panels that switch instantly.
 */
export function onTabKeyDown(event: KeyboardEvent<HTMLElement>): void {
  const { key } = event;
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(key)) return;
  const tabs = Array.from(
    event.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]:not([disabled])'),
  );
  if (tabs.length === 0) return;
  const current = tabs.findIndex((tab) => tab === document.activeElement);
  const last = tabs.length - 1;
  const next =
    key === "Home" ? 0
    : key === "End" ? last
    : key === "ArrowRight" ? (current < 0 || current === last ? 0 : current + 1)
    : (current <= 0 ? last : current - 1);
  event.preventDefault();
  const target = tabs[next];
  target?.focus();
  target?.click();
}
