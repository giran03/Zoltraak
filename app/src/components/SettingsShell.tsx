import { lazy, useEffect, useRef, useState } from "react";
import { SlideGroup } from "./SlideGroup";
import { VisitedPanel } from "./VisitedPanel";
import { Legend } from "./Panel";
import { useAutosaveSettings } from "../lib/useAutosaveSettings";
import { onTabKeyDown } from "../lib/tabKeys";
import { searchSettings, type SettingMatch } from "../lib/settingsSearch";
import { SetupHealth } from "./SetupHealth";
import type { ExtensionUpdates } from "../lib/useExtensionUpdates";
import type { ToastItem } from "./Toast";
const AboutView = lazy(() => import("./AboutView").then(m => ({ default: m.AboutView })));
const AppearanceView = lazy(() => import("./AppearanceView").then(m => ({ default: m.AppearanceView })));
const ExtensionSettings = lazy(() => import("./ExtensionSettings").then(m => ({ default: m.ExtensionSettings })));
const SchedulerView = lazy(() => import("./SchedulerView").then(m => ({ default: m.SchedulerView })));
const SettingsView = lazy(() => import("./SettingsView").then(m => ({ default: m.SettingsView })));
import {
  SETTINGS_TABS,
  type SettingsTab,
  type OutlineItem,
  TAB_OUTLINES,
} from "../lib/settingsOutlines";

export {
  SETTINGS_TABS,
  type SettingsTab,
  type OutlineItem,
  TAB_OUTLINES,
};

/**
 * Settings and everything configuration-shaped, behind one destination.
 */
export function SettingsShell({
  tab: controlledTab,
  onTabChange,
  onOpenExtension,
  extensionUpdates,
  onAddToast,
}: {
  tab?: SettingsTab;
  onTabChange?: (tab: SettingsTab) => void;
  onOpenExtension?: () => void;
  extensionUpdates?: ExtensionUpdates;
  onAddToast?: (toast: ToastItem) => void;
}) {
  const [internalTab, setInternalTab] = useState<SettingsTab>("General");
  const tab = controlledTab ?? internalTab;
  const setTab = onTabChange ?? setInternalTab;
  const settings = useAutosaveSettings();

  const [activeSectionId, setActiveSectionId] = useState<string>("");
  const [availableSectionIds, setAvailableSectionIds] = useState<Set<string>>(new Set());
  const shellRef = useRef<HTMLDivElement>(null);
  const isClickingRef = useRef<string | null>(null);
  const clickTimeoutRef = useRef<number | null>(null);
  const [search, setSearch] = useState("");
  const [jump, setJump] = useState<SettingMatch | null>(null);
  const [searchFeedback, setSearchFeedback] = useState<string | null>(null);
  const matches = searchSettings(search);
  const requestJump = (next: SettingMatch) => { setSearch(""); setSearchFeedback(null); setTab(next.tab); setJump(next); };
  useEffect(() => {
    if (!jump || tab !== jump.tab) return;
    let observer: MutationObserver | undefined;
    let finished = false;
    const reveal = () => {
      if (finished) return;
      const target = document.getElementById(jump.id);
      if (!target) return;
      finished = true;
      observer?.disconnect();
      setActiveSectionId(jump.id);
      target.scrollIntoView({ block: "start", behavior: "instant" });
      const control = jump.control ? target.querySelector<HTMLElement>(`[aria-label="${CSS.escape(jump.control)}"]`) : null;
      const focus = control || target;
      if (!control) target.tabIndex = -1;
      focus.focus({ preventScroll: true });
      shellRef.current?.querySelectorAll(".settings-search-hit").forEach(el => el.classList.remove("settings-search-hit"));
      target.classList.add("settings-search-hit");
      setJump(null);
    };
    observer = new MutationObserver(reveal);
    if (shellRef.current) observer.observe(shellRef.current, { childList: true, subtree: true });
    const frame = requestAnimationFrame(reveal);
    const timeout = window.setTimeout(() => { if (!finished) { setSearchFeedback("This setting is unavailable until its panel finishes loading. Try again or check the engine connection."); setJump(null); } }, 3000);
    return () => { cancelAnimationFrame(frame); clearTimeout(timeout); observer?.disconnect(); };
  }, [jump, tab]);
  useEffect(() => { if (jump) return; const timer = window.setTimeout(() => shellRef.current?.querySelectorAll(".settings-search-hit").forEach(el => el.classList.remove("settings-search-hit")), 2500); return () => clearTimeout(timer); }, [jump]);
  useEffect(() => () => { if (clickTimeoutRef.current) clearTimeout(clickTimeoutRef.current); }, []);

  // Filter sections for the current tab based on what is currently present in the DOM
  const tabSections = TAB_OUTLINES[tab] || [];
  const visibleSections = tabSections.filter(
    (item) => availableSectionIds.size === 0 || availableSectionIds.has(item.id)
  );

  // Discover sections present in the DOM on tab switch / render
  useEffect(() => {
    const checkSections = () => {
      const outline = TAB_OUTLINES[tab] || [];
      const existing = new Set<string>();
      for (const item of outline) {
        if (document.getElementById(item.id)) {
          existing.add(item.id);
        }
      }
      setAvailableSectionIds(existing.size > 0 ? existing : new Set(outline.map((o) => o.id)));
    };

    checkSections();
    const timer = window.setTimeout(checkSections, 80);
    return () => window.clearTimeout(timer);
  }, [tab]);

  // Set default active section when tab switches
  useEffect(() => {
    const outline = TAB_OUTLINES[tab] || [];
    const first = outline[0];
    if (first) {
      setActiveSectionId(first.id);
    }
  }, [tab]);

  // Scroll spy: observe visible section elements to update active outline item dynamically
  useEffect(() => {
    const outline = visibleSections;
    if (outline.length === 0) return;

    const shellEl = shellRef.current;
    const scrollContainer = shellEl?.closest(".overflow-y-auto") || window;

    const handleScroll = () => {
      if (isClickingRef.current) return;

      const containerEl = scrollContainer instanceof HTMLElement ? scrollContainer : document.documentElement;
      // When reached the bottom-most part of the scroll container, highlight the last outline section
      const isAtBottom = containerEl.scrollHeight - containerEl.scrollTop - containerEl.clientHeight < 40;

      if (isAtBottom) {
        const last = outline[outline.length - 1];
        if (last) {
          setActiveSectionId(last.id);
          return;
        }
      }

      const elements = outline
        .map((item) => ({ id: item.id, el: document.getElementById(item.id) }))
        .filter((item): item is { id: string; el: HTMLElement } => item.el !== null);

      if (elements.length === 0) return;

      const containerTop = scrollContainer instanceof HTMLElement ? scrollContainer.getBoundingClientRect().top : 0;
      const topThreshold = containerTop + 140;

      const first = elements[0];
      if (!first) return;

      let currentActiveId = first.id;
      for (const item of elements) {
        const rect = item.el.getBoundingClientRect();
        if (rect.top <= topThreshold) {
          currentActiveId = item.id;
        } else {
          break;
        }
      }

      setActiveSectionId(currentActiveId);
    };

    scrollContainer.addEventListener("scroll", handleScroll, { passive: true });
    handleScroll();

    return () => {
      scrollContainer.removeEventListener("scroll", handleScroll);
    };
  }, [tab, visibleSections]);

  // Highlight a section and pause the scroll spy while the smooth scroll settles
  const lockActiveSection = (sectionId: string) => {
    setActiveSectionId(sectionId);
    isClickingRef.current = sectionId;
    if (clickTimeoutRef.current) window.clearTimeout(clickTimeoutRef.current);
    clickTimeoutRef.current = window.setTimeout(() => {
      isClickingRef.current = null;
    }, 1200);
  };

  const handleScrollToSection = (sectionId: string) => {
    lockActiveSection(sectionId);
    const target = document.getElementById(sectionId);
    if (target) {
      target.scrollIntoView({ behavior: "smooth", block: "start" });
    }
  };

  // Scroll the page's own scroll container to its very top, not just the first section
  const handleScrollToTop = () => {
    const first = visibleSections[0];
    if (first) lockActiveSection(first.id);
    const scrollContainer = shellRef.current?.closest(".overflow-y-auto") || window;
    scrollContainer.scrollTo({ top: 0, behavior: "smooth" });
  };

  return (
    <div ref={shellRef} className="mx-auto w-full max-w-6xl px-4 py-6 sm:px-6">
      {/* Header & Tabs bar */}
      <div className="mb-6 flex flex-col gap-4 border-b border-edge pb-4 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <div className="flex items-center gap-2">
            <Legend className="text-[13px] font-bold uppercase tracking-[0.16em] text-ink">
              Settings Panel
            </Legend>
            <span className="text-[11px] font-mono text-ink-faint">//</span>
            <span className="text-[11px] font-mono font-medium uppercase text-route">{tab}</span>
          </div>
          <p className="mt-0.5 text-[12px] text-ink-dim">
            Configure system parameters, display hardware acceleration, dispatch timetable, and security leases.
          </p>
        </div>

        <SlideGroup
          role="tablist"
          aria-label="Settings sections"
          onKeyDown={onTabKeyDown}
          className="settings-tablist-glass flex w-fit max-w-full overflow-x-auto rounded-hw border border-edge-strong bg-face/50"
        >
          {SETTINGS_TABS.map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={tab === t}
              tabIndex={tab === t ? 0 : -1}
              onClick={() => setTab(t)}
              className={`focus-ring legend shrink-0 border-r border-edge px-3.5 py-1.5 text-[11px] transition-colors last:border-r-0 ${
                tab === t ? "text-on-signal" : "text-ink-dim hover:bg-face hover:text-ink"
              }`}
            >
              {t}
            </button>
          ))}
        </SlideGroup>
      </div>

      {/* Mobile / Tablet Horizontal Outline Strip */}
      <div className="mb-5">
        <label htmlFor="settings-search" className="mb-2 block text-[12px] text-ink-dim">Find a setting</label>
        <input id="settings-search" type="search" autoComplete="off" spellCheck={false} value={search} onChange={e => setSearch(e.target.value)} placeholder="Search delete, sound, folder, extension…" className="recess focus-ring w-full rounded-hw px-3 py-2 text-[13px] text-ink" aria-controls="settings-search-results" onKeyDown={e => { if (e.key === "Escape") setSearch(""); if (e.key === "Enter" && matches[0]) { e.preventDefault(); requestJump(matches[0]); } }} />
        {searchFeedback && <p role="status" className="mt-2 text-[12px] text-danger">{searchFeedback}</p>}
        {search.trim() && <div id="settings-search-results" className="mt-2 border border-edge bg-face p-2">
          <p role="status" className="px-2 py-1 text-[11px] text-ink-dim">{matches.length ? `${matches.length} settings found. Choose a result to jump to it.` : "No settings found. Try another term."}</p>
          {matches.map((match, i) => <button key={`${match.tab}-${match.id}-${i}`} type="button" className="focus-ring flex w-full items-center justify-between gap-4 px-2 py-2 text-left text-[12px] text-ink hover:bg-recess" onClick={() => requestJump(match)}><span>{match.label}</span><span className="text-ink-dim">{match.tab}</span></button>)}
        </div>}
      </div>
      <div className="settings-outline-strip mb-5 overflow-x-auto pb-1 lg:hidden">
        <div className="flex min-w-max items-center gap-1.5">
          <span className="px-1 text-[11px] font-mono uppercase text-ink-faint">Outline:</span>
          {visibleSections.map((item, idx) => {
            const isCurrent = activeSectionId === item.id;
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => handleScrollToSection(item.id)}
                className={`focus-ring flex items-center gap-1.5 rounded-hw border px-2.5 py-1 font-mono text-[11px] transition-colors ${
                  isCurrent
                    ? "border-route bg-route font-semibold text-on-signal"
                    : "border-edge-strong bg-face text-ink-dim hover:text-ink"
                }`}
              >
                <span className="opacity-70">{String(idx + 1).padStart(2, "0")}</span>
                <span>{item.label}</span>
              </button>
            );
          })}
        </div>
      </div>

      {/* Two-column layout: Left Outline Rail + Right Content Area */}
      <div className="flex flex-col gap-8 lg:flex-row lg:items-start">
        {/* Left Side Outline Rail */}
        <aside className="hidden lg:sticky lg:top-4 lg:block lg:w-56 lg:shrink-0 self-start">
          <div className="settings-outline-card rounded-hw border border-edge bg-face/80 p-2.5 shadow-xs backdrop-blur-sm">
            <div className="mb-2 flex items-center justify-between border-b border-edge/60 px-1.5 pb-2">
              <div className="flex items-center gap-2">
                <span className="size-1.5 rounded-full bg-route animate-pulse" />
                <span className="text-[11px] font-mono uppercase tracking-[0.14em] text-ink-dim">
                  Outline // {tab}
                </span>
              </div>
              <span className="text-[11px] font-mono tabular-nums text-ink-faint">
                {visibleSections.length}
              </span>
            </div>

            <SlideGroup as="nav" aria-label={`${tab} section navigation`} className="flex flex-col gap-0.5" indicatorClassName="rounded-hw border-l-2 border-route bg-route/15 shadow-xs">
              {visibleSections.map((item, idx) => {
                const isCurrent = activeSectionId === item.id;
                return (
                  <button
                    key={item.id}
                    type="button"
                    onClick={() => handleScrollToSection(item.id)}
                    aria-current={isCurrent ? "true" : undefined}
                    className={`focus-ring group flex w-full items-center gap-2 rounded-hw px-2 py-1.5 text-left font-mono text-[11px] transition-colors ${
                      isCurrent
                        ? "border-l-2 border-transparent pl-2 font-semibold text-route"
                        : "border-l-2 border-transparent text-ink-dim hover:border-edge-strong hover:bg-face/80 hover:text-ink"
                    }`}
                  >
                    <span
                      className={`text-[11px] tabular-nums transition-colors ${
                        isCurrent ? "font-bold text-route" : "text-ink-faint group-hover:text-ink-dim"
                      }`}
                    >
                      {String(idx + 1).padStart(2, "0")}
                    </span>
                    <span className="truncate">{item.label}</span>
                    {isCurrent && (
                      <span className="ml-auto size-1 shrink-0 rounded-full bg-route" />
                    )}
                  </button>
                );
              })}
            </SlideGroup>

            {/* Quick Scroll To Top shortcut */}
            <div className="mt-3 border-t border-edge/60 px-1 pt-2">
              <button
                type="button"
                onClick={handleScrollToTop}
                className="focus-ring flex w-full items-center justify-between px-1 py-1 text-[11px] font-mono text-ink-faint transition-colors hover:text-ink"
              >
                <span>Scroll To Top</span>
                <span className="text-[11px]">↑</span>
              </button>
            </div>
          </div>
        </aside>

        {/* Right Side Settings Panels */}
        <div className="min-w-0 flex-1 pb-48">
          <VisitedPanel active={tab === "General"}><div className="mb-7"><SetupHealth directory={settings.settings?.defaultDirectory} onJump={(tab, id) => requestJump({ tab, id, label: "Setup" })} onOpenExtension={onOpenExtension} /></div><SettingsView settingsState={settings} /></VisitedPanel>
          <VisitedPanel active={tab === "Appearance"}><AppearanceView /></VisitedPanel>
          <VisitedPanel active={tab === "Timetable"}><SchedulerView settingsState={settings} /></VisitedPanel>
          <VisitedPanel active={tab === "Extension"}><ExtensionSettings updates={extensionUpdates} onAddToast={onAddToast} /></VisitedPanel>
          <VisitedPanel active={tab === "About"}><AboutView onAddToast={onAddToast} /></VisitedPanel>
        </div>
      </div>
    </div>
  );
}
