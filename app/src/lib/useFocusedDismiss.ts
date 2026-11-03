import { useEffect, useRef } from "react";

/** Give feedback five uninterrupted seconds in the active app window. */
export function useFocusedDismiss(message: string | null | undefined, onDismiss?: () => void) {
  const dismissRef = useRef(onDismiss);
  dismissRef.current = onDismiss;
  const enabled = !!onDismiss;
  useEffect(() => {
    if (!message || !enabled) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const stop = () => { clearTimeout(timer); timer = undefined; };
    const start = () => {
      stop();
      if (document.visibilityState === "visible" && document.hasFocus()) {
        timer = setTimeout(() => dismissRef.current?.(), 5000);
      }
    };
    window.addEventListener("focus", start);
    window.addEventListener("blur", stop);
    document.addEventListener("visibilitychange", start);
    start();
    return () => {
      stop();
      window.removeEventListener("focus", start);
      window.removeEventListener("blur", stop);
      document.removeEventListener("visibilitychange", start);
    };
  }, [message, enabled]);
}
