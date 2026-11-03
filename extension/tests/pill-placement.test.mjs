import assert from 'node:assert/strict';
import { test } from 'node:test';
import '../pill-placement.js';

const P = globalThis.ZoltraakPillPlacement;
const viewport = { width: 1200, height: 800 };
const rectAt = (left, top, width = 120, height = 34) => ({ left, top, right: left + width, bottom: top + height });

test('defaults apply to anything missing or invalid', () => {
  assert.deepEqual(P.sanitize(undefined), { ...P.DEFAULTS });
  const messy = P.sanitize({ anchor: 'nowhere', margin: 999, scale: 5, idleOpacity: 'x', size: 'huge', offsetX: Infinity });
  assert.equal(messy.anchor, 'bottom-left');
  assert.equal(messy.margin, 80);
  assert.equal(messy.scale, 75);
  assert.equal(messy.idleOpacity, 100);
  assert.equal(messy.size, 'standard');
  assert.equal(messy.offsetX, 0);
});

test('the old single corner is honoured only until a placement is saved', () => {
  assert.equal(P.sanitize(undefined, 'top-right').anchor, 'top-right');
  assert.equal(P.sanitize({ anchor: 'middle-left' }, 'top-right').anchor, 'middle-left');
  assert.equal(P.sanitize(undefined, 'sideways').anchor, 'bottom-left');
});

test('every anchor produces a pinned or centred host style', () => {
  assert.deepEqual(
    P.hostStyle({ anchor: 'bottom-left', margin: 20 }),
    { top: 'auto', right: 'auto', bottom: '20px', left: '20px', transform: 'none' },
  );
  const topCentre = P.hostStyle({ anchor: 'top-center', margin: 10, offsetX: 30 });
  assert.equal(topCentre.left, 'calc(50% + 30px)');
  assert.equal(topCentre.transform, 'translateX(-50%)');
  assert.equal(topCentre.top, '10px');
  const middleRight = P.hostStyle({ anchor: 'middle-right', margin: 12, offsetY: -40 });
  assert.equal(middleRight.right, '12px');
  assert.equal(middleRight.top, 'calc(50% + -40px)');
  for (const anchor of P.ANCHORS) assert.ok(P.transformOrigin(anchor).split(' ').length === 2);
});

test('offsets move the pill right and down from its anchor', () => {
  assert.equal(P.hostStyle({ anchor: 'top-left', margin: 20, offsetX: 50, offsetY: 8 }).left, '70px');
  assert.equal(P.hostStyle({ anchor: 'bottom-right', margin: 20, offsetX: 50, offsetY: 8 }).right, '-30px');
  assert.equal(P.hostStyle({ anchor: 'bottom-right', margin: 20, offsetX: 50, offsetY: -8 }).bottom, '28px');
});

test('a point on screen picks the nearest anchor', () => {
  assert.equal(P.zoneAnchor(50, 50, 1200, 800), 'top-left');
  assert.equal(P.zoneAnchor(600, 40, 1200, 800), 'top-center');
  assert.equal(P.zoneAnchor(1150, 400, 1200, 800), 'middle-right');
  assert.equal(P.zoneAnchor(40, 400, 1200, 800), 'middle-left');
  assert.equal(P.zoneAnchor(600, 790, 1200, 800), 'bottom-center');
  assert.equal(P.zoneAnchor(1190, 790, 1200, 800), 'bottom-right');
  // The centre cell goes to whichever edge is closest.
  assert.equal(P.zoneAnchor(450, 400, 1200, 800), 'middle-left');
  assert.equal(P.zoneAnchor(600, 300, 1200, 800), 'top-center');
});

test('a drop near an anchor snaps onto it with no offset', () => {
  const drop = P.placementFromDrop({ rect: rectAt(30, 30), pointer: { x: 40, y: 40 }, viewport });
  assert.deepEqual(drop, { anchor: 'top-left', offsetX: 0, offsetY: 0 });
});

test('a drop away from the resting place keeps its distance along the edge', () => {
  const drop = P.placementFromDrop({ rect: rectAt(300, 20), pointer: { x: 320, y: 30 }, viewport });
  assert.equal(drop.anchor, 'top-left');
  assert.equal(drop.offsetX, 280);
  assert.equal(drop.offsetY, 0);
  const right = P.placementFromDrop({ rect: rectAt(1060, 450), pointer: { x: 1100, y: 410 }, viewport });
  assert.equal(right.anchor, 'middle-right');
  assert.equal(right.offsetX, 0);
  assert.equal(right.offsetY, 450 - (400 - P.MIDDLE_HALF));
});

test('a placement that is dropped resolves back to the same spot', () => {
  const drop = P.placementFromDrop({ rect: rectAt(400, 20, 160), pointer: { x: 480, y: 36 }, viewport });
  assert.equal(drop.anchor, 'top-center');
  const style = P.hostStyle({ ...drop, margin: 20 });
  assert.equal(style.top, '20px');
  assert.equal(style.transform, 'translateX(-50%)');
  // Centre of the 160px-wide pill was at 480, 120px left of the 600 midline.
  assert.equal(style.left, 'calc(50% + -120px)');
});

test('per-site spots win only when per-site is on', () => {
  const sites = { 'example.com': { anchor: 'top-right', offsetX: 5, offsetY: 6 } };
  const global = { anchor: 'bottom-left', scale: 120, perSite: true };
  const site = P.resolveForSite(global, sites, 'example.com');
  assert.equal(site.anchor, 'top-right');
  assert.equal(site.offsetX, 5);
  assert.equal(site.scale, 120);
  assert.equal(P.resolveForSite(global, sites, 'other.test').anchor, 'bottom-left');
  assert.equal(P.resolveForSite({ ...global, perSite: false }, sites, 'example.com').anchor, 'bottom-left');
});

test('a settings backup never carries the pairing token and re-validates on import', () => {
  const backup = P.buildBackup({ token: 'secret', captureAll: false, pillPlacement: { anchor: 'top-center' }, junk: 1 });
  assert.equal(JSON.stringify(backup).includes('secret'), false);
  assert.equal(backup.settings.junk, undefined);

  const parsed = P.parseBackup(JSON.stringify({
    format: 'zoltraak-extension-settings', version: 1,
    settings: { captureAll: 'yes', enableSfx: false, pillPlacement: { anchor: 'bogus', margin: 500 }, token: 'x' },
  }));
  assert.equal(parsed.ok, true);
  assert.equal(parsed.values.captureAll, undefined);
  assert.equal(parsed.values.enableSfx, false);
  assert.equal(parsed.values.pillPlacement.anchor, 'bottom-left');
  assert.equal(parsed.values.pillPlacement.margin, 80);
  assert.equal('token' in parsed.values, false);

  assert.equal(P.parseBackup('nope').ok, false);
  assert.equal(P.parseBackup('{"format":"other"}').ok, false);
  assert.equal(P.parseBackup(JSON.stringify({ format: 'zoltraak-extension-settings', settings: {} })).ok, false);
});
