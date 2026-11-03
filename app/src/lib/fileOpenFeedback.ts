import type { AppearanceSettings } from "./useAppearance";

/** Acknowledge an open request without delaying it or changing row selection. */
export function flashFileOpen(element: HTMLElement | null, settings: AppearanceSettings) {
  if (!element) return;
  for (const animation of element.getAnimations()) {
    if (animation.id === "file-open-feedback") animation.cancel();
  }
  if (!settings.fileOpenFeedback || settings.motion === "reduced") return;
  const style = getComputedStyle(element);
  const accent = style.getPropertyValue("--c-route").trim();
  const duration = matchMedia("(prefers-reduced-motion: reduce)").matches
    ? Math.min(100, settings.fileOpenFeedbackDuration)
    : settings.fileOpenFeedbackDuration;
  element.animate([
    { boxShadow: `inset 0 0 0 100vmax color-mix(in srgb, ${accent} 14%, transparent)`, offset: 0 },
    { boxShadow: `inset 0 0 0 100vmax color-mix(in srgb, ${accent} 14%, transparent)`, offset: 0.2 },
    { boxShadow: "inset 0 0 0 100vmax transparent", offset: 1 },
  ], { id: "file-open-feedback", duration, easing: "ease-out" });
}
