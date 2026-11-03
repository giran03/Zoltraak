import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "./Icons";
import { FaultSlug, Legend } from "./Panel";
import { describeSelectOption, groupSelectOptions, type ParsedSelectOption, type SelectOption } from "../lib/conversion";
import { useFocusedDismiss } from "../lib/useFocusedDismiss";

export { describeSelectOption, groupSelectOptions, type ParsedSelectOption, type SelectOption };

/**
 * Panel hardware.
 *
 * Every control on the desk is cut from the same stock: a recessed well for
 * anything you type into, an engraved legend naming it, and a route-blue
 * indication for the one that is set. Defined once so Rules, Settings,
 * Scheduler and Extension cannot drift into four different-looking forms.
 */

const WELL =
  "recess focus-ring w-full rounded-hw px-2.5 py-1.5 text-[13px] text-ink outline-none placeholder:text-ink-faint disabled:cursor-not-allowed disabled:opacity-55";

export function TextInput({
  value,
  onChange,
  onBlur,
  placeholder,
  mono,
  disabled,
  type = "text",
  inputMode,
  ariaLabel,
  className = "",
}: {
  value: string;
  onChange: (value: string) => void;
  /** Autosaving forms commit here — see `useAutosaveSettings`. */
  onBlur?: () => void;
  placeholder?: string;
  mono?: boolean;
  disabled?: boolean;
  type?: string;
  inputMode?: "numeric" | "text";
  ariaLabel?: string;
  className?: string;
}) {
  return (
    <input
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      onBlur={onBlur}
      placeholder={placeholder}
      disabled={disabled}
      inputMode={inputMode}
      aria-label={ariaLabel}
      autoComplete="off"
      spellCheck={false}
      className={`${WELL} ${mono ? "font-mono text-[12px]" : ""} ${className}`}
    />
  );
}

export function SelectInput<T extends string>({
  value,
  onChange,
  options,
  ariaLabel,
  name,
  disabled,
  size = "md",
  className = "",
}: {
  value: T;
  onChange: (value: T) => void;
  options: SelectOption<T>[];
  disabled?: boolean;
  ariaLabel?: string;
  name?: string;
  size?: "sm" | "md";
  className?: string;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const selectRef = useRef<HTMLSelectElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const listboxId = useId();
  const [open, setOpen] = useState(false);
  const parsed = useMemo(() => options.map(describeSelectOption), [options]);
  const sections = useMemo(() => groupSelectOptions(parsed), [parsed]);
  const selectedIndex = useMemo(() => {
    const match = parsed.findIndex((item) => item.value === value);
    return match >= 0 ? match : 0;
  }, [parsed, value]);
  const [activeIndex, setActiveIndex] = useState(selectedIndex);
  const [menuBox, setMenuBox] = useState<{
    top?: number;
    bottom?: number;
    left: number;
    minWidth: number;
    maxWidth: number;
    maxHeight: number;
  }>({ top: 0, left: 0, minWidth: 180, maxWidth: 420, maxHeight: 340 });

  useEffect(() => {
    if (disabled && open) setOpen(false);
  }, [disabled, open]);

  useEffect(() => {
    setActiveIndex(selectedIndex);
  }, [selectedIndex, open]);

  useLayoutEffect(() => {
    if (!open) return;
    function updatePosition() {
      const el = rootRef.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const minWidth = Math.min(Math.max(rect.width, size === "sm" ? 160 : 220), vw - 16);
      const maxWidth = Math.min(Math.max(rect.width, 420), vw - 16);
      const left = Math.max(8, Math.min(rect.left, vw - minWidth - 8));
      const spaceBelow = vh - rect.bottom - 10;
      const spaceAbove = rect.top - 10;
      const placeAbove = spaceBelow < 220 && spaceAbove > spaceBelow;
      const maxHeight = Math.min(540, Math.max(150, placeAbove ? spaceAbove : spaceBelow));
      if (placeAbove) {
        setMenuBox({ bottom: vh - rect.top + 4, left, minWidth, maxWidth, maxHeight });
      } else {
        setMenuBox({ top: rect.bottom + 4, left, minWidth, maxWidth, maxHeight });
      }
    }
    updatePosition();
    window.addEventListener("resize", updatePosition);
    window.addEventListener("scroll", updatePosition, true);
    return () => {
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("scroll", updatePosition, true);
    };
  }, [open, size, options.length]);

  useEffect(() => {
    if (!open) return;
    const activeButton = itemRefs.current[activeIndex];
    activeButton?.scrollIntoView({ block: "nearest" });
  }, [open, activeIndex]);

  useEffect(() => {
    if (!open) return;
    function onPointerDown(event: PointerEvent) {
      const target = event.target as Node | null;
      if (!target) return;
      if (rootRef.current?.contains(target) || menuRef.current?.contains(target)) return;
      setOpen(false);
    }
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => window.removeEventListener("pointerdown", onPointerDown, true);
  }, [open]);

  function stepEnabled(from: number, delta: 1 | -1): number {
    if (!parsed.length) return 0;
    let cursor = from;
    for (let step = 0; step < parsed.length; step++) {
      cursor = (cursor + delta + parsed.length) % parsed.length;
      if (!parsed[cursor]?.disabled) return cursor;
    }
    return from;
  }

  function boundaryEnabled(first: boolean): number {
    if (first) {
      const idx = parsed.findIndex((item) => !item.disabled);
      return idx >= 0 ? idx : 0;
    }
    for (let i = parsed.length - 1; i >= 0; i--) {
      if (!parsed[i]?.disabled) return i;
    }
    return 0;
  }

  function handleMouseDown(event: ReactMouseEvent<HTMLSelectElement>) {
    event.preventDefault();
    if (disabled) return;
    event.currentTarget.focus({ preventScroll: true });
    setOpen((previous) => !previous);
  }

  function handleClick(event: ReactMouseEvent<HTMLSelectElement>) {
    if (event.detail === 0 && !disabled) {
      event.preventDefault();
      setOpen((previous) => !previous);
    }
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLSelectElement>) {
    if (disabled) return;
    if (event.key === "Escape") {
      if (open) {
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
      }
      return;
    }
    if (event.key === "Tab") {
      if (open) setOpen(false);
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const delta = event.key === "ArrowDown" ? 1 : -1;
      if (!open) {
        setOpen(true);
        setActiveIndex(parsed[selectedIndex]?.disabled ? stepEnabled(selectedIndex, delta) : selectedIndex);
      } else {
        setActiveIndex((previous) => stepEnabled(previous, delta));
      }
      return;
    }
    if (event.key === "Home" && open) {
      event.preventDefault();
      setActiveIndex(boundaryEnabled(true));
      return;
    }
    if (event.key === "End" && open) {
      event.preventDefault();
      setActiveIndex(boundaryEnabled(false));
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (!open) {
        setOpen(true);
      } else {
        const chosen = parsed[activeIndex];
        if (chosen && !chosen.disabled) {
          onChange(chosen.value);
          setOpen(false);
        }
      }
    }
  }

  const current = parsed[selectedIndex] ?? parsed[0];
  const portalTarget = typeof document !== "undefined" ? (rootRef.current?.closest("dialog") ?? document.body) : null;

  return (
    <div
      ref={rootRef}
      className={`select-control relative ${size === "sm" ? "inline-flex" : "w-full"} ${className}`}
      data-open={open || undefined}
    >
      <select
        ref={selectRef}
        name={name}
        disabled={disabled}
        value={value}
        onChange={(e) => {
          onChange(e.target.value as T);
          setOpen(false);
        }}
        onMouseDown={handleMouseDown}
        onClick={handleClick}
        onKeyDown={handleKeyDown}
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={open ? listboxId : undefined}
        className="select-native peer absolute inset-0 z-10 size-full cursor-pointer appearance-none opacity-0 disabled:cursor-not-allowed"
      >
        {options.map((option) => (
          <option key={option.value} value={option.value} disabled={option.disabled} className="bg-face text-ink">
            {option.label}
          </option>
        ))}
      </select>
      <div
        aria-hidden="true"
        className={`select-face recess flex w-full items-center justify-between gap-2 rounded-hw ${
          size === "sm" ? "px-2 py-1 text-[11px]" : "px-2.5 py-1.5 text-[13px]"
        } text-ink ${open ? "border-route" : ""} ${disabled ? "opacity-55" : ""}`}
      >
        <span className="min-w-0 flex-1 truncate text-left">
          {current ? (
            current.group ? (
              <>
                <span className="text-ink-dim">{current.group} · </span>
                <span className="font-medium text-ink">{current.primary}</span>
                {current.detail ? (
                  <span className="text-ink-dim"> ({current.detail})</span>
                ) : current.badge ? (
                  <span className="ml-1 text-ink-dim">{current.badge}</span>
                ) : null}
              </>
            ) : (
              <>
                <span className="font-medium text-ink">{current.primary}</span>
                {current.badge && <span className="ml-1 text-ink-dim">{current.badge}</span>}
                {current.detail && <span className="text-ink-dim"> · {current.detail}</span>}
              </>
            )
          ) : null}
        </span>
        <ChevronDown
          className={`select-chevron ${size === "sm" ? "size-3.5" : "size-4"} shrink-0 ${
            open ? "rotate-180 text-route" : "text-ink-dim"
          }`}
        />
      </div>
      {open &&
        !disabled &&
        portalTarget &&
        createPortal(
          <div
            ref={menuRef}
            id={listboxId}
            role="listbox"
            aria-label={ariaLabel}
            className="select-popover plate lift fixed z-[9995] overflow-y-auto rounded-hw-xl border border-edge-strong bg-face px-2 py-2 text-ink"
            style={{
              top: menuBox.top,
              bottom: menuBox.bottom,
              left: menuBox.left,
              minWidth: menuBox.minWidth,
              maxWidth: menuBox.maxWidth,
              maxHeight: menuBox.maxHeight,
            }}
          >
            {sections.map((section, sectionIndex) => (
              <div key={section.group ?? `section-${sectionIndex}`} className={sectionIndex > 0 ? "mt-2" : ""}>
                {section.group && (
                  <div className="legend select-none px-2.5 pt-1 pb-1 text-[11px] text-ink-faint">
                    {section.group}
                  </div>
                )}
                {section.items.map(({ option, index }) => {
                  const isSelected = option.value === value;
                  const isActive = index === activeIndex;
                  return (
                    <button
                      key={option.value}
                      ref={(el) => {
                        itemRefs.current[index] = el;
                      }}
                      type="button"
                      role="option"
                      aria-selected={isSelected}
                      aria-disabled={option.disabled || undefined}
                      disabled={option.disabled}
                      onMouseEnter={() => {
                        if (!option.disabled) setActiveIndex(index);
                      }}
                      onPointerMove={() => {
                        if (!option.disabled && activeIndex !== index) setActiveIndex(index);
                      }}
                      onMouseDown={(event) => event.preventDefault()}
                      onClick={() => {
                        if (option.disabled) return;
                        onChange(option.value);
                        setOpen(false);
                        selectRef.current?.focus({ preventScroll: true });
                      }}
                      className={`select-option flex w-full items-center justify-between gap-3 rounded-hw-xl px-2.5 py-1.5 text-left ${
                        size === "sm" ? "text-[12px]" : "text-[13px]"
                      } leading-snug transition-colors ${
                        option.disabled ? "cursor-not-allowed opacity-40" : "cursor-pointer"
                      } ${
                        isActive && !option.disabled ? "select-option-active text-ink" : "text-ink"
                      }`}
                    >
                      <span className="min-w-0 flex-1 break-words">
                        <span className="font-medium text-ink">{option.primary}</span>
                        {option.badge && <span className="ml-1 font-normal text-ink-dim">{option.badge}</span>}
                        {option.detail && (
                          <>
                            <span className="mx-1.5 text-ink-faint" aria-hidden="true">
                              •
                            </span>
                            <span className="font-normal text-ink-dim">{option.detail}</span>
                          </>
                        )}
                      </span>
                      {isSelected && <Check className="size-3.5 shrink-0 text-route" />}
                    </button>
                  );
                })}
              </div>
            ))}
          </div>,
          portalTarget,
        )}
    </div>
  );
}

export function Button({
  children,
  onClick,
  type = "button",
  tone = "primary",
  disabled,
  className = "",
}: {
  children: ReactNode;
  onClick?: () => void;
  type?: "button" | "submit";
  tone?: "primary" | "ghost" | "danger";
  disabled?: boolean;
  className?: string;
}) {
  const skin =
    tone === "primary"
      ? "border-route bg-route text-on-signal hover:brightness-110"
      : tone === "danger"
        ? "border-danger/45 text-danger hover:bg-danger hover:text-on-signal"
        : "border-edge-strong text-ink-dim hover:bg-face hover:text-ink";

  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled}
      className={`focus-ring legend shrink-0 rounded-hw border px-4 py-[7px] text-[11px] transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${skin} ${className}`}
    >
      {children}
    </button>
  );
}

/** Shared transitions.dev switch; mounting never plays the return animation. */
export function Switch({
  checked,
  onChange,
  label,
  disabled,
  descriptionId,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
  descriptionId?: string;
}) {
  const [interacted, setInteracted] = useState(false);
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-describedby={descriptionId}
      disabled={disabled}
      data-on={checked ? "true" : "false"}
      onClick={() => { setInteracted(true); onChange(!checked); }}
      className={`t-toggle focus-ring shrink-0 disabled:cursor-not-allowed disabled:opacity-45${interacted ? " is-init" : ""}`}
    >
      <span aria-hidden="true" className="t-toggle-thumb" />
    </button>
  );
}

/** Native numeric editing with theme-aware step buttons. */
export function NumberInput({ value, onChange, ariaLabel, min, max, step = "any", required = false, disabled = false, size = "md", className = "w-full", ...inputProps }: {
  value: string;
  onChange: (value: string) => void;
  ariaLabel: string;
  min?: number;
  max?: number;
  step?: number | "any";
  required?: boolean;
  disabled?: boolean;
  size?: "sm" | "md";
  className?: string;
  id?: string;
  name?: string;
  "aria-invalid"?: boolean;
  "aria-describedby"?: string;
}) {
  const input = useRef<HTMLInputElement>(null);
  const parsed = value.trim() ? Number(value) : NaN;
  const adjust = (direction: number) => {
    if (disabled) return;
    if (step !== "any" && input.current) {
      input.current.stepUp(direction);
      onChange(input.current.value);
      return;
    }
    const next = Number.isFinite(parsed) ? parsed + direction : min ?? 0;
    onChange(String(Math.max(min ?? -Infinity, Math.min(max ?? Infinity, Number(next.toFixed(6))))));
  };
  return <div className={`relative ${className}`}>
    <input {...inputProps} ref={input} type="number" autoComplete="off" aria-label={ariaLabel} min={min} max={max} step={step} required={required}
      value={value} onChange={event => onChange(event.target.value)} disabled={disabled}
      onKeyDown={event => {
        if (event.key === "ArrowUp" || event.key === "ArrowDown") {
          event.preventDefault();
          adjust(event.key === "ArrowUp" ? 1 : -1);
        }
      }}
      className={`number-input recess focus-ring w-full rounded-hw text-ink user-invalid:border-danger disabled:opacity-55 ${size === "sm" ? "h-7 py-0 pl-2 pr-7 text-right font-mono text-[11px]" : "py-1.5 pl-2.5 pr-8 text-[13px]"}`} />
    <div className="absolute inset-y-px right-px grid w-6 grid-rows-2 overflow-hidden rounded-r-hw-sm border-l border-edge">
      {([1, -1] as const).map(direction => <button key={direction} type="button"
        tabIndex={-1}
        aria-label={`${direction === 1 ? "Increase" : "Decrease"} ${ariaLabel.toLowerCase()}`}
        disabled={disabled || (Number.isFinite(parsed) && (direction === 1 ? max !== undefined && parsed >= max : min !== undefined && parsed <= min))}
        onPointerDown={event => { event.preventDefault(); input.current?.focus(); }}
        onClick={() => adjust(direction)}
        className={`focus-ring flex items-center justify-center text-ink-dim transition-colors hover:bg-route/10 hover:text-route disabled:cursor-not-allowed disabled:opacity-35 ${direction === 1 ? "border-b border-edge" : ""}`}>
        <ChevronDown className={`size-3 ${direction === 1 ? "rotate-180" : ""}`} />
      </button>)}
    </div>
  </div>;
}

/** A labelled control. The legend is cut above the well it names. */
export function Field({
  label,
  info,
  hint,
  children,
  className = "",
}: {
  label: string;
  info?: ReactNode;
  hint?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={`flex flex-col gap-1.5 ${className}`}>
      <div className="field-heading flex items-center gap-1.5"><Legend>{label}</Legend>{info}</div>
      {children}
      {hint && <p className="text-[12px] leading-relaxed text-ink-faint">{hint}</p>}
    </div>
  );
}

export function ErrorStrip({ message, onDismiss }: { message: string; onDismiss?: () => void }) {
  useFocusedDismiss(message, onDismiss);
  return (
    <div className="flex items-start gap-3 border border-danger/45 bg-danger/10 px-3 py-2.5">
      <FaultSlug>fault</FaultSlug>
      <p data-selectable className="flex-1 text-[12px] leading-relaxed text-ink">
        {message}
      </p>
      {onDismiss && (
        <button
          type="button"
          onClick={onDismiss}
          className="focus-ring legend shrink-0 text-[11px] text-ink-dim transition-colors hover:text-ink"
        >
          Dismiss
        </button>
      )}
    </div>
  );
}

export function ClearButton({ onClick, disabled }: { onClick: () => void; disabled?: boolean }) {
  return (
    <button type="button" onClick={onClick} disabled={disabled}
      className="focus-ring legend text-[11px] text-ink-dim transition-colors hover:text-ink disabled:opacity-50">
      Clear
    </button>
  );
}

/**
 * A section of the desk.
 *
 * The heading is a rule cut across the panel with its legend in it — not a
 * card wrapping a card. Nested plates were the previous build's whole page
 * structure and are the thing this replaces.
 */
export function Section({
  title,
  aside,
  description,
  id,
  children,
}: {
  title: string;
  aside?: ReactNode;
  description?: ReactNode;
  id?: string;
  children: ReactNode;
}) {
  const sectionId = id || `section-${title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "")}`;
  return (
    <section id={sectionId} className="flex flex-col gap-3 scroll-mt-6">
      <div className="flex items-center gap-3">
        <Legend className="text-ink">{title}</Legend>
        <span className="h-px min-w-4 flex-1 bg-edge" />
        {aside}
      </div>
      {description && (
        <p className="max-w-[70ch] text-[12px] leading-relaxed text-ink-dim">{description}</p>
      )}
      {children}
    </section>
  );
}

/** A named behaviour with its lever, matching the General tab's rows. */
export function ToggleRow({
  title,
  description,
  checked,
  onChange,
  disabled,
}: {
  title: string;
  description: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-start justify-between gap-6 border-b border-edge py-3 last:border-b-0">
      <div className="min-w-0">
        <p className="text-[13px] text-ink">{title}</p>
        <p className="mt-0.5 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">{description}</p>
      </div>
      <Switch label={title} checked={checked} onChange={onChange} disabled={disabled} />
    </div>
  );
}
