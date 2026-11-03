import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";
import { engine, hasEngine, type ConversionCapabilities, type ConversionPlanRow, type ConversionQueueState, type ConversionSettings, type MediaInfo, type RemuxJob } from "./engine";
import { conversionDirectory, defaultConversionSettings, groupBySettings } from "./conversion";
import { reconcileRows } from "./polling";
import { safeStorage } from "./safeStorage";
import { usePolling } from "./usePolling";

/** What to do when every conversion that was working has finished. */
export type AfterBatch = { notify: boolean; openFolder: boolean; copyPath: boolean };
const AFTER_BATCH_KEY = "zoltraak.convert.afterBatch.v1";
const PARALLELISM_KEY = "zoltraak.convert.parallelism.v1";
const IDLE_REFRESH_MS = 8000;

function loadAfterBatch(): AfterBatch {
  try {
    const value = JSON.parse(safeStorage.getItem(AFTER_BATCH_KEY) ?? "{}");
    return { notify: value.notify !== false, openFolder: value.openFolder === true, copyPath: value.copyPath === true };
  } catch { return { notify: true, openFolder: false, copyPath: false }; }
}

const isActive = (job: RemuxJob) => job.state === "running" || job.state === "probing" || job.state === "queued";

/** Local inputs, immutable queued recipes, and backend-authored conversion plans. */
export function useConvert(active = true, workingDirectory?: string | null, onBatchFinished?: (finished: RemuxJob[]) => void) {
  const [files, setFiles] = useState<MediaInfo[]>([]);
  const [jobs, setJobs] = useState<RemuxJob[]>([]);
  const [shared, setShared] = useState<ConversionSettings>(defaultConversionSettings);
  const [overrides, setOverrides] = useState<Record<string, ConversionSettings>>({});
  const overridesRef = useRef(overrides);
  overridesRef.current = overrides;
  const [capabilities, setCapabilities] = useState<ConversionCapabilities | null>(null);
  const [plans, setPlans] = useState<ConversionPlanRow[]>([]);
  const [planning, setPlanning] = useState(false);
  const [starting, setStarting] = useState(false);
  const basket = useRef<MediaInfo[]>([]);
  const [selection, setSelection] = useState<{ path: string; focus?: boolean } | null>(null);
  const selectFile = useCallback((path: string) => {
    if (basket.current.some((file) => file.path === path)) setSelection({ path, focus: false });
  }, []);
  const [directory, setDirectory] = useState("");
  const defaultDirectory = conversionDirectory(workingDirectory);
  const [replaceSource, setReplaceSource] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [queue, setQueue] = useState<ConversionQueueState>({ paused: false, parallelism: 2, queued: [] });
  const [afterBatch, setAfterBatchState] = useState<AfterBatch>(loadAfterBatch);
  const afterBatchRef = useRef(afterBatch);
  afterBatchRef.current = afterBatch;
  const onBatchFinishedRef = useRef(onBatchFinished);
  onBatchFinishedRef.current = onBatchFinished;
  const watchedJobs = useRef(new Set<string>());

  // Each file uses its own recipe when one was made for it, and the shared one otherwise.
  const selectedPath = files.find((file) => file.path === selection?.path)?.path ?? files[0]?.path ?? null;
  const resolveFor = useCallback((path: string) => overrides[path] ?? shared, [overrides, shared]);
  const settings = selectedPath ? resolveFor(selectedPath) : shared;
  const customized = selectedPath !== null && selectedPath in overrides;
  const setSettings = useCallback((update: SetStateAction<ConversionSettings>) => {
    const apply = (previous: ConversionSettings) => typeof update === "function" ? update(previous) : update;
    if (selectedPath && selectedPath in overridesRef.current) {
      setOverrides((previous) => ({ ...previous, [selectedPath]: apply(previous[selectedPath] ?? shared) }));
    } else {
      setShared(apply);
    }
  }, [selectedPath, shared]);
  const setCustomized = useCallback((on: boolean) => {
    if (!selectedPath) return;
    setOverrides((previous) => {
      const { [selectedPath]: _dropped, ...rest } = previous;
      return on ? { ...rest, [selectedPath]: { ...shared } } : rest;
    });
  }, [selectedPath, shared]);

  const refreshCapabilities = useCallback(async () => {
    if (!hasEngine()) return;
    try { setCapabilities(await engine.mediaCapabilities()); }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); }
  }, []);
  useEffect(() => { if (active && !capabilities) void refreshCapabilities(); }, [active, capabilities, refreshCapabilities]);

  useEffect(() => {
    let live = true;
    setPlans([]);
    if (files.length === 0 || !hasEngine()) { setPlanning(false); return; }
    const groups = groupBySettings(files.map((file) => file.path), resolveFor);
    if (groups.some((group) => Object.values(group.settings).some((value) => typeof value === "number" && !Number.isFinite(value)))) {
      setPlans(files.map((file) => ({path:file.path,plan:null,error:"Enter valid numeric settings and timestamps"})));
      setPlanning(false);
      return;
    }
    setPlanning(true);
    const timer = window.setTimeout(() => {
      Promise.all(groups.map((group) => engine.planConversion(group.paths, group.settings))).then((chunks) => {
        if (!live) return;
        const byPath = new Map(chunks.flat().map((row) => [row.path, row]));
        setPlans(files.flatMap((file) => byPath.get(file.path) ?? []));
      }).catch((e) => { if (live) setError(e instanceof Error ? e.message : String(e)); })
        .finally(() => { if (live) setPlanning(false); });
    }, 250);
    return () => { live = false; window.clearTimeout(timer); };
  }, [files, resolveFor]);

  /** Run the chosen after-batch actions once everything that was working has ended. */
  const settleBatch = useCallback((next: RemuxJob[]) => {
    for (const job of next) if (isActive(job)) watchedJobs.current.add(job.id);
    if (next.some(isActive) || watchedJobs.current.size === 0) return;
    const finished = next.filter((job) => watchedJobs.current.has(job.id) && job.state === "finished");
    watchedJobs.current.clear();
    if (finished.length === 0) return;
    const choices = afterBatchRef.current;
    if (choices.notify) onBatchFinishedRef.current?.(finished);
    const last = finished[finished.length - 1]!;
    if (choices.openFolder) void engine.openRemuxOutput(last.id).catch(() => {});
    if (choices.copyPath) {
      const paths = finished.map((job) => job.output).filter(Boolean).join("\n");
      void navigator.clipboard?.writeText(paths).catch(() => {});
    }
  }, []);

  const refresh = useCallback(async () => {
    if (!hasEngine()) return;
    try {
      const [next, state] = await Promise.all([engine.remuxJobs(), engine.remuxQueueState()]);
      setJobs((previous) => reconcileRows(previous, next));
      setQueue(state);
      settleBatch(next);
    } catch {
      // A missed tick is not worth an error strip.
    }
  }, [settleBatch]);

  // Nothing is polled quickly until this screen is visited *or* something is
  // converting. A slow background check still notices work started elsewhere —
  // a context-menu conversion or a watch rule — so its results can be announced.
  const busy = jobs.some(isActive);
  usePolling(refresh, () => (busy ? 400 : active ? 2000 : IDLE_REFRESH_MS), true);

  useEffect(() => {
    const saved = Number(safeStorage.getItem(PARALLELISM_KEY));
    if (hasEngine() && Number.isInteger(saved) && saved >= 1) void engine.setRemuxParallelism(saved).then(refresh).catch(() => {});
  }, [refresh]);

  const setPaused = useCallback(async (paused: boolean) => {
    try { await engine.setRemuxPaused(paused); } catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    await refresh();
  }, [refresh]);
  const setParallelism = useCallback(async (parallelism: number) => {
    try {
      await engine.setRemuxParallelism(parallelism);
      safeStorage.setItem(PARALLELISM_KEY, String(parallelism));
    } catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    await refresh();
  }, [refresh]);
  const moveJob = useCallback(async (id: string, movement: "top" | "up" | "down") => {
    try { await engine.moveRemuxJob(id, movement); } catch { /* It started between the click and the call. */ }
    await refresh();
  }, [refresh]);
  const setAfterBatch = useCallback((next: AfterBatch) => {
    setAfterBatchState(next);
    safeStorage.setItem(AFTER_BATCH_KEY, JSON.stringify(next));
  }, []);

  /** Merge probed files into the basket, ignoring ones already in it. */
  const stage = useCallback((found: MediaInfo[]) => {
    const seen = new Set(basket.current.map((f) => f.path.toLowerCase()));
    const fresh = found.filter((f) => {const key=f.path.toLowerCase(); if (seen.has(key)) return false; seen.add(key); return true;});
    basket.current = [...basket.current, ...fresh];
    setFiles(basket.current);
    return fresh.length;
  }, []);

  /**
   * Add paths to the basket. Folders are walked by the engine, which also
   * reads each file — so a row can say what conversion would actually cost
   * before anything is started.
   */
  const addPaths = useCallback(async (paths: string[], select = false) => {
    const wanted = paths.map((p) => p.trim()).filter(Boolean);
    if (wanted.length === 0 || !hasEngine()) return 0;
    setError(null);
    setScanning(true);
    try {
      const found = await engine.scanMedia(wanted);
      if (found.length === 0) {
        setError("No supported media files were found. Choose video, audio, images, or animations.");
        return 0;
      }
      const added = stage(found);
      if (select && found[0]) {
        const existing = basket.current.find((file) => file.path.toLowerCase() === found[0]!.path.toLowerCase());
        setSelection({ path: existing?.path ?? found[0].path });
      }
      return added;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return 0;
    } finally {
      setScanning(false);
    }
  }, [stage]);

  /** The native picker. Cancelling returns nothing, which is not an error. */
  const browse = useCallback(async () => {
    if (!hasEngine()) return 0;
    setError(null);
    setScanning(true);
    try {
      return stage(await engine.chooseMediaFiles());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return 0;
    } finally {
      setScanning(false);
    }
  }, [stage]);

  /** The native folder picker for the destination. */
  const browseDirectory = useCallback(async () => {
    if (!hasEngine()) return;
    try {
      const picked = await engine.chooseDirectory(directory.trim() || defaultDirectory || undefined);
      if (picked) setDirectory(picked);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [directory, defaultDirectory]);

  const dropOverrides = useCallback((paths: string[]) => {
    setOverrides((previous) => {
      if (!paths.some((path) => path in previous)) return previous;
      return Object.fromEntries(Object.entries(previous).filter(([path]) => !paths.includes(path)));
    });
  }, []);

  const removeFile = useCallback((path: string) => {
    basket.current = basket.current.filter((f) => f.path !== path);
    setFiles(basket.current);
    dropOverrides([path]);
    setSelection((previous) => previous?.path === path ? null : previous);
  }, [dropOverrides]);

  const clearFiles = useCallback(() => { basket.current = []; setFiles([]); setOverrides({}); setSelection(null); }, []);

  useEffect(() => {
    if (!active || !hasEngine()) return;
    let live = true;
    let unlisten: (() => void) | undefined;
    import("@tauri-apps/api/webviewWindow").then(({ getCurrentWebviewWindow }) =>
      getCurrentWebviewWindow().onDragDropEvent((event) => {
        if (live && event.payload.type === "drop") void addPaths(event.payload.paths);
      }),
    ).then((stop) => { if (live) unlisten = stop; else stop(); }).catch(() => {});
    return () => { live = false; unlisten?.(); };
  }, [active, addPaths]);

  /** Hand the basket to the engine, one batch per distinct recipe. Returns how many jobs were started. */
  const start = useCallback(async () => {
    if (files.length === 0 || starting || planning) return 0;
    const paths = plans.filter((row) => row.plan && !row.error).map((row) => row.path);
    if (paths.length === 0) return 0;
    setStarting(true);
    setError(null);
    let started = 0;
    try {
      for (const group of groupBySettings(paths, resolveFor)) {
        const ids = await engine.startConversions(group.paths, group.settings, directory.trim() || defaultDirectory || undefined, replaceSource);
        started += ids.length;
        basket.current = basket.current.filter((file) => !group.paths.includes(file.path));
        setFiles(basket.current);
        dropOverrides(group.paths);
      }
      await refresh();
      return started;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return started;
    } finally {
      setStarting(false);
    }
  }, [files, directory, defaultDirectory, replaceSource, refresh, starting, planning, plans, resolveFor, dropOverrides]);

  const cancel = useCallback(
    async (id: string) => {
      try {
        await engine.cancelRemuxJob(id);
      } catch {
        // Already finished between the click and the call.
      }
      await refresh();
    },
    [refresh],
  );

  const remove = useCallback(
    async (id: string) => {
      try {
        await engine.removeRemuxJob(id);
      } catch {
        // Already gone.
      }
      setJobs((previous) => previous.filter((job) => job.id !== id));
    },
    [],
  );

  const retry = useCallback(
    async (id: string) => {
      setError(null);
      try {
        await engine.retryRemuxJob(id, directory.trim() || undefined);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
      await refresh();
    },
    [directory, refresh],
  );

  const clearDone = useCallback(async () => {
    try {
      await engine.clearRemuxDone();
    } catch {
      // Nothing to clear is not a failure worth reporting.
    }
    await refresh();
  }, [refresh]);

  return {
    files,
    jobs,
    selection,
    selectedPath,
    selectFile,
    settings,
    setSettings,
    customized,
    customizedPaths: Object.keys(overrides),
    setCustomized,
    capabilities,
    refreshCapabilities,
    plans,
    planning,
    starting,
    directory,
    defaultDirectory,
    setDirectory,
    replaceSource,
    setReplaceSource,
    scanning,
    error,
    clearError: () => setError(null),
    addPaths,
    browse,
    browseDirectory,
    removeFile,
    clearFiles,
    start,
    cancel,
    remove,
    retry,
    clearDone,
    refresh,
    queue,
    setPaused,
    setParallelism,
    moveJob,
    afterBatch,
    setAfterBatch,
  };
}
