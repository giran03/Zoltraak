import { forwardRef, useCallback, useEffect, useId, useImperativeHandle, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

export interface TooltipGroupProps extends React.HTMLAttributes<HTMLDivElement> {
  children: ReactNode;
  side?: "top" | "bottom";
  className?: string;
  disabled?: boolean;
}

/**
 * Tooltip open/close component based on https://transitions.dev/detail.html?t=tooltip-open-close
 *
 * A hover/focus tooltip shared by a group of triggers. It fades + scales in with
 * an appear-delay, disappears immediately on leave, and when the pointer moves to a
 * neighbouring trigger, the single bubble moves and resizes to the new target.
 * Geometry updates immediately so long text stays inside the window.
 */
export const TooltipGroup = forwardRef<HTMLDivElement, TooltipGroupProps>(
  function TooltipGroup(
    {
      children,
      side = "top",
      className = "",
      disabled = false,
      onPointerOver,
      onPointerOut,
      onFocusCapture,
      onBlurCapture,
      ...props
    },
    forwardedRef,
  ) {
    const groupRef = useRef<HTMLDivElement>(null);
    useImperativeHandle(forwardedRef, () => groupRef.current as HTMLDivElement);
    const tipRef = useRef<HTMLSpanElement>(null);
    const textRef = useRef<HTMLSpanElement>(null);
    const triggerRef = useRef<HTMLElement | null>(null);
    // React owns the tooltip inside this stable portal host; the host can follow
    // its dialog without placing the tooltip in the modal's inert background.
    const [portalHost] = useState(() => document.createElement("div"));
    const tipId = useId();

    const hide = useCallback(() => {
      if (!tipRef.current) return;
      tipRef.current.setAttribute("data-show", "false");
      tipRef.current.setAttribute("aria-hidden", "true");
      if (typeof tipRef.current.hidePopover === "function" && tipRef.current.matches(":popover-open")) tipRef.current.hidePopover();
      const trigger = triggerRef.current;
      if (trigger) {
        const descriptions = (trigger.getAttribute("aria-describedby") || "").split(/\s+/).filter(id => id && id !== tipId);
        if (descriptions.length) trigger.setAttribute("aria-describedby", descriptions.join(" "));
        else trigger.removeAttribute("aria-describedby");
        const title = trigger.getAttribute("data-original-title");
        if (title) trigger.setAttribute("title", title);
      }
      triggerRef.current = null;
    }, [tipId]);

    const place = useCallback(
      (trigger: HTMLElement) => {
        if (disabled) return;
        if (!groupRef.current || !tipRef.current || !textRef.current) return;
        const tip = tipRef.current;
        const text = textRef.current;

        const owner = trigger.closest("dialog[open]") || document.body;
        if (portalHost.parentElement !== owner) {
          if (typeof tip.hidePopover === "function" && tip.matches(":popover-open")) tip.hidePopover();
          owner.appendChild(portalHost);
        }

        const label = trigger.getAttribute("data-tooltip") || trigger.getAttribute("title") || "";
        if (!label) return;
        if (triggerRef.current && triggerRef.current !== trigger) hide();
        triggerRef.current = trigger;
        const descriptions = new Set((trigger.getAttribute("aria-describedby") || "").split(/\s+/).filter(Boolean));
        descriptions.add(tipId);
        trigger.setAttribute("aria-describedby", [...descriptions].join(" "));

        // Suppress native browser tooltip while our custom animated tooltip is active
        if (trigger.hasAttribute("title")) {
          trigger.setAttribute("data-original-title", trigger.getAttribute("title") || "");
          trigger.removeAttribute("title");
        }

        const showing = tip.getAttribute("data-show") === "true";
        text.textContent = label;
        if (typeof tip.showPopover === "function" && !tip.matches(":popover-open")) tip.showPopover();

        const r = trigger.getBoundingClientRect();
        // Measure the fully wrapped bubble, independently of the trigger group.
        const width = tip.offsetWidth;
        const tipHeight = tip.offsetHeight;

        // Check space above and below relative to group and viewport
        const spaceAbove = r.top - 8;
        const spaceBelow = window.innerHeight - 8 - r.bottom;

        let effectiveSide = side;
        if (side === "top" && spaceAbove < tipHeight + 6 && spaceBelow > spaceAbove) {
          effectiveSide = "bottom";
        } else if (side === "bottom" && spaceBelow < tipHeight + 6 && spaceAbove > spaceBelow) {
          effectiveSide = "top";
        }
        tip.setAttribute("data-side", effectiveSide);

        const x = Math.max(8, Math.min(r.left + r.width / 2 - width / 2, window.innerWidth - width - 8));
        const preferredY = effectiveSide === "top" ? r.top - tipHeight - 6 : r.bottom + 6;
        const y = Math.max(8, Math.min(preferredY, window.innerHeight - tipHeight - 8));

        if (!showing) {
          // Snap geometry while hidden so only the appear plays
          tip.style.transition = "none";
          tip.style.setProperty("--tt-x", `${x}px`);
          tip.style.setProperty("--tt-y", `${y}px`);
          void tip.offsetWidth; // force reflow
          tip.style.transition = "";
        } else {
          tip.style.setProperty("--tt-x", `${x}px`);
          tip.style.setProperty("--tt-y", `${y}px`);
        }

        tip.setAttribute("data-show", "true");
        tip.setAttribute("aria-hidden", "false");
      },
      [side, hide, tipId, portalHost, disabled],
    );

    useEffect(() => { if (disabled) hide(); }, [disabled, hide]);

    const handlePointerOver = useCallback(
      (event: React.PointerEvent<HTMLDivElement>) => {
        onPointerOver?.(event);
        if (event.defaultPrevented) return;
        const target = (event.target as HTMLElement).closest(".t-tt-trigger") as HTMLElement | null;
        if (target && target.closest(".t-tt-group") === groupRef.current) {
          place(target);
        }
      },
      [place, onPointerOver],
    );

    const handlePointerOut = useCallback(
      (event: React.PointerEvent<HTMLDivElement>) => {
        onPointerOut?.(event);
        const target = (event.target as HTMLElement).closest(".t-tt-trigger") as HTMLElement | null;
        if (target) {
          // Restore title if it was saved
          const orig = target.getAttribute("data-original-title");
          if (orig) {
            target.setAttribute("title", orig);
          }
        }
        const nextTarget = (event.relatedTarget as HTMLElement | null)?.closest?.(".t-tt-trigger");
        if (!nextTarget || nextTarget.closest(".t-tt-group") !== groupRef.current) {
          hide();
        }
      },
      [hide, onPointerOut],
    );

    const handleFocusIn = useCallback(
      (event: React.FocusEvent<HTMLDivElement>) => {
        onFocusCapture?.(event);
        if (event.defaultPrevented) return;
        const target = (event.target as HTMLElement).closest(".t-tt-trigger") as HTMLElement | null;
        if (target && target.closest(".t-tt-group") === groupRef.current) {
          place(target);
        }
      },
      [place, onFocusCapture],
    );

    const handleFocusOut = useCallback(
      (event: React.FocusEvent<HTMLDivElement>) => {
        onBlurCapture?.(event);
        const nextTarget = (event.relatedTarget as HTMLElement | null)?.closest?.(".t-tt-trigger");
        if (!nextTarget || nextTarget.closest(".t-tt-group") !== groupRef.current) {
          hide();
        }
      },
      [hide, onBlurCapture],
    );

    useEffect(() => {
      if (!portalHost.isConnected) document.body.appendChild(portalHost);
      const onDialogClose = (event: Event) => {
        if (event.target instanceof HTMLDialogElement && event.target.contains(portalHost)) hide();
      };
      const onScroll = (event: Event) => {
        if (!tipRef.current?.contains(event.target as Node)) hide();
      };
      const onResize = () => { if (triggerRef.current) place(triggerRef.current); };
      const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") hide(); };
      window.addEventListener("scroll", onScroll, true);
      window.addEventListener("resize", onResize);
      window.addEventListener("keydown", onKeyDown);
      document.addEventListener("close", onDialogClose, true);
      return () => {
        hide();
        window.removeEventListener("scroll", onScroll, true);
        window.removeEventListener("resize", onResize);
        window.removeEventListener("keydown", onKeyDown);
        document.removeEventListener("close", onDialogClose, true);
        portalHost.remove();
      };
    }, [hide, place, portalHost]);

    return (
      <div
        ref={groupRef}
        className={`t-tt-group ${className}`}
        onPointerOver={handlePointerOver}
        onPointerOut={handlePointerOut}
        onFocusCapture={handleFocusIn}
        onBlurCapture={handleFocusOut}
        {...props}
      >
        {children}
        {createPortal(<span
          ref={tipRef}
          id={tipId}
          role="tooltip"
          popover={typeof HTMLElement !== "undefined" && "popover" in HTMLElement.prototype ? "manual" : undefined}
          aria-hidden="true"
          data-show="false"
          data-side={side}
          className="t-tt"
        >
          <span ref={textRef} className="t-tt-text" />
        </span>, portalHost)}
      </div>
    );
  },
);
