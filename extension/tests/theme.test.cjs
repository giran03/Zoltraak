const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const vm = require('node:vm');

test('theme updates reach registered hosts, reject CSS injection, and reset when disabled', async () => {
  let changed;
  const values = new Map();
  const host = { isConnected: true, style: {
    setProperty: (key, value) => values.set(key, value), removeProperty: key => values.delete(key),
  } };
  const context = vm.createContext({
    chrome: { storage: {
      local: { get: async () => ({ extensionTheme: null }) },
      onChanged: { addListener: fn => { changed = fn; } },
    }, runtime: { getURL: () => 'chrome-extension://test/' } },
    location: { href: 'https://example.test' },
    window: { top: {} },
  });
  const { resolve } = require('node:path');
  vm.runInContext(readFileSync(resolve(__dirname, '../theme.js'), 'utf8'), context);
  await new Promise(resolve => setImmediate(resolve));
  context.ZoltraakTheme.register(host);
  changed({ extensionTheme: { newValue: { colorScheme: 'light', variables: {
    '--panel': '#abcdef', '--face': 'url(https://evil.test)', '--route': '#123456',
  } } } }, 'local');
  assert.equal(values.get('--panel'), '#abcdef');
  assert.equal(values.get('color-scheme'), 'light');
  assert.equal(values.has('--face'), false);
  changed({ extensionTheme: { newValue: null } }, 'local');
  assert.equal(values.size, 0);
});

test('handles extension context invalidation on start without starting interval', async () => {
  let intervalStarted = false;
  const win = {};
  win.top = win;
  const context = vm.createContext({
    chrome: {
      storage: {
        local: { get: async () => ({ extensionTheme: null }) },
        onChanged: { addListener: () => {} },
      },
      runtime: {
        id: 'test-id',
        getURL: () => 'chrome-extension://test/',
        sendMessage: () => {
          throw new Error('Extension context invalidated.');
        },
      },
    },
    location: { href: 'http://localhost:9696/settings/general' },
    window: win,
    document: { visibilityState: 'visible', addEventListener: () => {}, removeEventListener: () => {} },
    setInterval: () => { intervalStarted = true; return 123; },
    clearInterval: () => {},
  });
  const { resolve } = require('node:path');
  vm.runInContext(readFileSync(resolve(__dirname, '../theme.js'), 'utf8'), context);
  assert.equal(intervalStarted, false);
});

test('handles extension context invalidation during polling by clearing interval', async () => {
  let intervalTick;
  let clearedId = null;
  let shouldThrow = false;
  const win = {};
  win.top = win;
  const context = vm.createContext({
    chrome: {
      storage: {
        local: { get: async () => ({ extensionTheme: null }) },
        onChanged: { addListener: () => {} },
      },
      runtime: {
        id: 'test-id',
        getURL: () => 'chrome-extension://test/',
        sendMessage: () => {
          if (shouldThrow) throw new Error('Extension context invalidated.');
          return Promise.resolve({ ok: true });
        },
      },
    },
    location: { href: 'http://localhost:9696/settings/general' },
    window: win,
    document: { visibilityState: 'visible', addEventListener: () => {}, removeEventListener: () => {} },
    setInterval: (fn) => { intervalTick = fn; return 999; },
    clearInterval: (id) => { clearedId = id; },
  });
  const { resolve } = require('node:path');
  vm.runInContext(readFileSync(resolve(__dirname, '../theme.js'), 'utf8'), context);
  assert.equal(typeof intervalTick, 'function');
  assert.equal(clearedId, null);

  // Trigger next tick after extension is reloaded/invalidated
  shouldThrow = true;
  intervalTick();
  assert.equal(clearedId, 999);
});

