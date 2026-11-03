import { Suspense, useState, type ReactNode } from "react";

/** Mount on first visit, then retain drafts and scroll position. */
export function VisitedPanel({ active, className = "", children }: {
  active: boolean;
  className?: string;
  children: ReactNode;
}) {
  const [visited, setVisited] = useState(active);
  if (active && !visited) setVisited(true);
  return <div className={active ? className : "hidden"}>{(active || visited) && <Suspense fallback={<p role="status" className="p-4 text-[12px] text-ink-dim">Loading panel…</p>}>{children}</Suspense>}</div>;
}
