/** Mirrors `zoltraak_core::model::DownloadState`. */
export type DownloadState =
  | "waiting"
  | "downloading"
  | "paused"
  | "finished"
  | "failed"
  | "scheduled";

/** View model for a queue row. Shaped by the UI, filled by the engine in M2. */
export type DownloadView = {
  id: string;
  filename: string;
  state: DownloadState;
  downloadedBytes: number;
  /** `null` when the server sent no Content-Length. */
  totalBytes: number | null;
  bytesPerSecond: number;
  etaSeconds: number | null;
  connections: number;
};
