import { useEffect, useState } from "react";
import { Section } from "./Controls";
import { ArrowUpRight, BookOpen, Coffee, Shield } from "./Icons";
import { Legend } from "./Panel";
import { engine, openGuide, openLegal, openUrl } from "../lib/engine";
import { ReleaseNotesDialog } from "./ReleaseNotesDialog";
import { currentRelease } from "../lib/releaseNotes";

export function AboutView() {
  const [notesOpen, setNotesOpen] = useState(false);
  const [appVersion, setAppVersion] = useState<string>("0.1.0");
  const [extensionVersion, setExtensionVersion] = useState<string | null>(null);

  useEffect(() => {
    void engine.getAppVersion().then(setAppVersion).catch(() => {});
    void engine.getExtensionVersion().then(setExtensionVersion).catch(() => {});
  }, []);

  return (
    <div className="flex flex-col gap-7">
      {notesOpen && <ReleaseNotesDialog onClose={() => setNotesOpen(false)} />}
      {currentRelease && <button type="button" onClick={() => setNotesOpen(true)} className="focus-ring flex w-fit items-center gap-2 rounded-hw border border-edge px-3 py-2 text-[12px] text-ink hover:bg-recess"><BookOpen className="size-4" />What's new in this version</button>}
      <Section title="Software identity">
        <div className="flex flex-col gap-4 border border-edge bg-face p-5">
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div>
              <div className="flex items-center gap-3">
                <span className="text-[18px] font-bold tracking-[0.18em] text-ink font-mono uppercase">
                  Zoltraak
                </span>
                <span className="rounded-hw border border-edge bg-recess px-2 py-0.5 text-[11px] font-mono tracking-wider text-ink-dim uppercase">
                  v{appVersion}
                </span>
                <span className="rounded-hw border border-clear/40 bg-clear/10 px-2 py-0.5 text-[11px] font-mono tracking-wider text-clear uppercase">
                  Line Active
                </span>
              </div>
              <p className="mt-1.5 max-w-2xl text-[12.5px] leading-relaxed text-ink-dim">
                High-performance segmented download manager, streaming media extractor, and album harvester.
                Engineered with Rust, Tauri v2, and the industrial Dispatch Board design system.
              </p>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3 border-t border-edge pt-4 sm:grid-cols-5">
            <div>
              <Legend className="text-[11px] text-ink-faint">ENGINE RUNTIME</Legend>
              <p className="mt-0.5 font-mono text-[12px] text-ink">Rust / Tokio</p>
            </div>
            <div>
              <Legend className="text-[11px] text-ink-faint">SHELL PROTOCOL</Legend>
              <p className="mt-0.5 font-mono text-[12px] text-ink">Tauri v2 IPC</p>
            </div>
            <div>
              <Legend className="text-[11px] text-ink-faint">LOCAL API</Legend>
              <p className="mt-0.5 font-mono text-[12px] text-ink">127.0.0.1:1450</p>
            </div>
            <div>
              <Legend className="text-[11px] text-ink-faint">TELEMETRY</Legend>
              <p className="mt-0.5 font-mono text-[12px] text-clear">None (0% Remote)</p>
            </div>
            <div>
              <Legend className="text-[11px] text-ink-faint">EXTENSION BUILD</Legend>
              <p className="mt-0.5 font-mono text-[12px] text-ink">
                {extensionVersion ? `v${extensionVersion}` : "checking…"}
              </p>
            </div>
          </div>
        </div>
      </Section>

      <Section title="Developer">
        <div className="border border-edge bg-face p-5">
          <h3 className="text-[16px] font-semibold text-ink">CafeTako Team</h3>
          <p className="mt-2 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
            The developers behind Zoltraak, bringing together its download engine,
            desktop application, and browser integration.
          </p>
          <p className="mt-2 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
            Our focus is reliable file transfers, practical media tools, and clear controls
            that make everyday downloading easier.
          </p>
          <div className="mt-4 border-t border-edge pt-4">
            <div className="flex flex-col items-start justify-between gap-4 sm:flex-row sm:items-center">
              <div className="min-w-0">
                <p className="text-[12px] text-ink-dim">Support email</p>
                <a href="mailto:support-zoltraak@cafetako.com"
                  onClick={event => { event.preventDefault(); void openUrl("mailto:support-zoltraak@cafetako.com"); }}
                  className="focus-ring mt-1 inline-block max-w-full break-words text-[12px] text-route underline underline-offset-4">
                  support-zoltraak@cafetako.com
                </a>
              </div>
              <a
                href="https://buymeacoffee.com/cafetako"
                target="_blank"
                rel="noopener noreferrer"
                aria-label="Support CafeTako on Buy Me a Coffee"
                onClick={event => { event.preventDefault(); void openUrl("https://buymeacoffee.com/cafetako"); }}
                className="focus-ring inline-flex min-h-11 shrink-0 items-center gap-3 rounded-hw border border-brass/40 bg-brass/10 px-4 py-2.5 text-ink transition-colors hover:border-brass/70 hover:bg-brass/20"
              >
                <Coffee className="size-5 shrink-0 text-brass" />
                <span className="flex flex-col gap-0.5">
                  <span className="text-[12px] font-semibold">Support CafeTako</span>
                  <span className="text-[11px] text-ink-dim">Buy Me a Coffee</span>
                </span>
                <ArrowUpRight className="size-3.5 shrink-0 text-ink-dim" />
              </a>
            </div>
            <p className="mt-3 text-[11px] leading-relaxed text-ink-dim">
              © 2026 CafeTako Team. All rights reserved.
            </p>
          </div>
        </div>
      </Section>

      <Section
        title="Legal, privacy & documentation"
        description="Operation manuals, privacy policies, terms of service, and software notices."
      >
        <div className="flex flex-col gap-3">
          {/* Legal & Privacy Card */}
          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border border-edge bg-face p-4">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <Shield className="size-4 text-route" />
                <p className="text-[13px] font-medium text-ink">Privacy Policy & Terms of Service</p>
              </div>
              <p className="mt-1 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
                Comprehensive statement detailing our zero-telemetry guarantee, local-first SQLite data storage,
                encrypted session cookie vault, and acceptable fair-use policies.
              </p>
            </div>
            <button
              type="button"
              onClick={() => void openLegal()}
              className="focus-ring flex shrink-0 items-center gap-2 border border-edge-strong bg-recess px-4 py-2 text-[11px] legend text-ink hover:bg-face transition-colors"
            >
              <Shield className="size-3.5" />
              <span>Read Legal & Privacy</span>
              <ArrowUpRight className="size-3 text-ink-faint" />
            </button>
          </div>

          {/* User Guide Card */}
          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border border-edge bg-face p-4">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <BookOpen className="size-4 text-clear" />
                <p className="text-[13px] font-medium text-ink">User Guide & Operation Manual</p>
              </div>
              <p className="mt-1 max-w-[65ch] text-[12px] leading-relaxed text-ink-dim">
                Detailed handbook covering segmented parallel downloading, video and gallery extraction,
                automatic category routing, browser extension installation, CLI automation, and shortcuts.
              </p>
            </div>
            <button
              type="button"
              onClick={() => void openGuide()}
              className="focus-ring flex shrink-0 items-center gap-2 border border-edge-strong bg-recess px-4 py-2 text-[11px] legend text-ink hover:bg-face transition-colors"
            >
              <BookOpen className="size-3.5" />
              <span>Open User Guide</span>
              <ArrowUpRight className="size-3 text-ink-faint" />
            </button>
          </div>
        </div>
      </Section>

      <Section title="Licensing & attributions">
        <div className="border border-edge bg-face p-5 text-[12px] leading-relaxed text-ink-dim">
          <p className="text-ink font-medium">Software & Third-Party Notices</p>
          <p className="mt-1">
            Zoltraak is developed by CafeTako Team. Third-party components retain their respective
            licenses and attribution requirements.
          </p>
          <p className="mt-2 text-ink-faint">
            All third-party trademarks, logos, and platform names (YouTube, Meta, Instagram, Facebook, X/Twitter, Reddit,
            TikTok) are property of their respective owners and referenced solely for nominative compatibility purposes.
          </p>
        </div>
      </Section>
    </div>
  );
}
