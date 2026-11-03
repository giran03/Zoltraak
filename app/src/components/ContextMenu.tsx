import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Check } from "./Icons";

export type ContextMenuActionItem = {
  id: string;
  label: string;
  icon?: React.ReactNode;
  action: () => void;
  disabled?: boolean;
  danger?: boolean;
  bold?: boolean;
  /**
   * Renders the entry as a tick box. `undefined` is an ordinary command; the
   * icon slot holds the tick, so a checkable entry carries no icon of its own.
   */
  checked?: boolean;
  /**
   * Leave the menu open after this entry runs. For entries that are settings
   * rather than commands — turning six columns on should take one trip, not
   * six.
   */
  keepOpen?: boolean;
};

export type ContextMenuSeparator = {
  type: "separator";
};

export type ContextMenuHeading = {
  type: "heading";
  label: string;
};

export type ContextMenuItem = ContextMenuActionItem | ContextMenuSeparator | ContextMenuHeading;

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [coords, setCoords] = useState({ left: x, top: y });
  const [closing, setClosing] = useState(false);

  const triggerClose = () => {
    if (closing) return;
    setClosing(true);
    const rawDur =
      typeof window !== "undefined"
        ? getComputedStyle(document.documentElement).getPropertyValue("--dropdown-close-dur")
        : "";
    const closeMs = parseFloat(rawDur) || 150;
    setTimeout(onClose, closeMs);
  };

  // Clamp within viewport so menu never overflows off-screen
  useLayoutEffect(() => {
    if (!menuRef.current) return;
    const rect = menuRef.current.getBoundingClientRect();
    const padding = 12;
    let newLeft = x;
    let newTop = y;

    if (x + rect.width > window.innerWidth - padding) {
      newLeft = Math.max(padding, window.innerWidth - rect.width - padding);
    }
    if (y + rect.height > window.innerHeight - padding) {
      newTop = Math.max(padding, window.innerHeight - rect.height - padding);
    }

    setCoords({ left: newLeft, top: newTop });
  }, [x, y]);

  // Click outside, escape, scroll, blur dismiss listeners
  useEffect(() => {
    const handlePointerDown = (e: PointerEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        triggerClose();
      }
    };

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        triggerClose();
      }
    };

    // Scrolling the page behind the menu dismisses it, but a long menu is
    // allowed to scroll inside itself — the column list is taller than a short
    // window, and closing on the first wheel tick would make its tail
    // unreachable.
    const handleScroll = (e: Event) => {
      if (menuRef.current?.contains(e.target as Node)) return;
      triggerClose();
    };

    window.addEventListener("pointerdown", handlePointerDown, true);
    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("wheel", handleScroll, { passive: true });
    window.addEventListener("blur", triggerClose);

    return () => {
      window.removeEventListener("pointerdown", handlePointerDown, true);
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("wheel", handleScroll);
      window.removeEventListener("blur", triggerClose);
    };
  }, [onClose, closing]);

  return (
    <div
      ref={menuRef}
      role="menu"
      style={{ left: `${coords.left}px`, top: `${coords.top}px` }}
      className={`t-dropdown ${closing ? "is-closing" : "is-open"} plate lift context-menu-glass fixed z-50 max-h-[min(70vh,560px)] min-w-[210px] select-none overflow-y-auto rounded-hw-lg py-1`}
    >
      {items.map((item, idx) => {
        if ("type" in item && item.type === "separator") {
          return (
            <div
              key={`sep-${idx}`}
              className="my-1 h-px bg-edge"
              role="separator"
            />
          );
        }

        if ("type" in item && item.type === "heading") {
          return (
            <p
              key={`heading-${idx}`}
              className="legend px-3 pb-1 pt-1.5 text-[11px] text-ink-faint"
            >
              {item.label}
            </p>
          );
        }

        const actionItem = item as ContextMenuActionItem;
        const checkable = actionItem.checked !== undefined;
        const disabled = actionItem.disabled;

        return (
          <button
            key={actionItem.id}
            type="button"
            role={checkable ? "menuitemcheckbox" : "menuitem"}
            aria-checked={checkable ? actionItem.checked : undefined}
            disabled={disabled}
            onClick={() => {
              if (disabled) return;
              actionItem.action();
              if (!actionItem.keepOpen) triggerClose();
            }}
            className={`flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-[12px] transition-colors ${
              disabled
                ? "cursor-not-allowed text-ink-faint opacity-55"
                : actionItem.danger
                  ? "text-danger hover:bg-danger hover:text-on-signal"
                  : actionItem.bold
                    ? "text-ink hover:bg-route hover:text-on-signal"
                    : "text-ink-dim hover:bg-route hover:text-on-signal"
            }`}
          >
            {checkable ? (
              <span className="flex size-4 shrink-0 items-center justify-center">
                <Check
                  className={`size-3 ${actionItem.checked ? "" : "invisible"}`}
                />
              </span>
            ) : (
              actionItem.icon && (
                <span className="flex size-4 shrink-0 items-center justify-center">
                  {actionItem.icon}
                </span>
              )
            )}
            <span className="flex-1 truncate">{actionItem.label}</span>
          </button>
        );
      })}
    </div>
  );
}
