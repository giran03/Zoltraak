import changelog from "../../../CHANGELOG.json";
import { version } from "../../package.json";

export interface ReleaseNotes {
  summary: string;
  changes: { brief: string; details: string[] }[];
}

export const releaseVersion = version;
export const currentRelease = (changelog.releases as Record<string, ReleaseNotes>)[version];
export const RELEASE_SEEN_KEY = "zoltraak_release_notes_seen";

export function hasNewRelease(current: string, seen: string | null): boolean {
  const pattern = /^(\d+)\.(\d+)\.(\d+)(?:-([\w.-]+))?$/;
  const next = current.match(pattern);
  const last = seen?.match(pattern);
  if (!next) return false;
  if (!last) return true;
  const previous = last.slice(1, 4).map(Number);
  const parts = next.slice(1, 4).map(Number);
  for (let i = 0; i < 3; i++) {
    if (parts[i] !== previous[i]) return (parts[i] ?? 0) > (previous[i] ?? 0);
  }
  const nextPre = next[4], lastPre = last[4];
  if (nextPre === lastPre) return false;
  if (!nextPre) return true;
  if (!lastPre) return false;
  const a = nextPre.split("."), b = lastPre.split(".");
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const left = a[i], right = b[i];
    if (left === right) continue;
    if (left === undefined) return false;
    if (right === undefined) return true;
    const leftNumeric = /^\d+$/.test(left), rightNumeric = /^\d+$/.test(right);
    if (leftNumeric && rightNumeric) return Number(left) > Number(right);
    if (leftNumeric !== rightNumeric) return !leftNumeric;
    return left > right;
  }
  return false;
}
