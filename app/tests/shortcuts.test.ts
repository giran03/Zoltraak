import assert from "node:assert/strict";
import { test } from "node:test";
import { bindingFromEvent, defaultShortcuts, formatBinding, matchesShortcut, readShortcuts, sameBinding, shortcutConflict } from "../src/lib/shortcuts.ts";
const event = (key: string, options = {}) => ({ key, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, defaultPrevented: false, isComposing: false, repeat: false, ...options }) as KeyboardEvent;
test("bindings require exact modifiers and respect toggles, composition and handled events", () => {
  const settings = defaultShortcuts();
  assert.equal(matchesShortcut(settings, "newDownload", event("N")), true);
  assert.equal(matchesShortcut(settings, "newDownload", event("n", { altKey: true })), false);
  assert.equal(matchesShortcut(settings, "search", event("/", { ctrlKey: true })), false);
  assert.equal(matchesShortcut(settings, "search", event("/", { isComposing: true })), false);
  assert.equal(matchesShortcut(settings, "remove", event("Delete", { defaultPrevented: true })), false);
  assert.equal(matchesShortcut(settings, "moveDown", event("ArrowDown", { repeat: true })), true);
  assert.equal(matchesShortcut(settings, "remove", event("Delete", { repeat: true })), false);
  settings.actions.newDownload.enabled = false;
  assert.equal(matchesShortcut(settings, "newDownload", event("n")), false);
  settings.enabled = false;
  assert.equal(matchesShortcut(settings, "remove", event("Delete")), false);
});
test("modifier capture and cross-platform primary modifier conflicts", () => {
  assert.deepEqual(bindingFromEvent(event("k", { ctrlKey: true, altKey: true, shiftKey: true })), { key: "k", modifiers: ["Ctrl", "Alt", "Shift"] });
  assert.equal(bindingFromEvent(event("Control", { ctrlKey: true })), null);
  assert.equal(sameBinding({ key: "n", modifiers: ["Mod"] }, { key: "N", modifiers: ["Ctrl"] }, false), true);
  assert.equal(sameBinding({ key: "n", modifiers: ["Mod"] }, { key: "n", modifiers: ["Meta"] }, true), true);
  assert.equal(shortcutConflict(defaultShortcuts(), "search", { key: "n", modifiers: [] })?.id, "newDownload");
});
test("preferences survive reload with reassigned default keys and tolerate invalid storage", () => {
  const settings = defaultShortcuts();
  settings.actions.newDownload.bindings = [{ key: "x", modifiers: ["Alt", "Shift"] }];
  settings.actions.search.bindings = [{ key: "n", modifiers: [] }];
  settings.actions.remove.enabled = false;
  assert.deepEqual(readShortcuts(JSON.stringify(settings)), settings);
  assert.deepEqual(readShortcuts("garbage"), defaultShortcuts());
  assert.deepEqual(readShortcuts(JSON.stringify({ actions: { search: { bindings: [{ key: "Tab", modifiers: [] }] } } })), defaultShortcuts());
  settings.actions.newDownload.bindings = [{ key: "n", modifiers: [] }];
  assert.deepEqual(readShortcuts(JSON.stringify(settings)), defaultShortcuts(), "colliding storage cannot activate two actions");
});

test("download opening can switch between mouse buttons and keys with exact modifiers and persisted toggles", () => {
  const settings = defaultShortcuts();
  const mouse = (button: number, shiftKey = false) => ({ button, shiftKey, ctrlKey: false, altKey: false, metaKey: false, defaultPrevented: false }) as MouseEvent;
  assert.equal(formatBinding(settings.actions.openDownload.bindings[0]), "Middle click");
  assert.equal(matchesShortcut(settings, "openDownload", mouse(1)), true);
  for (const button of [0, 2, 3, 4]) assert.equal(matchesShortcut(settings, "openDownload", mouse(button)), false);
  assert.equal(bindingFromEvent(mouse(2)), null, "right-click stays reserved for context menus");
  settings.actions.openDownload.bindings = [{ key: "o", modifiers: ["Alt"] }];
  assert.equal(matchesShortcut(settings, "openDownload", mouse(1)), false);
  assert.equal(matchesShortcut(settings, "openDownload", event("o", { altKey: true })), true);
  settings.actions.openDownload.bindings = [bindingFromEvent(mouse(3, true))!];
  assert.equal(matchesShortcut(settings, "openDownload", mouse(3)), false);
  assert.equal(matchesShortcut(settings, "openDownload", mouse(3, true)), true);
  assert.deepEqual(readShortcuts(JSON.stringify(settings)), settings);
  settings.actions.openDownload.enabled = false;
  assert.equal(matchesShortcut(readShortcuts(JSON.stringify(settings)), "openDownload", mouse(3, true)), false);
  assert.equal(matchesShortcut(readShortcuts(JSON.stringify({ actions: {} })), "openDownload", mouse(1)), true, "existing preferences gain the default binding");
});
