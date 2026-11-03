import { useEffect, useRef } from "react";
import { hasEngine } from "./engine";
import { startPolling } from "./polling";

export function usePolling(refresh: () => Promise<unknown>, delay: () => number, active = true) {
  const delayRef = useRef(delay);
  delayRef.current = delay;
  useEffect(() => {
    if (!active || !hasEngine()) return;
    const poll = startPolling(refresh, () => document.visibilityState === "hidden" ? 5000 : delayRef.current());
    const onVisible = () => {
      if (document.visibilityState === "visible") poll.wake();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      poll.stop();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refresh, active]);
}
