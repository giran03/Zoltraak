import { useLayoutEffect, useRef, type ElementType, type HTMLAttributes, type ReactNode } from "react";

/** What marks the chosen item of a group, across the ways this app expresses "selected". */
const ACTIVE_ITEM =
  '[aria-pressed="true"],[aria-selected="true"],[aria-checked="true"],[aria-current="page"],[aria-current="true"],[data-active="true"]';

type SlideGroupProps = HTMLAttributes<HTMLElement> & {
  as?: ElementType;
  /** Classes for the moving highlight: its colour, border and corners. */
  indicatorClassName?: string;
  children: ReactNode;
};

/**
 * A row or column of choices whose highlight slides to the one chosen instead
 * of switching on in place.
 *
 * The highlight is one element behind the items. It is moved to the chosen item
 * by measuring it, so it follows wrapping rows, scrolling tab strips and items
 * of different widths. The slide itself is only a CSS transition on the
 * animation tokens (`--duration-fast`, `--ease-smooth-out`), so the Interface
 * Motion setting and the system reduced-motion preference govern it exactly as
 * they do every other transition: subtle shortens it and reduced makes it a cut.
 *
 * Items mark themselves chosen with `aria-pressed`, `aria-selected`,
 * `aria-checked`, `aria-current` or `data-active`, and draw no background of
 * their own for the chosen state.
 */
export function SlideGroup({ as: Tag = "div", indicatorClassName = "bg-route", className = "", children, ...rest }: SlideGroupProps) {
  const root = useRef<HTMLElement>(null);

  // After every render: the chosen item may have changed or moved.
  useLayoutEffect(() => {
    const group = root.current;
    const indicator = group?.querySelector<HTMLElement>(":scope > [data-slide-indicator]");
    if (!group || !indicator) return;

    const place = () => {
      // The group's own items first; a nested control elsewhere inside never wins.
      const chosen = Array.from(group.children).find((item): item is HTMLElement => item !== indicator && item instanceof HTMLElement && item.matches(ACTIVE_ITEM))
        ?? group.querySelector<HTMLElement>(ACTIVE_ITEM);
      if (!chosen) {
        indicator.dataset.visible = "false";
        return;
      }
      const outer = group.getBoundingClientRect();
      const box = chosen.getBoundingClientRect();
      // Relative to the group's padding box, and carried along when it scrolls.
      const x = box.left - outer.left - group.clientLeft + group.scrollLeft;
      const y = box.top - outer.top - group.clientTop + group.scrollTop;
      indicator.style.transform = `translate(${x}px, ${y}px)`;
      indicator.style.width = `${box.width}px`;
      indicator.style.height = `${box.height}px`;
      indicator.dataset.visible = "true";
    };

    place();
    // The first placement is a cut: the highlight should appear on the chosen
    // item, not slide in from the corner.
    // (A timer rather than an animation frame: frames pause in a hidden window.)
    const arm = window.setTimeout(() => {
      indicator.dataset.ready = "true";
    }, 40);

    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);
    observer?.observe(group);
    void document.fonts?.ready.then(place).catch(() => undefined);
    return () => {
      window.clearTimeout(arm);
      observer?.disconnect();
    };
  });

  return (
    <Tag ref={root} className={`slide-group relative ${className}`} {...rest}>
      <span aria-hidden="true" data-slide-indicator="" className={`slide-indicator ${indicatorClassName}`} />
      {children}
    </Tag>
  );
}
