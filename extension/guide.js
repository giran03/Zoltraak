
    const htmlEl = document.documentElement;

    // Built-in presets palette mapping
    const PRESET_PALETTES = {
      "classic-dispatch": {
        "--c-panel": "#161a17",
        "--c-panel-face": "#1e2320",
        "--c-panel-recess": "#101312",
        "--c-panel-sunk": "#0b0e0d",
        "--c-edge": "#333b35",
        "--c-edge-strong": "#4c564e",
        "--c-ink": "#e9ede7",
        "--c-ink-dim": "#a6aea4",
        "--c-route": "#56a2f0",
        "--c-clear": "#35c46f",
        "--c-caution": "#e8a317",
        "--c-danger": "#f1595e",
        "--c-brass": "#b09a63",
        "data-theme": "dark"
      },
      "day-enamel": {
        "--c-panel": "#d7dcd2",
        "--c-panel-face": "#e6eae0",
        "--c-panel-recess": "#cdd3c7",
        "--c-panel-sunk": "#bcc3b6",
        "--c-edge": "#a7afa0",
        "--c-edge-strong": "#7b8476",
        "--c-ink": "#161a14",
        "--c-ink-dim": "#3c4239",
        "--c-route": "#1560c4",
        "--c-clear": "#086633",
        "--c-caution": "#7a4c00",
        "--c-danger": "#b01b18",
        "--c-brass": "#7d6d3f",
        "data-theme": "light"
      },
      "amber-crt": {
        "--c-panel": "#0c0b08",
        "--c-panel-face": "#171510",
        "--c-panel-recess": "#080705",
        "--c-panel-sunk": "#050403",
        "--c-edge": "#382e18",
        "--c-edge-strong": "#614e26",
        "--c-ink": "#fef3c7",
        "--c-ink-dim": "#d4a373",
        "--c-route": "#f59e0b",
        "--c-clear": "#4ade80",
        "--c-caution": "#fbbf24",
        "--c-danger": "#f87171",
        "--c-brass": "#d4af37",
        "data-theme": "dark"
      },
      "emerald-radar": {
        "--c-panel": "#07120a",
        "--c-panel-face": "#0d1e12",
        "--c-panel-recess": "#040a06",
        "--c-panel-sunk": "#020603",
        "--c-edge": "#183c24",
        "--c-edge-strong": "#285e39",
        "--c-ink": "#d1fae5",
        "--c-ink-dim": "#6ee7b7",
        "--c-route": "#10b981",
        "--c-clear": "#34d399",
        "--c-caution": "#fbbf24",
        "--c-danger": "#f43f5e",
        "--c-brass": "#86efac",
        "data-theme": "dark"
      },
      "solarized-foundry": {
        "--c-panel": "#14110f",
        "--c-panel-face": "#1c1815",
        "--c-panel-recess": "#0d0b09",
        "--c-panel-sunk": "#080706",
        "--c-edge": "#3a2f26",
        "--c-edge-strong": "#5c4a3b",
        "--c-ink": "#f5eee6",
        "--c-ink-dim": "#c2b19f",
        "--c-route": "#d97706",
        "--c-clear": "#22c55e",
        "--c-caution": "#eab308",
        "--c-danger": "#ef4444",
        "--c-brass": "#e5b85a",
        "data-theme": "dark"
      },
      "nordic-frost": {
        "--c-panel": "#0a0e17",
        "--c-panel-face": "#121929",
        "--c-panel-recess": "#060910",
        "--c-panel-sunk": "#030509",
        "--c-edge": "#1f2d47",
        "--c-edge-strong": "#324870",
        "--c-ink": "#f1f5f9",
        "--c-ink-dim": "#94a3b8",
        "--c-route": "#38bdf8",
        "--c-clear": "#34d399",
        "--c-caution": "#fbbf24",
        "--c-danger": "#fb7185",
        "--c-brass": "#7dd3fc",
        "data-theme": "dark"
      },
      "monochrome-carbon": {
        "--c-panel": "#080808",
        "--c-panel-face": "#131313",
        "--c-panel-recess": "#040404",
        "--c-panel-sunk": "#000000",
        "--c-edge": "#262626",
        "--c-edge-strong": "#404040",
        "--c-ink": "#ffffff",
        "--c-ink-dim": "#a3a3a3",
        "--c-route": "#e5e5e5",
        "--c-clear": "#4ade80",
        "--c-caution": "#facc15",
        "--c-danger": "#f87171",
        "--c-brass": "#d4d4d4",
        "data-theme": "dark"
      },
      "liquid-glass": {
        "--c-panel": "#060911",
        "--c-panel-face": "#0c1322",
        "--c-panel-recess": "#04060c",
        "--c-panel-sunk": "#020306",
        "--c-edge": "#1e2f4d",
        "--c-edge-strong": "#325080",
        "--c-ink": "#f0f6fc",
        "--c-ink-dim": "#9bb2d6",
        "--c-route": "#0ea5e9",
        "--c-clear": "#10b981",
        "--c-caution": "#f59e0b",
        "--c-danger": "#f43f5e",
        "--c-brass": "#38bdf8",
        "data-theme": "dark"
      }
    };

    function applyPresetColors(presetId) {
      const p = PRESET_PALETTES[presetId];
      if (!p) return;
      Object.entries(p).forEach(([key, val]) => {
        if (key === "data-theme") {
          htmlEl.setAttribute("data-theme", val);
        } else {
          htmlEl.style.setProperty(key, val);
        }
      });
      document.querySelectorAll(".preset-card").forEach((c) => {
        c.classList.toggle("active", c.dataset.themeId === presetId);
      });
    }

    // 1. Dynamic Version Synchronization
    async function syncVersion() {
      const badge = document.getElementById("guideVersionBadge");
      try {
        const res = await fetch("http://127.0.0.1:1450/health");
        if (res.ok) {
          const data = await res.json();
          if (data && data.version) {
            badge.textContent = "v" + data.version.replace(/^v/, "");
          }
        }
      } catch {
        // Offline: keep the version embedded by the changelog generator.
      }
    }
    syncVersion();

    // 2. Appearance & Transitions Settings Synchronization
    function syncAppearance() {
      const appTheme = document.getElementById("app-theme");
      if (appTheme) {
        htmlEl.setAttribute("data-theme", appTheme.dataset.colorScheme || "dark");
        return;
      }
      try {
        const raw = localStorage.getItem("zoltraak.appearance.v1");
        if (raw) {
          const settings = JSON.parse(raw);
          if (settings.motion === "subtle" || settings.motion === "reduced" || settings.motion === "full") {
            setMotionMode(settings.motion, false);
          }
          if (['sharp', 'bevel', 'modern', 'rounded'].includes(settings.radius)) htmlEl.setAttribute('data-radius', settings.radius);
          if (['compact', 'default', 'comfortable'].includes(settings.density)) htmlEl.setAttribute('data-density', settings.density);
          if (settings.themeMode === "light" || settings.themeMode === "dark") {
            htmlEl.setAttribute("data-theme", settings.themeMode);
          }
          if (settings.activePresetId && PRESET_PALETTES[settings.activePresetId]) {
            applyPresetColors(settings.activePresetId);
          } else if (settings.customColors) {
            const colors = settings.customColors;
            if (colors.panel) htmlEl.style.setProperty("--c-panel", colors.panel);
            if (colors.panelFace) htmlEl.style.setProperty("--c-panel-face", colors.panelFace);
            if (colors.panelRecess) htmlEl.style.setProperty("--c-panel-recess", colors.panelRecess);
            if (colors.panelSunk) htmlEl.style.setProperty("--c-panel-sunk", colors.panelSunk);
            if (colors.edge) htmlEl.style.setProperty("--c-edge", colors.edge);
            if (colors.edgeStrong) htmlEl.style.setProperty("--c-edge-strong", colors.edgeStrong);
            if (colors.ink) htmlEl.style.setProperty("--c-ink", colors.ink);
            if (colors.inkDim) htmlEl.style.setProperty("--c-ink-dim", colors.inkDim);
            if (colors.route) htmlEl.style.setProperty("--c-route", colors.route);
            if (colors.clear) htmlEl.style.setProperty("--c-clear", colors.clear);
            if (colors.caution) htmlEl.style.setProperty("--c-caution", colors.caution);
            if (colors.danger) htmlEl.style.setProperty("--c-danger", colors.danger);
          }
          return;
        }
      } catch {
        // Fallback below
      }

      const legacyTheme = localStorage.getItem("zoltraak_guide_theme") ||
        (window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
      htmlEl.setAttribute("data-theme", legacyTheme);

      if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
        setMotionMode("reduced", false);
      }
    }

    function setMotionMode(mode, save = true) {
      htmlEl.setAttribute("data-motion", mode);
      document.querySelectorAll(".motion-btn").forEach((btn) => {
        btn.classList.toggle("active", btn.dataset.profile === mode);
      });
      if (save) {
        try {
          const raw = localStorage.getItem("zoltraak.appearance.v1");
          const settings = raw ? JSON.parse(raw) : {};
          settings.motion = mode;
          localStorage.setItem("zoltraak.appearance.v1", JSON.stringify(settings));
        } catch {
          // localStorage disabled
        }
      }
    }

    syncAppearance();

    // Listen for storage changes across tabs/windows
    window.addEventListener("storage", (e) => {
      if (e.key === "zoltraak.appearance.v1" || e.key === "zoltraak_guide_theme") {
        syncAppearance();
      }
    });

    // Interactive Preset Cards Click Handlers
    document.querySelectorAll(".preset-card").forEach((card) => {
      card.addEventListener("click", () => {
        const id = card.dataset.themeId;
        if (id) applyPresetColors(id);
      });
    });

    const resetThemeBtn = document.getElementById("resetThemePreviewBtn");
    if (resetThemeBtn) {
      resetThemeBtn.addEventListener("click", () => {
        // Clear inline properties
        const props = [
          "--c-panel", "--c-panel-face", "--c-panel-recess", "--c-panel-sunk",
          "--c-edge", "--c-edge-strong", "--c-ink", "--c-ink-dim",
          "--c-route", "--c-clear", "--c-caution", "--c-danger", "--c-brass"
        ];
        props.forEach((prop) => htmlEl.style.removeProperty(prop));
        syncAppearance();
      });
    }

    // Interactive Motion Mode Buttons
    document.querySelectorAll(".motion-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        const profile = btn.dataset.profile;
        if (profile) setMotionMode(profile, true);
      });
    });

    // 3. Navigation Links Click & Highlight Pulse
    const navLinks = document.querySelectorAll(".nav-link");
    const sections = document.querySelectorAll(".guide-chapter");

    navLinks.forEach((link) => {
      link.addEventListener("click", (e) => {
        const href = link.getAttribute("href");
        if (!href || !href.startsWith("#")) return;
        const targetId = href.slice(1);
        const targetSection = document.getElementById(targetId);
        if (targetSection) {
          e.preventDefault();
          navLinks.forEach((l) => l.classList.remove("active"));
          link.classList.add("active");

          targetSection.scrollIntoView({
            behavior: htmlEl.getAttribute("data-motion") === "reduced" ? "auto" : "smooth",
            block: "start"
          });

          targetSection.classList.remove("highlight-target");
          void targetSection.offsetWidth; // Trigger DOM reflow to restart animation
          targetSection.classList.add("highlight-target");
          history.pushState(null, "", href);
        }
      });
    });

    // 4. Top Right Themes Button: Redirects to Themes Preset Section with Pulse
    const themesLink = document.getElementById("themesLink");
    if (themesLink) {
      themesLink.addEventListener("click", (e) => {
        e.preventDefault();
        const target = document.getElementById("themes-presets") || document.getElementById("settings-appearance");
        if (target) {
          target.scrollIntoView({
            behavior: htmlEl.getAttribute("data-motion") === "reduced" ? "auto" : "smooth",
            block: "start"
          });
          target.classList.remove("highlight-target");
          void target.offsetWidth;
          target.classList.add("highlight-target");

          navLinks.forEach((l) => {
            const h = l.getAttribute("href");
            l.classList.toggle("active", h === "#settings-appearance");
          });
          history.pushState(null, "", "#themes-presets");
        }
      });
    }

    // Open in Zoltraak App Button (Focus window, unhide from tray, redirect to Settings > Appearance)
    const openAppBtn = document.getElementById("openAppAppearanceBtn");
    if (openAppBtn) {
      openAppBtn.addEventListener("click", async () => {
        const textEl = openAppBtn.querySelector(".btn-open-app-text");
        const originalText = textEl ? textEl.textContent : "Open in Zoltraak (Settings > Appearance)";
        if (textEl) textEl.textContent = "Opening Zoltraak...";

        try {
          const controller = new AbortController();
          const timeoutId = setTimeout(() => controller.abort(), 2500);
          const res = await fetch("http://127.0.0.1:1450/app/open?section=Settings&tab=Appearance", {
            method: "POST",
            signal: controller.signal,
            headers: { "Content-Type": "application/json" }
          });
          clearTimeout(timeoutId);

          if (res.ok) {
            openAppBtn.classList.add("success");
            if (textEl) textEl.textContent = "✓ Zoltraak Focused!";
            setTimeout(() => {
              openAppBtn.classList.remove("success");
              if (textEl) textEl.textContent = originalText;
            }, 2500);
          } else {
            throw new Error("HTTP " + res.status);
          }
        } catch {
          openAppBtn.classList.add("error");
          if (textEl) textEl.textContent = "⚠ Zoltraak not running (port 1450)";
          setTimeout(() => {
            openAppBtn.classList.remove("error");
            if (textEl) textEl.textContent = originalText;
          }, 3000);
        }
      });
    }

    // 5. Code Copy Buttons
    document.querySelectorAll(".copy-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        const code = btn.parentElement.querySelector("code").innerText;
        navigator.clipboard.writeText(code).then(() => {
          const original = btn.textContent;
          btn.textContent = "COPIED!";
          setTimeout(() => (btn.textContent = original), 1800);
        });
      });
    });

    // 6. Realtime Search Filter
    const searchInput = document.getElementById("docSearch");
    searchInput.addEventListener("input", (e) => {
      const query = e.target.value.toLowerCase().trim();
      sections.forEach((chapter) => {
        if (!query) {
          chapter.style.display = "";
          return;
        }
        const text = chapter.textContent.toLowerCase();
        chapter.style.display = text.includes(query) ? "" : "none";
      });
    });

    // Keyboard shortcut for search
    window.addEventListener("keydown", (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        searchInput.focus();
      }
    });

    // 7. ScrollSpy for Active Sidebar Link (with bottom edge detection)
    window.addEventListener("scroll", () => {
      const scrollY = window.pageYOffset || document.documentElement.scrollTop;
      const windowH = window.innerHeight;
      const docH = document.documentElement.scrollHeight;
      const atBottom = (scrollY + windowH) >= (docH - 80);

      let current = "";
      if (atBottom && sections.length > 0) {
        current = sections[sections.length - 1].getAttribute("id");
      } else {
        sections.forEach((section) => {
          const top = section.offsetTop - 110;
          if (scrollY >= top) {
            current = section.getAttribute("id");
          }
        });
      }

      if (current) {
        navLinks.forEach((link) => {
          link.classList.toggle("active", link.getAttribute("href") === "#" + current);
        });
      }
    });
  

// Generated into every guide copy from the same release-note source.
(() => {
  const page = document.getElementById('changelogPage');
  document.getElementById('guideVersionBadge').textContent = 'v' + page.dataset.currentVersion;
  const guide = document.querySelector('.layout-frame');
  const active = new URLSearchParams(location.search).get('view') === 'changelog';
  page.hidden = !active;
  guide.hidden = active;
  document.querySelectorAll('[data-doc-view]').forEach(link => {
    if (link.dataset.docView === (active ? 'changelog' : 'guide')) link.setAttribute('aria-current', 'page');
  });
  if (active) {
    document.title = 'Zoltraak — Changelog';
    document.querySelector('.search-box').hidden = true;
    document.getElementById('themesLink').href = '?view=guide#themes-presets';
    // The existing guide theme handler is only relevant in guide view.
    document.getElementById('themesLink').addEventListener('click', event => { event.stopImmediatePropagation(); location.href = '?view=guide#themes-presets'; }, true);
  }
  const search = document.getElementById('changeSearch');
  const version = document.getElementById('changeVersion');
  const category = document.getElementById('changeCategory');
  const entries = [...page.querySelectorAll('.change-entry')];
  function filter() {
    const query = search.value.trim().toLowerCase();
    entries.forEach(entry => {entry.hidden = Boolean(category.value && entry.dataset.category !== category.value || query && !(entry.closest('.change-release').dataset.version+' '+entry.textContent).toLowerCase().includes(query));});
    page.querySelectorAll('.change-category').forEach(section => { section.hidden = ![...section.querySelectorAll('.change-entry')].some(entry=>!entry.hidden); });
    page.querySelectorAll('.change-release').forEach(release => {
      const markerMatches=release.dataset.superseded && !category.value && (!query || release.textContent.toLowerCase().includes(query));
      release.hidden = Boolean(version.value && release.dataset.version !== version.value || !markerMatches && ![...release.querySelectorAll('.change-entry')].some(entry=>!entry.hidden));
    });
    const count=entries.filter(entry=>!entry.hidden&&!entry.closest('.change-release').hidden).length;
    document.getElementById('changeCount').textContent=count+' matching '+(count===1?'change':'changes');
    document.getElementById('changeEmpty').hidden=[...page.querySelectorAll('.change-release')].some(release=>!release.hidden);
  }
  [search,version,category].forEach(control=>control.addEventListener('input',filter));
  document.getElementById('changeReset').addEventListener('click',()=>{search.value='';version.value='';category.value='';filter();search.focus();});
  document.getElementById('changeDetails').addEventListener('click',event=>{
    const expanded=event.currentTarget.getAttribute('aria-pressed')!=='true';
    entries.forEach(entry=>{entry.open=expanded;});
    event.currentTarget.setAttribute('aria-pressed',String(expanded));
    event.currentTarget.textContent=expanded?'Collapse details':'Expand details';
  });
  filter();
  if(active&&location.hash.startsWith('#release-')) document.getElementById(decodeURIComponent(location.hash.slice(1)))?.scrollIntoView();
  const allowed = ['panel','panel-face','panel-recess','panel-sunk','edge','edge-strong','ink','ink-dim','ink-faint','clear','caution','danger','route','brass','on-signal','lamp-off'];
  async function syncLiveTheme() {
    if (document.hidden || !/^https?:$/.test(location.protocol)) return;
    try {
      const response = await fetch(new URL('/guide/theme', location.href), {signal:AbortSignal.timeout(2000)});
      if (!response.ok) return;
      const theme = await response.json();
      if (!theme.variables || !['light','dark'].includes(theme.colorScheme)) return;
      allowed.forEach(name=>{
        const value=theme.variables['--'+name];
        if (/^#[a-f0-9]{6}$/i.test(value||'')) document.documentElement.style.setProperty('--c-'+name,value);
      });
      document.documentElement.setAttribute('data-theme',theme.colorScheme);
      const geometry = theme.geometry || {};
      if (['sharp','bevel','modern','rounded'].includes(geometry.radius)) document.documentElement.setAttribute('data-radius',geometry.radius);
      if (['compact','default','comfortable'].includes(geometry.density)) document.documentElement.setAttribute('data-density',geometry.density);
    } catch { /* Offline guide retains its current theme. */ }
  }
  void syncLiveTheme();
  setInterval(()=>void syncLiveTheme(),3000);
})();

