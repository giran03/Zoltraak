// Shared by extension pages and content scripts. Only registered extension
// hosts receive variables; the website's own colors are never changed.
(() => {
  const names = ["panel", "face", "recess", "panel-sunk", "edge", "edge-strong",
    "ink", "ink-dim", "ink-faint", "route", "clear", "caution", "danger", "paused",
    "on-signal", "lamp-off", "brass", "block-clear"].map((name) => `--${name}`);
  const hosts = new Set();
  let theme = null;
  // The app's Corner Radius and Density settings, as the scale the stylesheets read.
  const RADII = {
    sharp: ["0px", "0px", "0px", "0px"],
    bevel: ["1px", "2px", "3px", "4px"],
    modern: ["3px", "6px", "8px", "10px"],
    rounded: ["5px", "10px", "12px", "14px"],
  };
  const DENSITIES = { compact: "0.8", default: "1", comfortable: "1.2" };
  const radiusNames = ["--hw-r-sm", "--hw-r", "--hw-r-lg", "--hw-r-xl"];
  function applyGeometry(host) {
    const radii = RADII[theme?.geometry?.radius];
    radiusNames.forEach((name, index) => {
      if (radii) host.style.setProperty(name, radii[index]);
      else host.style.removeProperty(name);
    });
    const density = DENSITIES[theme?.geometry?.density];
    if (density) host.style.setProperty("--density", density);
    else host.style.removeProperty("--density");
  }
  function apply(host) {
    applyGeometry(host);
    for (const name of names) {
      const value = theme?.variables?.[name];
      if (typeof value === "string" && /^#[\da-f]{6}$/i.test(value)) host.style.setProperty(name, value);
      else host.style.removeProperty(name);
    }
    if (theme?.colorScheme === "light" || theme?.colorScheme === "dark") {
      host.style.setProperty("color-scheme", theme.colorScheme);
    } else host.style.removeProperty("color-scheme");
  }
  globalThis.ZoltraakTheme = { register(host) { hosts.add(host); apply(host); } };
  function update(next) {
    theme = next;
    for (const host of hosts) {
      if (!host.isConnected) hosts.delete(host);
      else apply(host);
    }
  }
  try {
    if (chrome?.storage?.local) {
      chrome.storage.local.get("extensionTheme")
        .then(({ extensionTheme }) => update(extensionTheme))
        .catch(() => {});
    }
  } catch {}

  try {
    if (chrome?.storage?.onChanged) {
      chrome.storage.onChanged.addListener((changes, area) => {
        try {
          if (area === "local" && changes?.extensionTheme) update(changes.extensionTheme.newValue);
        } catch {}
      });
    }
  } catch {}

  try {
    if (chrome?.runtime?.getURL && typeof location !== "undefined" && location.href.startsWith(chrome.runtime.getURL(""))) {
      globalThis.ZoltraakTheme.register(document.documentElement);
    }
  } catch {}

  // Top frames and visible popups drive a single deduplicated background fetch.
  if (typeof window !== "undefined" && window === window.top) {
    let timer = null;
    let invalidated = false;
    const cleanup = () => {
      invalidated = true;
      if (timer) {
        clearInterval(timer);
        timer = null;
      }
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", onVisibilityChange);
      }
    };
    const refresh = () => {
      if (invalidated) return;
      try {
        // If extension context was invalidated (e.g. extension reloaded/updated), stop polling
        if (typeof chrome === "undefined" || !chrome.runtime || ("id" in chrome.runtime && !chrome.runtime.id)) {
          cleanup();
          return;
        }
        if (typeof document !== "undefined" && document.visibilityState === "hidden") {
          return;
        }
        if (typeof chrome.runtime.sendMessage === "function") {
          const res = chrome.runtime.sendMessage({ type: "zoltraak-theme" });
          if (res && typeof res.catch === "function") {
            res.catch((err) => {
              if (err?.message?.includes("context invalidated") || !chrome?.runtime?.id) {
                cleanup();
              }
            });
          }
        }
      } catch {
        // Synchronous throw (e.g. Extension context invalidated)
        cleanup();
      }
    };
    const onVisibilityChange = () => refresh();
    refresh();
    if (!invalidated) {
      timer = setInterval(refresh, 3000);
    }
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", onVisibilityChange);
    }
  }
})();
