import { test } from "node:test";
import assert from "node:assert/strict";
import { createVault, isInstalledApp, memoryStore } from "../static/vault.mjs";

const HOUR = 3_600_000;
const POOL = {
  id: 7, name: "John", tier_name: "public", updated_at: "2026-10-24T00:00:00Z",
  books: [{ name: "John", min_chapter: 1, max_chapter: 1 }],
  questions: [{ question: "Who?", type: "General", reference: "John 1:1", answer: "The Word" }],
};

function setup(clock = { time: 1_000_000 }) {
  const store = memoryStore();
  const vault = createVault({ store, now: () => clock.time });
  return { store, vault, clock };
}
const alice = (clock, hours = 24) => ({ userId: 1, username: "alice", expiresAt: clock.time + hours * HOUR });

test("pools round-trip and are stored only as ciphertext", async () => {
  const { store, vault, clock } = setup();
  await vault.startLease(alice(clock));
  await vault.savePool(POOL);
  assert.deepEqual(await vault.loadPool(7), POOL);
  assert.deepEqual((await vault.pools()).map(p => p.name), ["John"]);

  const stored = await store.get("pool:7");
  assert.ok(stored.data instanceof ArrayBuffer);
  assert.ok(!Buffer.from(stored.data).toString("latin1").includes("The Word"));
  assert.equal((await store.get("key")).extractable, false);
});

test("saving needs a live session", async () => {
  const { vault } = setup();
  await assert.rejects(vault.savePool(POOL), /No active session/);
});

test("an expired lease wipes everything", async () => {
  const { store, vault, clock } = setup();
  await vault.startLease(alice(clock, 1));
  await vault.savePool(POOL);
  clock.time += 2 * HOUR;
  assert.equal(await vault.lease(), null);
  assert.equal(await vault.loadPool(7), null);
  assert.deepEqual(await store.keys(), []);
});

test("a lease must have a real expiry", async () => {
  const { vault } = setup();
  await assert.rejects(vault.startLease({ userId: 1, username: "a", expiresAt: NaN }), /expiry/);
});

test("a different user signing in starts from nothing", async () => {
  const { vault, clock } = setup();
  await vault.startLease(alice(clock));
  await vault.savePool(POOL);
  await vault.startLease({ userId: 2, username: "bob", expiresAt: clock.time + HOUR });
  assert.equal(await vault.loadPool(7), null);
  assert.deepEqual(await vault.pools(), []);
  assert.equal((await vault.lease()).username, "bob");
});

test("renewing the same user's lease keeps their pools", async () => {
  const { vault, clock } = setup();
  await vault.startLease(alice(clock, 1));
  await vault.savePool(POOL);
  await vault.startLease(alice(clock, 24));
  assert.deepEqual(await vault.loadPool(7), POOL);
});

test("records cannot be moved between pools or users", async () => {
  const { store, vault, clock } = setup();
  await vault.startLease(alice(clock));
  await vault.savePool(POOL);
  await store.put("pool:8", await store.get("pool:7"));
  await store.put("meta:8", await store.get("meta:7"));
  assert.equal(await vault.loadPool(8), null);
});

test("dropping a pool and wiping remove data", async () => {
  const { store, vault, clock } = setup();
  await vault.startLease(alice(clock));
  await vault.savePool(POOL);
  await vault.dropPool(7);
  assert.equal(await vault.loadPool(7), null);
  await vault.savePool(POOL);
  await vault.wipe();
  assert.deepEqual(await store.keys(), []);
});

test("the vault is only available in an installed app", () => {
  const base = { indexedDB: {}, crypto: { subtle: {} } };
  const media = matches => query => ({ matches: matches && query.includes("standalone") });
  assert.equal(isInstalledApp({ ...base, matchMedia: media(false) }), false);
  assert.equal(isInstalledApp({ ...base, matchMedia: media(true) }), true);
  assert.equal(isInstalledApp({ ...base, matchMedia: media(false), navigator: { standalone: true } }), true);
  assert.equal(isInstalledApp({ ...base, matchMedia: media(true), indexedDB: undefined }), false);
  assert.equal(isInstalledApp({}), false);
});
