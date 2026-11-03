// Isolated from page CSS. Only extension messages can populate the dialog;
// filenames and URLs are always assigned as text, never interpolated as HTML.
(() => {
  let host, root, dialog, current, categories = [], defaultDir = "", busy = false, lastSelectedCat = "";
  let activeBrowseTarget = "directory", curNavPath = "", curParentPath = null;
  let originalExt = "";
  const send = (type, extra = {}) => chrome.runtime.sendMessage({ type: `zoltraak-intercept-${type}`, ...extra });
  const field = (id) => root.getElementById(id);

  function cleanPath(p) {
    let s = String(p || "").trim();
    if ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("'") && s.endsWith("'"))) {
      s = s.slice(1, -1).trim();
    }
    return s;
  }

  function isAbsolute(p) {
    return /^(?:[a-z]:[\\/]|\\\\[^\\]+\\[^\\]+|\/)/i.test(cleanPath(p));
  }

  function joinPath(dir, name) {
    if (!dir) return name;
    const cleanDir = cleanPath(dir);
    const sep = cleanDir.includes("\\") ? "\\" : "/";
    const trimmed = cleanDir.endsWith(sep) ? cleanDir.slice(0, -1) : cleanDir;
    return `${trimmed}${sep}${name}`;
  }

  function matchCategory(url, filename) {
    const extMatch = (filename.match(/\.([0-9a-z]+)$/i) || [])[1]?.toLowerCase();
    let hostName = "";
    try { hostName = new URL(url).hostname.toLowerCase(); } catch { /* ignore */ }

    const sorted = [...categories].filter((c) => c.enabled).sort((a, b) => (b.priority || 0) - (a.priority || 0));
    for (const cat of sorted) {
      if (cat.kind === "extension" && extMatch) {
        const exts = (cat.pattern || "").split(",").map((e) => e.trim().replace(/^\./, "").toLowerCase());
        if (exts.includes(extMatch)) return cat;
      } else if (cat.kind === "domain" && hostName) {
        const pattern = (cat.pattern || "").trim().toLowerCase();
        if (hostName === pattern || hostName.endsWith(`.${pattern}`)) return cat;
      } else if (cat.kind === "regex") {
        try {
          if (new RegExp(cat.pattern, "i").test(url)) return cat;
        } catch { /* ignore bad regex */ }
      }
    }
    return null;
  }

  function updatePreview() {
    if (!current) return;
    const fname = field("filename").value.trim() || current.filename || "download";
    const isManual = field("manual-override").checked;
    const remember = isManual && field("remember-dir")?.checked;
    const manualVal = cleanPath(field("directory").value);
    const catVal = field("category").value;
    const previewEl = field("dest-preview");
    const badgeEl = field("dest-badge");
    const pathWarnEl = field("dir-warning");
    const modeBadge = field("override-mode-badge");
    const manualNote = field("manual-note");

    if (modeBadge && manualNote) {
      if (remember) {
        modeBadge.textContent = "Remembered";
        modeBadge.className = "tag-remember";
        manualNote.textContent = "This folder will become your new default download folder.";
      } else {
        modeBadge.textContent = "One-time only";
        modeBadge.className = "tag-one-time";
        manualNote.textContent = "One-time override. Zoltraak's default folder will not be changed.";
      }
    }

    if (isManual) {
      if (manualVal) {
        if (!isAbsolute(manualVal)) {
          pathWarnEl.textContent = "Folder must be an absolute path (e.g. C:\\Downloads or /home/...).";
          pathWarnEl.hidden = false;
          pathWarnEl.style.display = "block";
          previewEl.textContent = joinPath(manualVal, fname);
          badgeEl.textContent = "Invalid path";
          badgeEl.className = "dest-badge badge-danger";
        } else {
          pathWarnEl.hidden = true;
          pathWarnEl.style.display = "none";
          previewEl.textContent = joinPath(manualVal, fname);
          badgeEl.textContent = remember ? "Manual · Remember" : "Manual · One-time";
          badgeEl.className = "dest-badge badge-signal";
        }
      } else {
        pathWarnEl.hidden = true;
        pathWarnEl.style.display = "none";
        previewEl.textContent = joinPath(defaultDir || "Choose folder", fname);
        badgeEl.textContent = "Manual (specify folder)";
        badgeEl.className = "dest-badge badge-dim";
      }
      return;
    }

    pathWarnEl.hidden = true;
    pathWarnEl.style.display = "none";
    if (catVal) {
      const selected = categories.find((c) => String(c.id) === catVal);
      const targetDir = selected?.directory || defaultDir || "Category folder";
      previewEl.textContent = joinPath(targetDir, fname);
      badgeEl.textContent = `Category: ${selected?.name || "Selected"}`;
      badgeEl.className = "dest-badge badge-route";
    } else {
      const autoCat = matchCategory(current.url, fname);
      if (autoCat && autoCat.directory) {
        previewEl.textContent = joinPath(autoCat.directory, fname);
        badgeEl.textContent = `Auto: ${autoCat.name}`;
        badgeEl.className = "dest-badge badge-route";
      } else if (defaultDir) {
        previewEl.textContent = joinPath(defaultDir, fname);
        badgeEl.textContent = "Default folder";
        badgeEl.className = "dest-badge badge-dim";
      } else {
        previewEl.textContent = joinPath("Default folder", fname);
        badgeEl.textContent = "Automatic routing";
        badgeEl.className = "dest-badge badge-dim";
      }
    }
  }

  function checkFilename() {
    const val = field("filename").value.trim();
    const hasExt = /\.[0-9a-z]+$/i.test(val);
    const restoreBtn = field("restore-ext");
    const warn = field("ext-warning");

    if (originalExt && !hasExt && val.length > 0) {
      warn.hidden = false;
      warn.style.display = "flex";
      restoreBtn.textContent = `Add ${originalExt}`;
    } else {
      warn.hidden = true;
      warn.style.display = "none";
    }

    const illegal = /[<>:"/\\|?*\x00-\x1f]/.test(val);
    if (illegal) {
      field("error").textContent = 'File name cannot contain < > : " / \\ | ? *';
    } else if (field("error").textContent.includes("File name cannot contain")) {
      field("error").textContent = "";
    }
  }

  function closeStandaloneTab() {
    if (location.href.split(/[?#]/, 1)[0] !== chrome.runtime.getURL("intercept.html")) return;
    void chrome.tabs.getCurrent().then((tab) => {
      if (tab?.id != null) return chrome.tabs.remove(tab.id);
    }).catch(() => {});
  }

  function renderChips() {
    const container = field("chips-list");
    container.replaceChildren();

    const chips = [];
    if (defaultDir) {
      chips.push({ label: "Default folder", path: defaultDir });
    }
    for (const c of categories) {
      if (c.directory && !chips.some((x) => x.path.toLowerCase() === c.directory.toLowerCase())) {
        chips.push({ label: c.name, path: c.directory });
      }
    }

    if (!chips.length) {
      field("quick-chips").hidden = true;
      return;
    }

    field("quick-chips").hidden = false;
    for (const item of chips) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "chip-btn";
      btn.textContent = item.label;
      btn.title = item.path;
      btn.onclick = () => {
        field("directory").value = item.path;
        updatePreview();
      };
      container.appendChild(btn);
    }
  }

  function renderCategoryOptions(selectedId = "") {
    const catSelect = field("category");
    catSelect.replaceChildren(
      new Option("Automatic routing", ""),
      ...categories.filter((c) => c.enabled).map((c) => new Option(c.name, String(c.id))),
      new Option("+ Create new category...", "__new__")
    );
    catSelect.value = selectedId;
  }

  function updateCatKindFields() {
    const kind = field("cat-kind").value;
    const patLabel = field("cat-pattern-label");
    const patInput = field("cat-pattern");
    const patHint = field("cat-pattern-hint");
    if (kind === "extension") {
      patLabel.textContent = "File extensions";
      patInput.placeholder = "zip, rar, 7z";
      patHint.textContent = "Comma-separated extensions (e.g. pdf, doc, docx). No dots needed.";
    } else if (kind === "domain") {
      patLabel.textContent = "Website domain";
      patInput.placeholder = "github.com";
      patHint.textContent = "Matches downloads originating from this host (or its subdomains).";
      if (current?.url && !patInput.value) {
        try {
          patInput.value = new URL(current.url).hostname.toLowerCase();
        } catch {}
      }
    } else if (kind === "regex") {
      patLabel.textContent = "Regular expression";
      patInput.placeholder = "\\.iso(\\?|$)";
      patHint.textContent = "Valid regular expression tested against the full URL.";
    }
  }

  function validateCatDir() {
    const dirVal = cleanPath(field("cat-dir").value);
    const warnEl = field("cat-dir-warn");
    if (dirVal && !isAbsolute(dirVal)) {
      warnEl.textContent = "Folder must be an absolute path (e.g. C:\\Downloads or /home/...).";
      warnEl.hidden = false;
      warnEl.style.display = "block";
    } else {
      warnEl.hidden = true;
      warnEl.style.display = "none";
    }
  }

  function renderCatChips() {
    const container = field("cat-chips-list");
    const wrap = field("cat-quick-chips");
    container.replaceChildren();

    const chips = [];
    if (defaultDir) {
      chips.push({ label: "Default folder", path: defaultDir });
      const curName = field("cat-name").value.trim();
      if (curName) {
        chips.push({ label: `Subfolder: ${curName}`, path: joinPath(defaultDir, curName) });
      }
    }

    if (!chips.length) {
      wrap.hidden = true;
      return;
    }
    wrap.hidden = false;
    for (const chip of chips) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "chip-btn";
      btn.textContent = chip.label;
      btn.title = chip.path;
      btn.onclick = () => {
        field("cat-dir").value = chip.path;
        validateCatDir();
      };
      container.appendChild(btn);
    }
  }

  function openCategoryModal() {
    const catDialog = field("cat-dialog");
    const nameInput = field("cat-name");
    const kindSelect = field("cat-kind");
    const patInput = field("cat-pattern");
    const dirInput = field("cat-dir");
    const errEl = field("cat-error");
    const dirWarn = field("cat-dir-warn");

    errEl.textContent = "";
    dirWarn.hidden = true;
    dirWarn.style.display = "none";

    kindSelect.value = "extension";
    updateCatKindFields();

    let ext = "";
    if (originalExt) {
      ext = originalExt.replace(/^\./, "").toLowerCase();
    } else if (current?.filename) {
      const match = current.filename.match(/\.([0-9a-z]+)$/i);
      if (match) ext = match[1].toLowerCase();
    }

    if (ext) {
      patInput.value = ext;
      nameInput.value = ext.toUpperCase();
      dirInput.value = defaultDir ? joinPath(defaultDir, ext.toUpperCase()) : "";
    } else {
      patInput.value = "";
      nameInput.value = "";
      dirInput.value = defaultDir || "";
    }

    renderCatChips();

    if (!catDialog.open) catDialog.showModal();
    nameInput.focus();
    nameInput.select();
  }

  async function submitCategory(e) {
    e?.preventDefault?.();
    const name = field("cat-name").value.trim();
    const kind = field("cat-kind").value;
    const pattern = field("cat-pattern").value.trim();
    const directory = cleanPath(field("cat-dir").value);
    const errEl = field("cat-error");

    if (!name) {
      errEl.textContent = "Category name is required.";
      field("cat-name").focus();
      return;
    }
    if (!pattern) {
      errEl.textContent = "Match pattern is required.";
      field("cat-pattern").focus();
      return;
    }
    if (!directory) {
      errEl.textContent = "Destination folder is required.";
      field("cat-dir").focus();
      return;
    }
    if (!isAbsolute(directory)) {
      errEl.textContent = "Destination folder must be an absolute path (e.g. C:\\Downloads or /home/...).";
      field("cat-dir").focus();
      return;
    }

    const saveBtn = field("cat-save");
    const cancelBtn = field("cat-cancel");
    saveBtn.disabled = true;
    cancelBtn.disabled = true;
    errEl.textContent = "";

    try {
      const res = await send("create-category", { name, kind, pattern, directory });
      if (!res?.ok || !res.category) {
        throw new Error(res?.error || "Failed to create category in Zoltraak.");
      }
      categories.push(res.category);
      renderCategoryOptions(String(res.category.id));
      lastSelectedCat = String(res.category.id);
      renderChips();
      updatePreview();
      field("cat-dialog").close();
    } catch (err) {
      errEl.textContent = err.message || "Failed to create category.";
    } finally {
      saveBtn.disabled = false;
      cancelBtn.disabled = false;
    }
  }

  async function pickFolder(targetId) {
    const input = field(targetId);
    const btn = targetId === "directory" ? field("browse-dir") : field("cat-browse-dir");
    if (!btn || !input) return;
    const origText = btn.textContent;
    btn.textContent = "Browsing...";
    btn.disabled = true;
    try {
      const res = await send("choose-folder", { start: cleanPath(input.value) || defaultDir });
      if (res?.ok) {
        if (res.path) {
          input.value = res.path;
          if (targetId === "directory") updatePreview();
          if (targetId === "cat-dir") validateCatDir();
          input.focus();
        }
        return;
      }
      // If native dialog is not supported or failed, seamlessly fall back to in-modal folder browser
      await openFolderBrowser(targetId);
    } catch {
      await openFolderBrowser(targetId);
    } finally {
      btn.textContent = origText;
      btn.disabled = false;
    }
  }

  async function openFolderBrowser(targetId) {
    activeBrowseTarget = targetId;
    const startPath = cleanPath(field(targetId)?.value) || defaultDir || "";
    curNavPath = startPath;
    const dialogEl = field("browse-dialog");
    if (!dialogEl.open) dialogEl.showModal();
    await loadDirectory(startPath);
  }

  async function loadDirectory(path) {
    const listEl = field("browse-folder-list");
    const pathEl = field("browse-current-path");
    const upBtn = field("browse-up-btn");
    const errEl = field("browse-error");
    const drivesEl = field("browse-drives-row");

    errEl.textContent = "";
    function showNotice(msg) {
      listEl.replaceChildren();
      const div = document.createElement("div");
      div.className = "empty-notice";
      div.textContent = msg;
      listEl.appendChild(div);
    }
    showNotice("Reading directory...");

    const res = await send("list-folders", { path });
    if (!res?.ok) {
      errEl.textContent = res?.error || "Failed to load directory.";
      showNotice("Could not load folder contents.");
      return;
    }

    curNavPath = res.current;
    curParentPath = res.parent;
    pathEl.textContent = res.current;
    pathEl.title = res.current;
    upBtn.disabled = !res.parent;

    // Drives
    drivesEl.replaceChildren();
    if (res.drives && res.drives.length > 1) {
      for (const d of res.drives) {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "drive-btn";
        btn.textContent = d;
        btn.onclick = () => void loadDirectory(d);
        drivesEl.appendChild(btn);
      }
    }

    // Folders
    listEl.replaceChildren();
    if (!res.folders || !res.folders.length) {
      showNotice("No subfolders found in this directory.");
      return;
    }

    for (const f of res.folders) {
      const itemBtn = document.createElement("button");
      itemBtn.type = "button";
      itemBtn.className = "folder-item";
      const iconSpan = document.createElement("span");
      iconSpan.textContent = "📁";
      const nameSpan = document.createElement("span");
      nameSpan.textContent = f.name;
      itemBtn.append(iconSpan, document.createTextNode(" "), nameSpan);
      itemBtn.title = f.path;
      itemBtn.onclick = () => {
        void loadDirectory(f.path);
      };
      listEl.appendChild(itemBtn);
    }
  }

  function selectCurrentBrowseFolder() {
    if (!curNavPath) return;
    const input = field(activeBrowseTarget);
    if (input) {
      input.value = curNavPath;
      if (activeBrowseTarget === "directory") updatePreview();
      if (activeBrowseTarget === "cat-dir") validateCatDir();
      input.focus();
    }
    field("browse-dialog").close();
  }

  function mount() {
    if (host) return;
    host = document.createElement("zoltraak-interception");
    root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `
      <style>
        :host { all: initial; color-scheme: dark; }
        * { box-sizing: border-box; }
        [hidden], .ext-warning[hidden], .manual-group[hidden], .quick-chips[hidden], .field-warn[hidden], .queue-badge[hidden], .cat-quick-chips[hidden], #cat-dialog[hidden], #browse-dialog[hidden] {
          display: none !important;
        }
        dialog { width: min(580px, calc(100vw - 32px)); max-height: calc(100dvh - 32px);
          overflow: auto; padding: 0; border: 1px solid var(--edge-strong, #4c564e); border-radius: 8px;
          background: var(--panel, #161a17); color: var(--ink, #e9ede7); box-shadow: 0 24px 80px #0008;
          font: 14px/1.5 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
        dialog::backdrop { background: #080d0bb3; backdrop-filter: blur(4px); }
        header { padding: 20px 24px 16px; border-bottom: 1px solid var(--edge, #333b35); }
        .header-top { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
        .brand { color: var(--clear, #35c46f); font-size: 11px; letter-spacing: .16em; font-weight: 700; }
        .queue-badge { font-size: 11px; font-weight: 600; padding: 2px 8px; border-radius: 999px;
          background: var(--recess, #101312); border: 1px solid var(--edge, #333b35); color: var(--ink-dim, #a6aea4); }
        h1 { font-size: 22px; letter-spacing: -.03em; margin: 6px 0 2px; }
        p { color: var(--ink-dim, #a6aea4); margin: 0; }
        form { padding: 18px 24px 22px; }
        .meta-row { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; margin-bottom: 12px; }
        .source { font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; overflow-wrap: anywhere;
          max-height: 48px; overflow: auto; color: var(--ink-dim, #a6aea4); flex: 1; min-width: 0; }
        .size-badge { font-size: 12px; font-weight: 600; color: var(--ink, #e9ede7); white-space: nowrap; }

        label { display: block; margin: 14px 0 5px; font-weight: 600; font-size: 13px; }
        input[type="text"], select { width: 100%; padding: 9px 12px; background: var(--recess, #101312);
          color: var(--ink, #e9ede7); border: 1px solid var(--edge-strong, #4c564e); border-radius: 5px; font: inherit; }
        input[type="text"]:focus, select:focus, button:focus-visible {
          outline: 2px solid var(--clear, #35c46f); outline-offset: 2px; }

        .ext-warning { margin-top: 5px; font-size: 12px; color: var(--caution, #e5a93c); display: flex; align-items: center; gap: 6px; }
        .ext-btn { padding: 2px 8px; background: var(--face, #1e2320); border: 1px solid var(--edge-strong, #4c564e);
          border-radius: 4px; font-size: 11px; cursor: pointer; color: var(--ink, #e9ede7); }
        .ext-btn:hover { border-color: var(--clear, #35c46f); }

        /* Destination Preview Box */
        .dest-card { margin-top: 16px; padding: 10px 14px; background: var(--recess, #101312);
          border: 1px solid var(--edge, #333b35); border-radius: 6px; }
        .dest-top { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 4px; }
        .dest-title { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: .08em; color: var(--ink-dim, #a6aea4); }
        .dest-badge { font-size: 11px; font-weight: 600; padding: 1px 7px; border-radius: 4px; }
        .badge-signal { background: #35c46f22; color: var(--clear, #35c46f); border: 1px solid #35c46f55; }
        .badge-route { background: #38bdf822; color: #38bdf8; border: 1px solid #38bdf855; }
        .badge-dim { background: var(--face, #1e2320); color: var(--ink-dim, #a6aea4); border: 1px solid var(--edge, #333b35); }
        .badge-danger { background: #ff939622; color: var(--danger, #ff9396); border: 1px solid #ff939655; }
        .dest-preview { font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; color: var(--ink, #e9ede7);
          overflow-wrap: anywhere; word-break: break-all; }

        /* Manual Folder Toggle & Group */
        .manual-toggle-row { display: flex; align-items: center; justify-content: space-between; margin-top: 16px;
          padding: 8px 12px; background: var(--face, #1e2320); border: 1px solid var(--edge, #333b35); border-radius: 5px; }
        .checkbox-label { display: flex; align-items: center; gap: 8px; font-size: 13px; font-weight: 600; cursor: pointer; user-select: none; margin: 0; }
        .checkbox-label input[type="checkbox"] { width: 16px; height: 16px; accent-color: var(--clear, #35c46f); cursor: pointer; }
        .tag-one-time { font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: .08em;
          padding: 2px 6px; border-radius: 3px; background: #e5a93c22; color: var(--caution, #e5a93c); border: 1px solid #e5a93c44; }
        .tag-remember { font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: .08em;
          padding: 2px 6px; border-radius: 3px; background: #35c46f22; color: var(--clear, #35c46f); border: 1px solid #35c46f44; }
        .remember-row { margin-top: 10px; padding-top: 8px; border-top: 1px solid var(--edge, #333b35); }
        .sub-checkbox { font-size: 12px; color: var(--ink-dim, #a6aea4); font-weight: 500; }
        .sub-checkbox:hover { color: var(--ink, #e9ede7); }

        .manual-group { margin-top: 10px; padding: 12px; background: var(--face, #1e2320);
          border: 1px solid var(--edge-strong, #4c564e); border-radius: 6px; }
        .input-clear-wrap { display: flex; gap: 6px; align-items: center; }
        .clear-btn { padding: 9px 12px; background: var(--recess, #101312); border: 1px solid var(--edge-strong, #4c564e);
          border-radius: 5px; color: var(--ink-dim, #a6aea4); font-weight: 700; cursor: pointer; }
        .clear-btn:hover { color: var(--ink, #e9ede7); border-color: var(--edge, #333b35); }

        .quick-chips { margin-top: 8px; }
        .chips-label { font-size: 11px; color: var(--ink-dim, #a6aea4); margin-bottom: 4px; display: block; }
        .chips-list { display: flex; flex-wrap: wrap; gap: 5px; }
        .chip-btn { font-size: 11px; padding: 3px 8px; background: var(--recess, #101312);
          border: 1px solid var(--edge, #333b35); border-radius: 4px; color: var(--ink, #e9ede7); cursor: pointer; }
        .chip-btn:hover { border-color: var(--clear, #35c46f); }

        .manual-note { font-size: 11px; color: var(--ink-dim, #a6aea4); margin-top: 8px; }
        .field-warn { font-size: 12px; color: var(--danger, #ff9396); margin-top: 6px; }

        #error { color: var(--danger, #ff9396); font-size: 12px; min-height: 18px; margin-top: 10px; }
        footer { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 14px; align-items: center; }
        button { padding: 9px 14px; background: var(--face, #1e2320); border: 1px solid var(--edge-strong, #4c564e);
          border-radius: 5px; color: var(--ink, #e9ede7); font: inherit; font-size: 13px; font-weight: 500; cursor: pointer; }
        button:disabled { opacity: .5; cursor: wait; }
        #now { background: var(--clear, #35c46f); color: var(--on-signal, #0b0e0d); border-color: var(--clear, #35c46f); font-weight: 700; margin-left: auto; }
        #cancel-all { color: var(--danger, #ff9396); border-color: #ff939644; }
        #cancel-all:hover { background: #ff939618; border-color: var(--danger, #ff9396); }

        @media (max-width: 460px) {
          header { padding: 16px 18px 12px; }
          form { padding: 14px 18px 18px; }
          footer { display: grid; grid-template-columns: 1fr 1fr; }
          #now { grid-column: 1 / -1; margin-left: 0; }
        }

        .field-header-row { display: flex; align-items: center; justify-content: space-between; margin-top: 14px; margin-bottom: 5px; }
        .link-btn { background: none; border: none; padding: 0; color: var(--clear, #35c46f); font-size: 11px; font-weight: 600; cursor: pointer; letter-spacing: .02em; text-decoration: underline; text-underline-offset: 2px; }
        .link-btn:hover { opacity: .85; }

        .browse-btn { padding: 9px 12px; background: var(--face, #1e2320); border: 1px solid var(--edge-strong, #4c564e);
          border-radius: 5px; color: var(--ink, #e9ede7); font-size: 12px; font-weight: 600; cursor: pointer;
          white-space: nowrap; display: inline-flex; align-items: center; gap: 5px; }
        .browse-btn:hover { border-color: var(--clear, #35c46f); background: var(--recess, #101312); }
        .browse-btn:disabled { opacity: .6; cursor: wait; }

        /* Sub-dialogs (Category Creation & Folder Browser) */
        #cat-dialog, #browse-dialog {
          width: min(520px, calc(100vw - 32px)); max-height: calc(100dvh - 32px);
          overflow: auto; padding: 0; border: 1px solid var(--edge-strong, #4c564e); border-radius: 8px;
          background: var(--panel, #161a17); color: var(--ink, #e9ede7); box-shadow: 0 32px 90px #000c;
          font: 14px/1.5 system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
        }
        #cat-dialog::backdrop, #browse-dialog::backdrop { background: #080d0bd9; backdrop-filter: blur(5px); }
        .close-icon-btn { background: transparent; border: none; color: var(--ink-dim, #a6aea4); font-size: 14px; cursor: pointer; padding: 4px 8px; border-radius: 4px; line-height: 1; }
        .close-icon-btn:hover { color: var(--ink, #e9ede7); background: var(--face, #1e2320); }
        .grid-2col { display: grid; grid-template-columns: 140px 1fr; gap: 10px; }
        @media (max-width: 440px) { .grid-2col { grid-template-columns: 1fr; } }
        .field-hint { font-size: 11px; color: var(--ink-dim, #a6aea4); margin-top: 4px; }
        .cat-quick-chips { margin-top: 8px; }
        .sub-footer { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--edge, #333b35); }
        .btn-primary { background: var(--clear, #35c46f); color: var(--on-signal, #0b0e0d); border-color: var(--clear, #35c46f); font-weight: 700; }
        .btn-primary:hover { filter: brightness(1.08); }

        /* In-Modal Folder Browser Elements */
        .browser-bar { display: flex; gap: 6px; align-items: center; margin-bottom: 10px; }
        .nav-btn { padding: 6px 10px; background: var(--recess, #101312); border: 1px solid var(--edge-strong, #4c564e);
          border-radius: 5px; color: var(--ink, #e9ede7); font-size: 12px; font-weight: 600; cursor: pointer; }
        .nav-btn:hover:not(:disabled) { border-color: var(--clear, #35c46f); }
        .nav-btn:disabled { opacity: .4; cursor: not-allowed; }
        .cur-path-display { font: 12px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; background: var(--recess, #101312);
          border: 1px solid var(--edge, #333b35); padding: 6px 10px; border-radius: 5px; color: var(--ink, #e9ede7);
          overflow-wrap: anywhere; word-break: break-all; flex: 1; }
        .drives-row { display: flex; gap: 6px; flex-wrap: wrap; margin-bottom: 10px; }
        .drive-btn { font-size: 11px; font-weight: 700; padding: 3px 8px; background: var(--face, #1e2320);
          border: 1px solid var(--edge, #333b35); border-radius: 4px; color: var(--clear, #35c46f); cursor: pointer; }
        .drive-btn:hover { border-color: var(--clear, #35c46f); }
        .folder-list { max-height: 240px; overflow-y: auto; border: 1px solid var(--edge, #333b35);
          border-radius: 6px; background: var(--recess, #101312); padding: 4px; display: flex; flex-direction: column; gap: 2px; }
        .folder-item { display: flex; align-items: center; gap: 8px; padding: 7px 10px; border-radius: 4px;
          background: transparent; border: 1px solid transparent; color: var(--ink, #e9ede7); font-size: 13px;
          cursor: pointer; text-align: left; width: 100%; font: inherit; }
        .folder-item:hover { background: var(--face, #1e2320); border-color: var(--edge, #333b35); }
        .empty-notice { padding: 18px; text-align: center; font-size: 12px; color: var(--ink-dim, #a6aea4); }
      </style>
      <dialog id="main-dialog" aria-labelledby="title" aria-describedby="description">
        <header>
          <div class="header-top">
            <div class="brand">↓ ZOLTRAAK · DOWNLOAD CAPTURED</div>
            <div class="queue-badge" id="queue-badge" hidden></div>
          </div>
          <h1 id="title">Ready when you are.</h1>
          <p id="description">Choose where to save your download.</p>
        </header>
        <form id="main-form">
          <div class="meta-row">
            <div class="source" id="source" title="Download Source URL"></div>
            <div class="size-badge" id="size"></div>
          </div>

          <label for="filename">File name</label>
          <input id="filename" type="text" required autocomplete="off" spellcheck="false">
          <div class="ext-warning" id="ext-warning" hidden style="display: none;">
            <span>Original file extension was removed.</span>
            <button type="button" class="ext-btn" id="restore-ext"></button>
          </div>

          <div class="field-header-row">
            <label for="category" style="margin: 0;">Category</label>
            <button type="button" class="link-btn" id="open-new-cat-btn">+ Add category</button>
          </div>
          <select id="category">
            <option value="">Automatic routing</option>
            <option value="__new__">+ Create new category...</option>
          </select>

          <!-- Destination Live Preview -->
          <div class="dest-card">
            <div class="dest-top">
              <span class="dest-title">Target Location</span>
              <span class="dest-badge badge-dim" id="dest-badge">Automatic</span>
            </div>
            <div class="dest-preview" id="dest-preview">...</div>
          </div>

          <!-- Optional Manual Directory Toggle (One-Time Only) -->
          <div class="manual-toggle-row">
            <label class="checkbox-label" for="manual-override">
              <input type="checkbox" id="manual-override">
              <span>Specify manual folder for this download</span>
            </label>
            <span class="tag-one-time" id="override-mode-badge">One-time only</span>
          </div>

          <!-- Collapsible Manual Directory Input Group -->
          <div class="manual-group" id="manual-group" hidden>
            <label for="directory" style="margin-top: 0;">Destination folder path</label>
            <div class="input-clear-wrap">
              <input id="directory" type="text" placeholder="e.g. D:\\Projects\\Downloads" autocomplete="off" spellcheck="false">
              <button type="button" class="browse-btn" id="browse-dir" title="Choose folder via file picker">📁 Browse...</button>
              <button type="button" class="clear-btn" id="clear-dir" title="Clear path">✕</button>
            </div>
            <div class="field-warn" id="dir-warning" hidden></div>
            <div class="quick-chips" id="quick-chips" hidden>
              <span class="chips-label">Quick suggestions:</span>
              <div class="chips-list" id="chips-list"></div>
            </div>
            <div class="remember-row">
              <label class="checkbox-label sub-checkbox" for="remember-dir">
                <input type="checkbox" id="remember-dir">
                <span>Remember this folder for future downloads</span>
              </label>
            </div>
            <p class="manual-note" id="manual-note">One-time override. Zoltraak's default folder will not be changed.</p>
          </div>

          <p id="error" role="alert"></p>

          <footer>
            <button id="cancel" type="button">Cancel</button>
            <button id="cancel-all" type="button" hidden>Cancel All</button>
            <button id="later" type="button">Download Later</button>
            <button id="now" type="submit">Download Now</button>
          </footer>
        </form>
      </dialog>

      <dialog id="cat-dialog" aria-labelledby="cat-title" aria-describedby="cat-desc">
        <header>
          <div class="header-top">
            <div class="brand">📁 ZOLTRAAK · NEW CATEGORY</div>
            <button type="button" class="close-icon-btn" id="cat-close-btn" title="Close">✕</button>
          </div>
          <h2 id="cat-title" style="font-size: 20px; margin: 6px 0 2px; letter-spacing: -.02em;">Create category</h2>
          <p id="cat-desc">Route matching downloads automatically into a dedicated folder.</p>
        </header>
        <form id="cat-form">
          <label for="cat-name">Category name</label>
          <input id="cat-name" type="text" placeholder="e.g. Archives, Documents, Media" required autocomplete="off" spellcheck="false">

          <div class="grid-2col">
            <div>
              <label for="cat-kind">Match on</label>
              <select id="cat-kind">
                <option value="extension">File extension</option>
                <option value="domain">Website domain</option>
                <option value="regex">Regular expression</option>
              </select>
            </div>
            <div>
              <label for="cat-pattern" id="cat-pattern-label">File extensions</label>
              <input id="cat-pattern" type="text" placeholder="zip, rar, 7z" required autocomplete="off" spellcheck="false">
            </div>
          </div>
          <div class="field-hint" id="cat-pattern-hint">Comma-separated extensions (e.g. pdf, doc, docx). No dots needed.</div>

          <label for="cat-dir">Destination folder</label>
          <div class="input-clear-wrap">
            <input id="cat-dir" type="text" placeholder="e.g. D:\\Downloads\\Archives" required autocomplete="off" spellcheck="false">
            <button type="button" class="browse-btn" id="cat-browse-dir" title="Choose folder via file picker">📁 Browse...</button>
            <button type="button" class="clear-btn" id="cat-dir-clear" title="Clear path">✕</button>
          </div>
          <div class="field-warn" id="cat-dir-warn" hidden></div>

          <div class="cat-quick-chips" id="cat-quick-chips" hidden>
            <span class="chips-label">Quick suggestions:</span>
            <div class="chips-list" id="cat-chips-list"></div>
          </div>

          <p id="cat-error" class="field-warn" style="min-height: 18px; margin-top: 10px;" role="alert"></p>

          <footer class="sub-footer">
            <button id="cat-cancel" type="button">Cancel</button>
            <button id="cat-save" type="submit" class="btn-primary">Save & Apply</button>
          </footer>
        </form>
      </dialog>

      <dialog id="browse-dialog" aria-labelledby="browse-title" aria-describedby="browse-desc">
        <header>
          <div class="header-top">
            <div class="brand">📂 ZOLTRAAK · FOLDER BROWSER</div>
            <button type="button" class="close-icon-btn" id="browse-close-btn" title="Close">✕</button>
          </div>
          <h2 id="browse-title" style="font-size: 20px; margin: 6px 0 2px; letter-spacing: -.02em;">Select destination folder</h2>
          <p id="browse-desc">Navigate directories or pick a drive location.</p>
        </header>
        <div style="padding: 18px 24px 22px;">
          <div class="drives-row" id="browse-drives-row"></div>
          <div class="browser-bar">
            <button type="button" class="nav-btn" id="browse-up-btn" title="Go up one folder">⬆ Up</button>
            <div class="cur-path-display" id="browse-current-path">...</div>
          </div>
          <div class="folder-list" id="browse-folder-list">
            <div class="empty-notice">Loading folders...</div>
          </div>
          <p id="browse-error" class="field-warn" style="min-height: 18px; margin-top: 10px;" role="alert"></p>
          <footer class="sub-footer">
            <button id="browse-cancel-btn" type="button">Cancel</button>
            <button id="browse-select-btn" type="button" class="btn-primary">Select This Folder</button>
          </footer>
        </div>
      </dialog>`;

    (document.body || document.documentElement).append(host);
    globalThis.ZoltraakTheme?.register(host);
    dialog = field("main-dialog") || root.querySelector("dialog");
    dialog.addEventListener("close", closeStandaloneTab);

    field("filename").oninput = () => {
      checkFilename();
      updatePreview();
    };

    field("restore-ext").onclick = () => {
      if (!originalExt) return;
      const currentVal = field("filename").value.trim();
      if (!currentVal.toLowerCase().endsWith(originalExt.toLowerCase())) {
        field("filename").value = `${currentVal}${originalExt}`;
      }
      checkFilename();
      updatePreview();
      field("filename").focus();
    };

    field("category").onchange = () => {
      if (field("category").value === "__new__") {
        field("category").value = lastSelectedCat;
        openCategoryModal();
        return;
      }
      lastSelectedCat = field("category").value;
      updatePreview();
    };

    field("open-new-cat-btn").onclick = () => {
      openCategoryModal();
    };

    field("cat-kind").onchange = () => updateCatKindFields();
    field("cat-name").oninput = () => renderCatChips();
    field("cat-dir").oninput = () => validateCatDir();
    field("cat-dir-clear").onclick = () => {
      field("cat-dir").value = "";
      validateCatDir();
      field("cat-dir").focus();
    };
    field("cat-cancel").onclick = () => field("cat-dialog").close();
    field("cat-close-btn").onclick = () => field("cat-dialog").close();
    field("cat-dialog").addEventListener("cancel", (e) => {
      e.stopPropagation();
      field("cat-dialog").close();
    });
    field("cat-form").onsubmit = (e) => {
      e.preventDefault();
      void submitCategory(e);
    };

    field("manual-override").onchange = () => {
      const active = field("manual-override").checked;
      field("manual-group").hidden = !active;
      if (active) {
        field("directory").focus();
      }
      updatePreview();
    };

    field("remember-dir").onchange = () => {
      updatePreview();
    };

    field("browse-dir").onclick = () => void pickFolder("directory");
    field("cat-browse-dir").onclick = () => void pickFolder("cat-dir");

    field("browse-up-btn").onclick = () => {
      if (curParentPath) void loadDirectory(curParentPath);
    };
    field("browse-select-btn").onclick = selectCurrentBrowseFolder;
    field("browse-cancel-btn").onclick = () => field("browse-dialog").close();
    field("browse-close-btn").onclick = () => field("browse-dialog").close();
    field("browse-dialog").addEventListener("cancel", (e) => {
      e.stopPropagation();
      field("browse-dialog").close();
    });

    field("directory").oninput = () => {
      updatePreview();
    };

    field("clear-dir").onclick = () => {
      field("directory").value = "";
      updatePreview();
      field("directory").focus();
    };

    field("cancel").onclick = cancel;
    field("cancel-all").onclick = cancelAll;
    dialog.addEventListener("cancel", (e) => { e.preventDefault(); void cancel(); });
    field("main-form").onsubmit = (e) => { e.preventDefault(); void submit(false); };
    field("later").onclick = () => submit(true);

    root.addEventListener("keydown", (e) => {
      if (field("cat-dialog")?.open || field("browse-dialog")?.open) return;
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        void submit(true);
      }
    });
  }

  async function refresh() {
    const result = await send("list");
    if (!result?.ok) return;
    const drafts = result.drafts || [];
    if (!drafts.length) {
      current = null;
      dialog?.close();
      if (!dialog) closeStandaloneTab();
      return;
    }

    mount();
    if (current && drafts.some((d) => d.id === current.id)) {
      const idx = drafts.findIndex((d) => d.id === current.id);
      field("queue-badge").textContent = drafts.length > 1 ? `${idx + 1} of ${drafts.length} pending` : "";
      field("queue-badge").hidden = drafts.length <= 1;
      return;
    }

    current = drafts[0];
    categories = result.categories || [];
    defaultDir = result.defaultDirectory || "";

    const extMatch = current.filename.match(/\.[0-9a-z]+$/i);
    originalExt = extMatch ? extMatch[0] : "";

    field("source").textContent = current.url;
    field("source").title = current.url;
    field("size").textContent = current.size > 0 ? `${(current.size / 1048576).toFixed(1)} MB` : "";

    // Multi-draft navigation
    if (drafts.length > 1) {
      field("queue-badge").textContent = `1 of ${drafts.length} pending`;
      field("queue-badge").hidden = false;
      field("cancel-all").textContent = `Cancel All (${drafts.length})`;
      field("cancel-all").hidden = false;
    } else {
      field("queue-badge").hidden = true;
      field("cancel-all").hidden = true;
    }

    field("filename").value = current.filename;
    field("ext-warning").hidden = true;
    field("ext-warning").style.display = "none";
    checkFilename();

    // Fresh one-time manual override state (never persists across downloads or drafts)
    field("manual-override").checked = false;
    field("manual-group").hidden = true;
    field("remember-dir").checked = false;
    field("directory").value = "";
    field("dir-warning").hidden = true;
    field("error").textContent = "";

    // Categories
    renderCategoryOptions("");
    lastSelectedCat = "";

    renderChips();
    updatePreview();

    if (!dialog.open) dialog.showModal();
    field("filename").focus();
    field("filename").select();
  }

  async function act(type, extra = {}) {
    if (busy) return;
    busy = true;
    root.querySelectorAll("button").forEach((b) => { b.disabled = true; });
    try {
      const result = await send(type, extra);
      if (!result?.ok) throw new Error(result?.error || "Zoltraak did not accept this download. Try again.");
      current = null;
      await refresh();
    } catch (e) {
      field("error").textContent = e.message;
    } finally {
      busy = false;
      root.querySelectorAll("button").forEach((b) => { b.disabled = false; });
    }
  }

  function cancel() {
    if (!current) return;
    return act("cancel", { id: current.id });
  }

  function cancelAll() {
    return act("cancel-all");
  }

  function submit(later) {
    if (!root.querySelector("form").reportValidity() || !current) return;
    const fname = field("filename").value.trim();
    if (!fname) {
      field("error").textContent = "File name is required.";
      field("filename").focus();
      return;
    }

    const isManual = field("manual-override").checked;
    const remember = isManual && field("remember-dir")?.checked;
    let dir = isManual ? cleanPath(field("directory").value) : "";
    if (isManual) {
      if (!dir) {
        field("error").textContent = "Please enter a destination folder path, or uncheck manual folder.";
        field("directory").focus();
        return;
      }
      if (!isAbsolute(dir)) {
        field("error").textContent = "Enter an absolute folder path (e.g. C:\\Downloads or /home/...).";
        field("directory").focus();
        return;
      }
    }

    return act("submit", { id: current.id, filename: fname, directory: dir || undefined, remember, later });
  }

  if (window === window.top) {
    chrome.runtime.onMessage.addListener((message, _sender, respond) => {
      if (message?.type !== "zoltraak-show-interception") return;
      refresh().then(() => respond({ ok: true })).catch(() => respond({ ok: false }));
      return true;
    });
    const recover = () => { void refresh().catch(() => {}); };
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", recover, { once: true });
    else recover();
  }

  // Catch explicit download links before a browser download (and its picker)
  // exists. Replay the original click when capture is off or disconnected.
  const replaying = new WeakSet();
  document.addEventListener("click", (event) => {
    const link = event.composedPath().find((node) => node instanceof HTMLAnchorElement);
    if (!link || replaying.has(link) || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    if (!/^https?:/i.test(link.href)) return;
    if (!link.hasAttribute("download") && !/\.(zip|rar|7z|exe|msi|dmg|iso|tar|gz|deb|rpm)(?:[?#]|$)/i.test(link.href)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    send("link", { url: link.href, filename: link.getAttribute("download") || undefined })
      .catch(() => ({ ok: false })).then((result) => {
        if (result?.ok) return;
        replaying.add(link);
        try { link.click(); } finally { replaying.delete(link); }
      });
  }, true);
})();
