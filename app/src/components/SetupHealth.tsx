import { useEffect, useState } from "react";
import { engine, hasEngine } from "../lib/engine";
import type { SettingsTab } from "../lib/settingsOutlines";
import { Button, Section } from "./Controls";
type Check = { label: string; detail: string; ok: boolean; action?: string; tab?: SettingsTab; section?: string };

export function SetupHealth({ directory, onJump, onOpenExtension }: { directory?: string | null; onJump: (tab: SettingsTab, id: string) => void; onOpenExtension?: () => void }) {
  const [checks, setChecks] = useState<Check[]>([]);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setBusy(true);
    const names = ["Download engine", "Browser extension", "Shared sign-ins", "Media tools", "Download destination"] as const;
    const tasks: Promise<Check>[] = [
      engine.ready().then(ready => ({ label: names[0], ok: ready, detail: ready ? "Engine responding" : "Engine unavailable", action: "Check again" })),
      Promise.all([engine.apiStatus(), engine.extensionSourceState()]).then(([api, state]) => ({ label: names[1], ok: api.listening && state.clients.some(c => c.online), detail: !api.listening ? api.detail || "Local API unavailable" : state.clients.some(c => c.online) ? `${state.clients.filter(c => c.online).map(c => c.browserName).join(", ")} connected` : state.clients.length ? "Paired browser offline — open your browser" : "No paired browser recorded", action: "Pair / manage", tab: "Extension", section: "section-browser-integration" })),
      engine.sessions().then(sessions => ({ label: names[2], ok: sessions.some(s => s.signedIn), detail: sessions.length ? `${sessions.length} stored site sessions; ${sessions.filter(s => s.signedIn).length} include sign-in markers. Sites may still expire them.` : "No shared sessions. Needed only for sites requiring sign-in.", action: "Manage sign-ins", tab: "Extension", section: "section-cookie-source" })),
      engine.musicTools().then(tools => ({ label: names[3], ok: !!tools.ffmpeg && tools.ffprobe, detail: `${tools.ffmpeg ? "FFmpeg installed" : "FFmpeg missing"} · ${tools.ffprobe ? "FFprobe installed" : "FFprobe missing"}`, action: "Media tools", tab: "General", section: "section-media-processing-codecs-ffmpeg" })),
      engine.destinationHealth().then(folder => ({ label: names[4], ok: folder.writable, detail: `${folder.path} — ${folder.writable ? "Write access confirmed" : folder.detail || "Cannot write here"}`, action: "Choose folder", tab: "General", section: "section-engine-defaults" })),
    ];
    void Promise.allSettled(tasks).then(results => {
      if (!active) return;
      setChecks(results.map((result, i) => result.status === "fulfilled" ? result.value : { label: names[i] || "Setup", ok: false, detail: hasEngine() ? `Check failed: ${String(result.reason)}` : "Launch the desktop app to check setup.", action: "Check again" }));
      setBusy(false); setCheckedAt(new Date().toLocaleTimeString());
    });
    return () => { active = false; };
  }, [directory, revision]);
  return <Section id="section-setup-health" title="Setup health" description="Check the engine, browser connection, sign-ins, media tools, and write access to your default folder." aside={<Button disabled={busy} onClick={() => setRevision(n => n + 1)}>{busy ? "Checking…" : "Check again"}</Button>}>
    <div aria-busy={busy} className="border-y border-edge">
      {!checks.length && <p role="status" className="py-3 text-[12px] text-ink-dim">Checking setup…</p>}
      {checks.map(check => <div key={check.label} className="flex flex-wrap items-center gap-3 border-b border-edge py-3 last:border-0">
        <span className={`size-2 shrink-0 rounded-full ${check.ok ? "bg-route" : "bg-danger"}`} aria-hidden="true" />
        <div className="min-w-0 flex-1"><p className="text-[13px] text-ink">{check.label} <span className="text-[11px] text-ink-dim">· {check.ok ? "Ready" : "Review"}</span></p><p className="mt-1 break-all text-[12px] text-ink-dim">{check.detail}</p></div>
        <Button tone="ghost" disabled={busy} onClick={() => {
          if (check.label === "Browser extension" && onOpenExtension) onOpenExtension();
          else if (check.tab && check.section) onJump(check.tab, check.section);
          else setRevision(n => n + 1);
        }}>{check.action}</Button>
      </div>)}
    </div>
    <p role="status" className="text-[11px] text-ink-faint">{busy ? "Checking current setup…" : `Last checked ${checkedAt}. Shared sign-ins are optional and do not guarantee that a site still accepts a session.`}</p>
  </Section>;
}
