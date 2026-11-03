import test from "node:test";
import assert from "node:assert/strict";
import { compareExtensionVersions, extensionUpdateConfirmed, extensionUpdateRows, type PendingExtensionUpdate } from "../src/lib/extensionUpdates.ts";
import type { DetectedBrowser, ExtensionClientSummary } from "../src/lib/engine.ts";

const browser: DetectedBrowser = { id: "zen", name: "Zen", kind: "firefox", installed: true, running: true, executablePath: null };
function client(id: string, version: string, seen = 101): ExtensionClientSummary {
  return { clientId: id, browserId: "zen", browserName: "Zen", extensionVersion: version, lastSeenAtMs: seen, online: true, preferred: false, active: false, temporary: false };
}
test("extension versions compare numerically and invalid versions cannot report success", () => {
  assert.equal(compareExtensionVersions("0.1.9", "0.1.41"), -1);
  assert.equal(compareExtensionVersions("1.2", "1.2.0"), 0);
  assert.equal(compareExtensionVersions("unknown", "0.1.41"), null);
});
test("mixed browser profiles still expose an update and missing signed packages are explicit", () => {
  const mixed = extensionUpdateRows([browser], [client("old", "0.1.40"), client("new", "0.1.41")], "0.1.41", "0.1.41")[0]!;
  assert.equal(mixed.updateAvailable, true); assert.equal(mixed.verifiedCurrent, false);
  assert.deepEqual(mixed.versions, ["0.1.40", "0.1.41"]);
  const missing = extensionUpdateRows([browser], [], "0.1.41", null)[0]!;
  assert.equal(missing.needsSignedPackage, true); assert.equal(missing.verifiedCurrent, false);
  const stale = extensionUpdateRows([browser], [client("old", "0.1.39")], "0.1.41", "0.1.40")[0]!;
  assert.equal(stale.needsSignedPackage, true);
  const offline = extensionUpdateRows([browser], [{ ...client("offline", "0.1.41"), online: false }], "0.1.41", "0.1.41")[0]!;
  assert.equal(offline.verifiedCurrent, false);
});
test("success requires fresh target-version heartbeats from every previously online profile", () => {
  const pending: PendingExtensionUpdate = { browserId: "zen", browserName: "Zen", targetVersion: "0.1.41", startedAt: 100, profileIds: ["a", "b"], phase: "waiting" };
  assert.equal(extensionUpdateConfirmed(pending, []), false);
  assert.equal(extensionUpdateConfirmed(pending, [client("a", "0.1.41", 99), client("b", "0.1.41", 99)]), false);
  assert.equal(extensionUpdateConfirmed(pending, [client("a", "0.1.41"), client("b", "0.1.40")]), false);
  assert.equal(extensionUpdateConfirmed(pending, [client("a", "0.1.41"), { ...client("b", "0.1.41"), online: false }]), false);
  assert.equal(extensionUpdateConfirmed(pending, [client("a", "0.1.41"), client("b", "0.1.41")]), true);
  assert.equal(extensionUpdateConfirmed({ ...pending, profileIds: [] }, [client("a", "unknown")]), false);
  assert.equal(extensionUpdateConfirmed({ ...pending, profileIds: [] }, [client("a", "0.1.41")]), true);
});
