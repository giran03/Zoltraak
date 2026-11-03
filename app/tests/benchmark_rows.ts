// Run with npm run bench:rows. Synthetic history, not a native drag benchmark.
import assert from "node:assert/strict";
import { reconcileRows } from "../src/lib/polling.ts";

const rows = Array.from({ length: 5000 }, (_, i) => ({
  id: String(i), filename: `file ${i}`, state: "finished", bytes: 1024,
  segments: Array.from({ length: 16 }, (_, id) => ({ id, done: 1024 })),
}));
const incoming = structuredClone(rows);
const original = () => {
  const byId = new Map(rows.map(row => [row.id, row]));
  const next = incoming.map(row => {
    const old = byId.get(row.id);
    return old && JSON.stringify(old) === JSON.stringify(row) ? old : row;
  });
  return rows.length === next.length && next.every((row, i) => row === rows[i]) ? rows : next;
};
const times: Record<string, number[]> = { stringify: [], current: [] };
for (let round = 0; round < 12; round++) {
  for (const [name, run] of Object.entries({ stringify: original, current: () => reconcileRows(rows, incoming) })) {
    const start = performance.now();
    assert.equal(run(), rows);
    if (round >= 2) times[name]!.push(performance.now() - start);
  }
}
const summary = Object.fromEntries(Object.entries(times).map(([name, values]) => {
  values.sort((a, b) => a - b);
  return [name, { medianMs: +values[5]!.toFixed(2), p95Ms: +values[9]!.toFixed(2) }];
}));
console.log(JSON.stringify({ rows: rows.length, segmentsPerRow: 16, ...summary }, null, 2));
