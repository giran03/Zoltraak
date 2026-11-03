// Run against Vite or preview: NODE_PATH=<Playwright packages> node tests/navigation.cjs
const { chromium } = require('playwright');
const assert = require('node:assert/strict');

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    for (const size of [0, 100, 1000]) {
      const page = await browser.newPage();
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      await page.addInitScript(({ size }) => {
        const callbacks = new Map();
        const listeners = new Map();
        let callbackId = 0;
        window.calls = {};
        window.settings = { bandwidthLimitBytesPerSec: null, defaultConnections: null, defaultDirectory: null, schedule: { enabled: false, startMinutes: 1320, stopMinutes: 360 }, onComplete: { type: 'none' } };
        window.emitTestEvent = (event, payload) => {
          for (const [id, entry] of listeners) if (entry.event === event) callbacks.get(entry.handler)?.({ event, id, payload });
        };
        window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_, id) => listeners.delete(id) };
        window.__TAURI_INTERNALS__ = {
          transformCallback: cb => { callbacks.set(++callbackId, cb); return callbackId; },
          unregisterCallback: id => callbacks.delete(id),
          invoke: async (command, args) => {
            window.calls[command] = (window.calls[command] || 0) + 1;
            if (command === 'plugin:event|listen') { const id = ++callbackId; listeners.set(id, args); return id; }
            if (command === 'plugin:event|unlisten') { listeners.delete(args.eventId); return; }
            if (command === 'queue_snapshot') return Array.from({ length: size }, (_, i) => ({ id: String(i), filename: `file-${i}.zip`, url: 'https://example.com/file', directory: 'C:/Downloads', state: 'finished', downloadedBytes: 100, totalBytes: 100, bytesPerSecond: 0, etaSeconds: null, connections: 1, priority: 0, error: null, createdAt: '2026-09-08T00:00:00Z' }));
            if (command === 'get_settings') return structuredClone(window.settings);
            if (command === 'update_settings') { window.settings = structuredClone(args.settings); return; }
            if (command === 'api_status') return { listening: true, port: 9876, token: 'test-token', detail: null };
            if (command === 'music_tools') return { ffmpeg: null, ffprobe: false };
            if (command === 'music_settings') return { output: { format: 'mp3', bitrate: 192, directory: null, filenameTemplate: '{title}', folderLayout: 'flat' }, maxConcurrent: 2, preferYoutubeMusic: true };
            if (command === 'discord_status') return { connected: false, hasCredential: false, botUsername: null };
            if (command === 'discord_settings') return { mode: 'webhook', enabled: false, guildId: null, channelId: null };
            if (command === 'analyze_video') return { title: 'Test video', webpageUrl: args.url, formats: [], thumbnail: null };
            if (command.endsWith('directory')) return 'C:/Downloads';
            return [];
          },
        };
      }, { size });
      await page.goto(process.env.TEST_URL || 'http://127.0.0.1:4173');
      const nav = name => page.getByRole('navigation', { name: 'Primary' }).getByRole('button', { name, exact: true });
      await nav('Downloads').waitFor();
      await page.waitForTimeout(150);
      const initial = await page.evaluate(() => window.calls);
      for (const command of ['music_settings', 'music_tools', 'detect_browsers', 'list_categories']) assert.equal(initial[command] || 0, 0, `${command} loaded before visit`);
      await nav('Video').click();
      const input = page.getByPlaceholder('https://example.com/watch?v=…  or a direct .mp4 / .m3u8 / .mpd link');
      await input.fill('unfinished draft');
      await nav('Downloads').click();
      await nav('Video').click();
      assert.equal(await input.inputValue(), 'unfinished draft');
      const beforePoll = await page.evaluate(() => window.calls.list_video_jobs);
      await page.waitForTimeout(2100);
      const pollDelta = await page.evaluate(before => window.calls.list_video_jobs - before, beforePoll);
      assert.equal(pollDelta, 1, 'Video should have one shared idle polling loop');
      await nav('Settings').click();
      const bandwidth = page.getByRole('textbox', { name: 'Bandwidth limit in KB/s' });
      await bandwidth.fill('123');
      await page.getByRole('tab', { name: 'Timetable', exact: true }).click();
      await page.getByRole('switch', { name: 'Time window', exact: true }).click();
      await page.getByLabel('Window opens at').fill('21:00');
      await page.getByRole('tab', { name: 'General', exact: true }).click();
      await page.waitForFunction(() => window.settings.schedule.startMinutes === 1260);
      assert.equal(await bandwidth.inputValue(), '123');
      assert.equal(await page.evaluate(() => window.settings.bandwidthLimitBytesPerSec), 123 * 1024);
      assert.equal(await page.evaluate(() => window.settings.schedule.startMinutes), 1260);
      const settingsReads = await page.evaluate(() => window.calls.get_settings);
      await page.getByRole('tab', { name: 'Extension', exact: true }).click();
      await page.waitForTimeout(100);
      const detections = await page.evaluate(() => window.calls.detect_browsers);
      await page.getByRole('tab', { name: 'General', exact: true }).click();
      await page.getByRole('tab', { name: 'Extension', exact: true }).click();
      assert.equal(await page.evaluate(() => window.calls.detect_browsers), detections);
      assert.equal(await page.evaluate(() => window.calls.get_settings), settingsReads);
      await page.getByRole('button', { name: 'Open browser extension setup' }).click();
      await page.getByRole('button', { name: 'Open extension settings' }).click();
      assert.equal(await nav('Settings').getAttribute('aria-current'), 'page');
      assert.equal(
        await page.getByRole('tab', { name: 'Extension', exact: true }).getAttribute('aria-selected'),
        'true',
      );
      await nav('Discord').click();
      await page.waitForTimeout(100);
      await nav('Downloads').click();
      const discordReads = await page.evaluate(() => window.calls.discord_deliveries);
      await page.waitForTimeout(1700);
      assert.equal(await page.evaluate(() => window.calls.discord_deliveries), discordReads, 'Hidden Discord panel still polls');
      await page.evaluate(() => window.emitTestEvent('zoltraak://video-prefill', { url: 'https://example.com/video', headers: [] }));
      await page.waitForFunction(() => document.querySelector('[aria-label="Video page or media URL"]')?.value === 'https://example.com/video');
      assert.equal(await nav('Video').getAttribute('aria-current'), 'page');
      assert.equal(await input.inputValue(), 'https://example.com/video');
      await nav('Music').click();
      await page.waitForTimeout(100);
      await nav('Images').click();
      await nav('Rules').click();
      if (size) {
        await nav('Downloads').click();
        const last = page.locator(`[data-row-id="${size - 1}"]`);
        await last.scrollIntoViewIfNeeded();
        await last.getByRole('checkbox').click();
        assert.equal(await last.getByRole('checkbox').getAttribute('aria-checked'), 'true');
      }
      const tabTimings = await page.evaluate(async () => {
        const buttons = [...document.querySelectorAll('nav[aria-label="Primary"] button')];
        const timings = [];
        const queue = await window.__TAURI_INTERNALS__.invoke('queue_snapshot');
        const progress = queue.length ? setInterval(() => {
          queue[0] = { ...queue[0], state: 'downloading', downloadedBytes: queue[0].downloadedBytes + 1, totalBytes: 10000, bytesPerSecond: 100 };
          window.emitTestEvent('zoltraak://queue', structuredClone(queue));
        }, 400) : null;
        for (let i = 0; i < 24; i++) {
          const start = performance.now();
          buttons[i % buttons.length].click();
          await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          timings.push(performance.now() - start);
        }
        clearInterval(progress);
        return timings.sort((a, b) => a - b);
      });
      assert.deepEqual(errors, []);

      if (size === 0) {
        for (const viewport of [
          { width: 640, height: 480, compact: true },
          { width: 1024, height: 640, compact: false },
          { width: 1360, height: 768, compact: false },
        ]) {
          await page.setViewportSize(viewport);
          await page.waitForTimeout(100);
          const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
          assert.equal(overflow, 0, `Header overflows horizontally at ${viewport.width}px`);
          const compactButton = page.getByRole('button', { name: /navigation\. Current section:/ });
          if (viewport.compact) {
            await compactButton.click();
            await page.getByRole('navigation', { name: 'Primary' }).getByRole('button', { name: 'Settings', exact: true }).click();
            assert.equal(await compactButton.getAttribute('aria-expanded'), 'false', 'Compact menu should close after navigation');
          } else {
            assert.equal(await compactButton.count(), 0, `Compact menu should be hidden at ${viewport.width}px`);
            await nav('Downloads').waitFor();
          }
        }
      }

      console.log(JSON.stringify({ historyRows: size, sharedVideoPollsIn2100ms: pollDelta,
        repeatTabPaintP95ms: Math.round(tabTimings[Math.ceil(tabTimings.length * 0.95) - 1]), result: 'passed' }));
      await page.close();
    }
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
