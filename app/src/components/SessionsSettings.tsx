import { useCallback, useEffect, useState } from "react";
import { Section } from "./Controls";
import { Lamp, Legend } from "./Panel";
import { engine, hasEngine, type SessionSummary } from "../lib/engine";

/**
 * The browser sessions the extension has shared with the engine, and the
 * means to forget them.
 *
 * What is shown is deliberately thin: the site, when it was last refreshed,
 * whether it amounts to a sign-in, and the *names* of the cookies held. The
 * values never leave the encrypted vault, not even to this screen — a
 * settings page is not the place a session token should be readable.
 */
export function SessionsSettings() {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!hasEngine()) return;
    try {
      setSessions(await engine.sessions());
    } catch {
      // A failed refresh leaves the previous list on screen.
    }
  }, []);

  useEffect(() => {
    void refresh();
    // The extension refreshes sessions in the background as the user browses;
    // a slow poll keeps the list honest without a push channel of its own.
    const timer = setInterval(() => void refresh(), 10_000);
    return () => clearInterval(timer);
  }, [refresh]);

  async function forget(domain: string | null) {
    if (busy) return;
    setBusy(domain ?? "*");
    setMessage(null);
    try {
      if (domain) {
        await engine.forgetSession(domain);
      } else {
        await engine.forgetAllSessions();
      }
      await refresh();
    } catch (e) {
      setMessage((e as Error).message || "Could not forget the session.");
    } finally {
      setBusy(null);
    }
  }

  return (
    <Section
      title="Browser sessions"
      aside={
        sessions.length > 0 ? (
          <button
            type="button"
            onClick={() => void forget(null)}
            disabled={busy !== null}
            className="focus-ring h-7 rounded-hw border border-edge-strong px-2.5 text-[11px] text-ink-dim transition-colors hover:bg-face hover:text-ink disabled:opacity-45"
          >
            Forget all
          </button>
        ) : undefined
      }
      description="Sign-in sessions the extension shares so downloads behind a login work — including links you paste here. Only the cookies each site's login needs are kept, encrypted on this machine, sent only to that site, and never written into a download's record. Forgetting one here does not sign you out of the browser."
    >
      {sessions.length === 0 ? (
        <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-10 text-center">
          <span className="h-px w-24 bg-edge" />
          <Legend className="text-ink-dim">no sessions stored</Legend>
          <p className="max-w-sm text-[12px] leading-relaxed text-ink-dim">
            Visit a supported site in a browser with the extension installed and the session appears
            here.
          </p>
        </div>
      ) : (
        <div className="border border-edge bg-face">
          {sessions.map((s) => (
            <div
              key={s.domain}
              className="flex items-center justify-between gap-4 border-b border-edge px-4 py-2.5 transition-colors last:border-b-0 hover:bg-panel"
            >
              <div className="flex min-w-0 items-center gap-3">
                <Lamp fill={s.signedIn ? "bg-clear" : null} size={7} />
                <div className="min-w-0">
                  <p className="truncate text-[13px] text-ink">{s.domain}</p>
                  <span className="flex flex-wrap items-baseline gap-x-2">
                    <Legend>{s.signedIn ? "signed in" : "anonymous"}</Legend>
                    {s.hasPoToken && <Legend className="text-caution">player token</Legend>}
                    <Legend>{describeAge(s.updatedAt)}</Legend>
                    {s.cookieNames.length > 0 && (
                      <span title={s.cookieNames.join(", ")}>
                        <Legend className="text-ink-faint">
                          {s.cookieNames.length} cookie{s.cookieNames.length === 1 ? "" : "s"}
                        </Legend>
                      </span>
                    )}
                  </span>
                </div>
              </div>
              <button
                type="button"
                onClick={() => void forget(s.domain)}
                disabled={busy !== null}
                className="focus-ring h-7 shrink-0 rounded-hw border border-edge-strong px-2.5 text-[11px] text-ink-dim transition-colors hover:bg-face hover:text-ink disabled:opacity-45"
              >
                Forget
              </button>
            </div>
          ))}
        </div>
      )}

      {message && (
        <p
          role="status"
          className="border border-route/45 bg-route/10 px-3 py-2.5 text-[12px] leading-relaxed text-ink"
        >
          {message}
        </p>
      )}
    </Section>
  );
}

function describeAge(iso: string): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const minutes = Math.max(0, Math.round((Date.now() - then) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}
