import assert from "node:assert/strict";
import { test } from "node:test";
import { applyRowDelta, reconcileRows, startPolling } from "../src/lib/polling.ts";

test("polling coalesces focus changes and stops even while a request is pending", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let calls = 0;
  let finish!: () => void;
  const poll = startPolling(() => {
    calls++;
    return new Promise<void>((resolve) => { finish = resolve; });
  }, () => 400);
  assert.equal(calls, 1);
  poll.wake();
  poll.wake();
  t.mock.timers.tick(5000);
  assert.equal(calls, 1);
  finish();
  await Promise.resolve();
  t.mock.timers.tick(0);
  assert.equal(calls, 2);
  poll.stop();
  finish();
  await Promise.resolve();
  poll.wake();
  t.mock.timers.tick(10000);
  assert.equal(calls, 2);
});

test("polling reads the current delay after each completed request", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let calls = 0;
  let delay = 400;
  const poll = startPolling(async () => { calls++; }, () => delay);
  await Promise.resolve();
  t.mock.timers.tick(399);
  assert.equal(calls, 1);
  delay = 5000;
  t.mock.timers.tick(1);
  await Promise.resolve();
  assert.equal(calls, 2);
  t.mock.timers.tick(4999);
  assert.equal(calls, 2);
  t.mock.timers.tick(1);
  assert.equal(calls, 3);
  poll.stop();
});

test("idle snapshots retain identity; nested edits, reordering and removals remain visible", () => {
  const previous = [
    { id: "a", track: { title: "First" }, bytes: 0 },
    { id: "b", track: { title: "Second" }, bytes: 1 },
  ];
  assert.equal(reconcileRows(previous, structuredClone(previous)), previous);
  const incoming = structuredClone(previous);
  incoming[1]!.track.title = "Updated";
  const changed = reconcileRows(previous, incoming);
  assert.equal(changed[0], previous[0]);
  assert.notEqual(changed[1], previous[1]);
  assert.equal(changed[1]!.track.title, "Updated");
  assert.deepEqual(reconcileRows(previous, [incoming[1]!, incoming[0]!]).map(r => r.id), ["b", "a"]);
  assert.equal(reconcileRows(previous, [incoming[0]!]).length, 1);
  assert.deepEqual(reconcileRows(previous, []), []);
});

test("JSON row comparison preserves key-order changes but detects added keys and array edits", () => {
  const previous = [{ id: "a", nested: { count: 1, label: "x" }, values: [1, null, { done: true }] }];
  assert.equal(reconcileRows(previous, [{ values: [1, null, { done: true }], nested: { label: "x", count: 1 }, id: "a" }]), previous);
  assert.notEqual(reconcileRows(previous, [{ ...previous[0]!, values: [null, 1, { done: true }] }]), previous);
  assert.notEqual(reconcileRows(previous, [{ ...previous[0]!, extra: null }]), previous);
  assert.notEqual(reconcileRows(previous, [{ ...previous[0]!, values: [1, null, {}] }]), previous);
});

type Row = { id: string; bytes: number };

test("applyRowDelta patches only the rows that moved and keeps the rest", () => {
  const a = { id: "a", bytes: 1 }, b = { id: "b", bytes: 2 };
  const next = applyRowDelta<Row>([a, b], { upserts: [{ id: "b", bytes: 5 }], removed: [], count: 2 });
  assert.ok(next);
  assert.equal(next[0], a);
  assert.deepEqual(next[1], { id: "b", bytes: 5 });
});

test("applyRowDelta returns the same array for an unchanged keepalive", () => {
  const rows: Row[] = [{ id: "a", bytes: 1 }];
  assert.equal(applyRowDelta(rows, { upserts: [], removed: [], count: 1 }), rows);
  assert.equal(applyRowDelta(rows, { upserts: [{ id: "a", bytes: 1 }], removed: [], count: 1 }), rows);
});

test("applyRowDelta applies removals, additions and a new order", () => {
  const rows: Row[] = [{ id: "a", bytes: 1 }, { id: "b", bytes: 2 }];
  const next = applyRowDelta(rows, { upserts: [{ id: "c", bytes: 0 }], removed: ["a"], order: ["c", "b"], count: 2 });
  assert.deepEqual(next?.map((row) => row.id), ["c", "b"]);
});

test("applyRowDelta reports drift so the caller can fetch a full snapshot", () => {
  const rows: Row[] = [{ id: "a", bytes: 1 }];
  assert.equal(applyRowDelta(rows, { upserts: [], removed: [], count: 3 }), null);
  assert.equal(applyRowDelta(rows, { upserts: [], removed: [], order: ["a", "missing"], count: 2 }), null);
});
