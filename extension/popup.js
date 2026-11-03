import { ensurePaired, health, getToken, reconnect, setToken, verify } from "./api.js";
import { mountPlacementEditor } from "./placement-controller.js";

const lamp = document.getElementById("lamp");
const statusWord = document.getElementById("statusWord");
const tokenInput = document.getElementById("token");
const saveButton = document.getElementById("save");
const pairStatus = document.getElementById("pairStatus");
const captureAllSwitch = document.getElementById("captureAll");
const enableSfxSwitch = document.getElementById("enableSfx");
const showMediaButtonSwitch = document.getElementById("showMediaButton");
const showSessionPillSwitch = document.getElementById("showSessionPill");
const reloadExtButton = document.getElementById("reloadExt");
const openGuideButton = document.getElementById("openGuide");
const orderGuide = document.getElementById("orderGuide");
const pendingDownloads = document.getElementById("pendingDownloads");
const interceptionResult = document.getElementById("interceptionResult");
pendingDownloads.addEventListener("click", () => {
  void chrome.tabs.create({ url: chrome.runtime.getURL("intercept.html") });
});

async function refreshInterception() {
  const stored = await chrome.storage.local.get(null);
  const count = Object.keys(stored).filter((key) => key.startsWith("intercept:")).length;
  pendingDownloads.hidden = count === 0;
  pendingDownloads.textContent = `Open pending downloads (${count})`;
  const diagnostic = stored.lastInterception;
  setResult(interceptionResult, diagnostic ? `${diagnostic.reason}${diagnostic.browserError ? ` (${diagnostic.browserError})` : ""}` : "",
    diagnostic && diagnostic.stage !== "captured" ? "warn" : "");
}
void refreshInterception();
chrome.storage.onChanged?.addListener((changes, area) => {
  if (area === "local" && Object.keys(changes).some((key) => key === "lastInterception" || key.startsWith("intercept:"))) void refreshInterception();
});

// Always reflect the real manifest version in the popup footer
const extVersionEl = document.getElementById("extVersion");
if (extVersionEl) {
  try {
    const manifestVersion =
      (typeof chrome !== "undefined" && chrome.runtime?.getManifest?.()?.version) ||
      (typeof browser !== "undefined" && browser.runtime?.getManifest?.()?.version);
    if (manifestVersion) {
      extVersionEl.textContent = `v${manifestVersion}`;
    }
  } catch {
    // Keep fallback version in HTML
  }
}

const orderBest = document.getElementById("orderBest");
const orderPick = document.getElementById("orderPick");
const orderSend = document.getElementById("orderSend");
const orderSession = document.getElementById("orderSession");
const orderRefresh = document.getElementById("orderRefresh");
const sessionResult = document.getElementById("sessionResult");
const refreshResult = document.getElementById("refreshResult");
const bestResult = document.getElementById("bestResult");
const pickResult = document.getElementById("pickResult");
const sendResult = document.getElementById("sendResult");

/** The lamp switches; it never fades. `null` is an unlit housing. */
function setLamp(aspect) {
  lamp.className = aspect ? `lamp ${aspect}` : "lamp";
}

function setResult(el, text, tone) {
  el.textContent = text ?? "";
  el.className = `order-result describer${tone ? ` ${tone}` : ""}`;
}

/**
 * A lever states its position in words rather than relying on a colour, so it
 * is readable at a glance and to a screen reader without a label lookup.
 */
function setSwitch(el, on) {
  el.setAttribute("aria-checked", on ? "true" : "false");
  el.querySelector(".switch-word").textContent = on ? "on" : "off";
}

function isOn(el) {
  return el.getAttribute("aria-checked") === "true";
}

/**
 * Reconnect, then reload.
 *
 * Restarting the extension is what this control has always done, but on its own
 * it never fixed the thing people press it for: a rejected token survives a
 * restart, because it lives in storage. So the pairing is repaired *first* —
 * `reconnect` retries the stored token, then the one the app baked into this
 * build, then native messaging — and only a pairing that now works earns the
 * reload.
 *
 * The order is forced by `chrome.runtime.reload()`, which tears this document
 * down mid-promise; anything asynchronous has to finish before it is called. A
 * failed reconnect deliberately skips the reload: restarting would blank the
 * explanation the user needs, and the extension's files are unchanged either
 * way.
 */
reloadExtButton.addEventListener("click", async () => {
  reloadExtButton.disabled = true;
  setLamp("caution");
  statusWord.textContent = "reconnecting";
  statusWord.className = "legend status-word";
  pairStatus.textContent = "";

  const result = await reconnect({ forceFresh: true });
  await refresh();
  reloadExtButton.disabled = false;

  if (!result.paired) {
    pairStatus.textContent = result.reason ?? "Could not reconnect.";
    pairStatus.className = "pair-status legend bad";
    return;
  }

  if (chrome.runtime?.reload) {
    await chrome.runtime.sendMessage({ type: "zoltraak-prepare-reload" }).catch(() => {});
    chrome.runtime.reload();
  }
  else window.location.reload();
});

async function refresh() {
  // Self-heal pairing on open if the app is reachable and token is mismatched
  await ensurePaired();
  const [up, token] = await Promise.all([health(), getToken()]);
  tokenInput.value = token;

  // Capture-all only acts while the app is reachable — the background handler
  // leaves downloads to the browser otherwise. The stored setting is left
  // alone, so the switch still reads "on"; dimming it is what says nothing is
  // behind it right now, which the word on its own cannot.
  captureAllSwitch.classList.toggle("inert", !up);
  captureAllSwitch.title = up
    ? ""
    : "The Zoltraak app is not running — downloads are being left to the browser.";

  if (!up) {
    setLamp(null);
    statusWord.textContent = "engine detached";
    statusWord.className = "legend status-word bad";
    pairStatus.textContent = "The Zoltraak app is not running.";
    pairStatus.className = "pair-status legend bad";
    return;
  }

  if (!token) {
    setLamp("caution");
    statusWord.textContent = "not paired";
    statusWord.className = "legend status-word";
    pairStatus.textContent = "Paste the token from the app's Extension card.";
    pairStatus.className = "pair-status legend";
    return;
  }

  let paired = await verify(token);
  if (!paired) {
    // If the token was rejected, automatically attempt to re-pair
    const result = await reconnect({ forceFresh: true });
    if (result.paired) {
      paired = true;
      tokenInput.value = await getToken();
    }
  }

  if (!paired) {
    setLamp("danger");
    statusWord.textContent = "token rejected";
    statusWord.className = "legend status-word bad";
    pairStatus.textContent = "Copy a fresh token from the app.";
    pairStatus.className = "pair-status legend bad";
    return;
  }

  setLamp("clear");
  statusWord.textContent = "line open";
  statusWord.className = "legend status-word good";
  pairStatus.textContent = "Paired.";
  pairStatus.className = "pair-status legend good";
}

saveButton.addEventListener("click", async () => {
  saveButton.disabled = true;
  await setToken(tokenInput.value.trim());
  await refresh();
  saveButton.disabled = false;
});

captureAllSwitch.addEventListener("click", async () => {
  const next = !isOn(captureAllSwitch);
  setSwitch(captureAllSwitch, next);
  await chrome.storage.local.set({ captureAll: next });
});

enableSfxSwitch.addEventListener("click", async () => {
  const next = !isOn(enableSfxSwitch);
  setSwitch(enableSfxSwitch, next);
  await chrome.storage.local.set({ enableSfx: next });
});

// The content script watches these two keys directly, so a flip here reaches
// every open tab at once — no reload, and no message plumbing of our own.
showMediaButtonSwitch.addEventListener("click", async () => {
  const next = !isOn(showMediaButtonSwitch);
  setSwitch(showMediaButtonSwitch, next);
  await chrome.storage.local.set({ showMediaButton: next });
});

showSessionPillSwitch.addEventListener("click", async () => {
  const next = !isOn(showSessionPillSwitch);
  setSwitch(showSessionPillSwitch, next);
  renderPillQuick();
  await chrome.storage.local.set({ showSessionPill: next });
});

// A one-click anchor picker for the pill; the placement page has the rest.
const PillPlacement = globalThis.ZoltraakPillPlacement;
const pillOffHint = document.getElementById("pillOffHint");
const pillQuick = document.getElementById("pillQuick");
const pillQuickGrid = document.getElementById("pillQuickGrid");
const pillQuickButtons = new Map();
let pillPlacement = PillPlacement.sanitize(undefined);

function renderPillQuick() {
  const on = isOn(showSessionPillSwitch);
  pillOffHint.hidden = on;
  pillQuick.hidden = !on;
  for (const [anchor, button] of pillQuickButtons) {
    const active = anchor === pillPlacement.anchor;
    button.classList.toggle("active", active);
    button.setAttribute("aria-checked", String(active));
  }
}

for (const anchor of ["top-left", "top-center", "top-right", "middle-left", null, "middle-right", "bottom-left", "bottom-center", "bottom-right"]) {
  if (!anchor) {
    pillQuickGrid.append(document.createElement("span"));
    continue;
  }
  const button = document.createElement("button");
  button.type = "button";
  button.className = "pill-quick-cell";
  button.setAttribute("role", "radio");
  button.title = anchor.replace("-", " ");
  button.setAttribute("aria-label", `Pill ${button.title}`);
  button.addEventListener("click", async () => {
    pillPlacement = PillPlacement.sanitize({ ...pillPlacement, anchor, offsetX: 0, offsetY: 0 });
    renderPillQuick();
    await chrome.storage.local.set({ pillPlacement });
  });
  pillQuickButtons.set(anchor, button);
  pillQuickGrid.append(button);
}

document.getElementById("openPillPlacement")?.addEventListener("click", () => {
  void chrome.tabs.create({ url: chrome.runtime.getURL("placement.html#pill") });
});

chrome.storage.onChanged?.addListener((changes, area) => {
  if (area !== "local" || !(changes.pillPlacement || changes.pillCorner)) return;
  chrome.storage.local.get({ pillPlacement: null, pillCorner: null }).then((stored) => {
    pillPlacement = PillPlacement.sanitize(stored.pillPlacement, stored.pillCorner);
    renderPillQuick();
  });
});

/** The tab the orders act on. */
async function currentTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  return tab?.url ? tab : null;
}

const mediaList = document.getElementById("mediaList");
const mediaStatus = document.getElementById("mediaStatus");
const refreshMediaButton = document.getElementById("refreshMedia");
let mediaBusy = false;
const mediaStatusText = {
  "source-found": "Source found", unresolved: "Source unresolved — resume playback and refresh",
  protected: "Protected media", live: "Live stream — open in the app to record",
  unsupported: "No downloadable source", unassigned: "Unassigned source — ownership is uncertain",
};
function mediaSourceLabel(source) {
  try { const u = new URL(source.url); return `${source.kind.toUpperCase()} · ${u.hostname}${u.pathname}`; }
  catch { return source.kind.toUpperCase(); }
}
async function refreshDetectedMedia() {
  if (!mediaList || mediaBusy) return;
  mediaBusy = true;
  refreshMediaButton.disabled = true;
  try {
    const tab = await currentTab();
    if (!tab || !/^https?:\/\//i.test(tab.url)) { mediaStatus.textContent = "Media detection is unavailable on this page."; mediaList.replaceChildren(); return; }
    const result = await chrome.runtime.sendMessage({ type: "zoltraak-media-list", tabId: tab.id });
    if (!result?.ok) { mediaStatus.textContent = result?.error || "Could not read this page. Reload the page and retry."; return; }
    if (result.accessible === false) {
      mediaStatus.textContent = "Media detection is unavailable here. Reload this page after updating the extension, or check its site access.";
      mediaList.replaceChildren(); return;
    }
    const expanded = new Set([...mediaList.querySelectorAll("details[open]")].map((d) => d.dataset.key));
    const fragment = document.createDocumentFragment();
    const players = [...result.players].sort((a, b) => (a.state !== "playing") - (b.state !== "playing"));
    for (const player of players) appendMediaPlayer(fragment, player, tab.id, expanded);
    if (result.unassigned.length) appendMediaPlayer(fragment, { type: "source", title: "Other detected streams", state: "", status: "unassigned", sources: result.unassigned }, tab.id, expanded);
    mediaList.replaceChildren(fragment);
    const playing = players.filter((p) => p.state === "playing").length;
    mediaStatus.textContent = players.length ? `${players.length} player${players.length === 1 ? "" : "s"} · ${playing} playing${result.unassigned.length ? ` · ${result.unassigned.length} unassigned sources` : ""}`
      : result.unassigned.length ? "Network sources found; no accessible player." : "No media detected yet. Play media, then refresh.";
  } catch (error) { mediaStatus.textContent = error?.message || "Could not read media. Reload the page and retry."; }
  finally { mediaBusy = false; refreshMediaButton.disabled = false; }
}
function appendMediaPlayer(container, player, tabId, expanded) {
  const details = document.createElement("details");
  details.className = "media-player";
  details.dataset.key = `${player.frameId ?? "network"}:${player.id ?? "sources"}`;
  details.open = expanded.has(details.dataset.key) || (player.state === "playing" && player.sources.length === 1);
  const summary = document.createElement("summary"), title = document.createElement("strong");
  const playerNumber = player.id?.replace("media-", "") || "";
  title.textContent = `${player.type === "audio" ? "Audio" : player.type === "video" ? "Video" : "Streams"}${playerNumber ? ` ${playerNumber}` : ""} · ${player.title}${player.state ? ` · ${player.state}` : ""}`;
  summary.append(title); details.append(summary);
  const note = document.createElement("p"); note.className = "order-note";
  note.textContent = mediaStatusText[player.status] || "Source unresolved"; details.append(note);
  if (player.status !== "protected" && player.status !== "unsupported") for (const source of player.sources) {
    const row = document.createElement("div"), label = document.createElement("span"), actions = document.createElement("div");
    row.className = "media-source"; label.className = "describer"; label.textContent = mediaSourceLabel(source);
    actions.className = "media-source-actions"; row.append(label, actions);
    for (const action of player.live ? ["prefill"] : ["download", "prefill"]) {
      const button = document.createElement("button"); button.type = "button"; button.className = "media-action";
      button.textContent = action === "download" ? "Download source" : "Open in app";
      button.addEventListener("click", async () => {
        mediaBusy = true;
        const buttons = [...mediaList.querySelectorAll("button")]; buttons.forEach((b) => { b.disabled = true; });
        mediaStatus.textContent = action === "download" ? "Analyzing selected source…" : "Opening selected source…";
        try {
          const result = await chrome.runtime.sendMessage({ type: `zoltraak-media-${action}`, tabId,
            id: player.id, frameId: player.frameId ?? source.frameId, generation: player.generation,
            documentStartedAt: player.documentStartedAt, url: source.url });
          mediaStatus.textContent = result?.ok ? action === "download" ? `Queued ${result.label || "media"}${result.complete === false && player.type !== "audio" ? " · no audio track" : ""}.` : "Selected source opened in the app."
            : result?.error || "Could not resolve this source. Resume playback and retry.";
        } catch (error) { mediaStatus.textContent = error.message || "The extension could not reach the app."; }
        finally { mediaBusy = false; buttons.forEach((b) => { b.disabled = false; }); }
      });
      actions.append(button);
    }
    details.append(row);
  }
  container.append(details);
}
refreshMediaButton?.addEventListener("click", refreshDetectedMedia);
void refreshDetectedMedia();

/**
 * One click, one complete file — the same resolver the floating button uses
 * (background.js's downloadBestFrom). Every order runs in the worker because a
 * request from this document would hit the API's origin check, the same as a
 * page.
 */
orderBest.addEventListener("click", async () => {
  const tab = await currentTab();
  if (!tab) return setResult(bestResult, "No page to download from.", "bad");

  orderBest.disabled = true;
  setResult(bestResult, "Finding the video…");

  const sources = await chrome.tabs.sendMessage(tab.id, { type: "zoltraak-video-sources" }, { frameId: 0 }).catch(() => null);

  const res = await chrome.runtime.sendMessage({
    type: "zoltraak-download-best",
    candidates: [tab.url],
    ...sources,
    tabId: tab.id,
    referrer: tab.url,
    title: tab.title || undefined,
  });
  orderBest.disabled = false;

  if (!res?.ok) {
    return setResult(bestResult, res?.error ?? "No downloadable video found.", "bad");
  }
  // A source with no audio track is still offered — it may be all there is —
  // but never unlabelled.
  if (res.complete === false) {
    return setResult(bestResult, `Downloading ${res.label} — no audio track`, "warn");
  }
  if (res.audioStatus === "unknown") return setResult(bestResult, `Downloading ${res.label || "best quality"} · audio unverified`, "warn");
  setResult(bestResult, `Downloading ${res.label || "best quality"} · video + audio`, "good");
});

orderPick.addEventListener("click", async () => {
  const tab = await currentTab();
  if (!tab) return setResult(pickResult, "No page to analyze.", "bad");

  orderPick.disabled = true;
  setResult(pickResult, "Handing to the app…");

  const res = await chrome.runtime.sendMessage({
    type: "zoltraak-prefill-video",
    url: tab.url,
    referrer: tab.url,
    title: tab.title || undefined,
    autoDownload: false,
  });
  orderPick.disabled = false;

  setResult(
    pickResult,
    res?.ok ? "Open the app — the Video view has it." : (res?.error ?? "The app did not accept it."),
    res?.ok ? "good" : "bad",
  );
});

orderSend.addEventListener("click", async () => {
  const tab = await currentTab();
  if (!tab) return setResult(sendResult, "No page to send.", "bad");

  orderSend.disabled = true;
  setResult(sendResult, "Handing to the app…");

  const res = await chrome.runtime.sendMessage({
    type: "zoltraak-prefill-download",
    url: tab.url,
    referrer: tab.url,
  });
  orderSend.disabled = false;

  setResult(
    sendResult,
    res?.ok ? "Open the app — the entry is filled." : (res?.error ?? "The app did not accept it."),
    res?.ok ? "good" : "bad",
  );
});

orderSession.addEventListener("click", async () => {
  const tab = await currentTab();
  if (!tab) return setResult(sessionResult, "No page to read a session from.", "bad");
  let host = "";
  try {
    host = new URL(tab.url).hostname;
  } catch {
    return setResult(sessionResult, "This page has no site to share a session for.", "bad");
  }

  orderSession.disabled = true;
  setResult(sessionResult, "Sharing…");

  const res = await chrome.runtime.sendMessage({ type: "zoltraak-sync-session", host });
  orderSession.disabled = false;

  setResult(
    sessionResult,
    res?.ok ? `Shared the ${res.domain} session.` : (res?.error ?? "The app did not accept it."),
    res?.ok ? "good" : "bad",
  );
});

/** "a", "a and b", "a, b and c" — a list a person would read aloud. */
function readable(names) {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

// Unlike the order above, this one needs no page: it re-reads the browser's
// cookies for every supported site, so it works from a blank tab.
orderRefresh.addEventListener("click", async () => {
  orderRefresh.disabled = true;
  setResult(refreshResult, "Re-reading this browser's cookies…");

  const res = await chrome.runtime.sendMessage({ type: "zoltraak-refresh-sessions" });
  orderRefresh.disabled = false;

  if (!res?.ok) {
    return setResult(refreshResult, res?.error ?? "The app did not accept the refresh.", "bad");
  }

  const shared = res.shared ?? [];
  const cleared = res.cleared ?? [];
  if (shared.length === 0) {
    return setResult(
      refreshResult,
      "No signed-in sites in this browser — the app's stored sign-ins were cleared.",
      "warn",
    );
  }
  const tail = cleared.length
    ? ` · cleared ${cleared.length} signed-out ${cleared.length === 1 ? "site" : "sites"}`
    : "";
  setResult(refreshResult, `Refreshed ${readable(shared)}${tail}`, "good");
});

void (async () => {
  const { captureAll, enableSfx, showMediaButton, showSessionPill, pillPlacement: savedPill, pillCorner } =
    await chrome.storage.local.get({
      captureAll: true,
      enableSfx: true,
      showMediaButton: true,
      showSessionPill: true,
      pillPlacement: null,
      pillCorner: null,
    });
  pillPlacement = PillPlacement.sanitize(savedPill, pillCorner);
  setSwitch(captureAllSwitch, Boolean(captureAll));
  setSwitch(enableSfxSwitch, enableSfx !== false);
  setSwitch(showMediaButtonSwitch, showMediaButton !== false);
  setSwitch(showSessionPillSwitch, showSessionPill !== false);
  renderPillQuick();
  await refresh();
})();

function openUserGuide() {
  const guideUrl = "http://127.0.0.1:1450/guide";
  if (typeof chrome !== "undefined" && chrome.tabs && chrome.tabs.create) {
    chrome.tabs.create({ url: guideUrl });
  } else {
    window.open(guideUrl, "_blank");
  }
}

openGuideButton?.addEventListener("click", openUserGuide);
orderGuide?.addEventListener("click", openUserGuide);

const openLegalButton = document.getElementById("openLegal");
function openLegalDoc() {
  const legalUrl = "http://127.0.0.1:1450/legal";
  if (typeof chrome !== "undefined" && chrome.tabs && chrome.tabs.create) {
    chrome.tabs.create({ url: legalUrl });
  } else {
    window.open(legalUrl, "_blank");
  }
}
openLegalButton?.addEventListener("click", openLegalDoc);



// Placement Modal Setup
const placementModal = document.getElementById("placementModal");
const openPlacementModalBtn = document.getElementById("openPlacementModal");
const closePlacementModalBtn = document.getElementById("closePlacementModal");
const popoutPlacementBtn = document.getElementById("popoutPlacement");

if (placementModal) {
  mountPlacementEditor(placementModal);

  openPlacementModalBtn?.addEventListener("click", () => {
    placementModal.removeAttribute("hidden");
  });

  closePlacementModalBtn?.addEventListener("click", () => {
    placementModal.setAttribute("hidden", "");
  });

  placementModal.querySelector(".modal-backdrop")?.addEventListener("click", () => {
    placementModal.setAttribute("hidden", "");
  });

  popoutPlacementBtn?.addEventListener("click", () => {
    const url = chrome.runtime.getURL("placement.html");
    if (chrome?.tabs?.create) {
      chrome.tabs.create({ url });
    } else {
      window.open(url, "_blank");
    }
    placementModal.setAttribute("hidden", "");
  });
}
