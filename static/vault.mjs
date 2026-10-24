// Encrypted on-device copy of question pools, for the installed app only.
//
// Pools are AES-GCM encrypted with a non-extractable key that lives in IndexedDB next to them:
// page script can use the key but never read its bytes, and nothing on disk is plaintext. Every
// record is bound (as authenticated data) to its owner and kind, so a record cannot be swapped
// for another user's or another pool's. A "lease" records whose copy this is and when their
// session ends; once it passes, or any signed-out page is seen, everything is wiped.
//
// Outside the installed app the vault is unavailable: see isInstalledApp().

const DB_NAME = "qsets-vault";
const STORE = "kv";
const KEY = "key";
const LEASE = "lease";
const VERSION = "qsets:v1";

// True only when running as an installed app (standalone window), never in a browser tab.
export function isInstalledApp(env = globalThis) {
  const standalone = env.matchMedia?.("(display-mode: standalone)")?.matches
    || env.matchMedia?.("(display-mode: window-controls-overlay)")?.matches
    || env.navigator?.standalone === true; // iOS home-screen apps
  return Boolean(standalone) && Boolean(env.indexedDB) && Boolean(env.crypto?.subtle);
}

// ── Storage backends: a small async key/value interface ──

export function indexedDbStore(indexedDB = globalThis.indexedDB) {
  let opened;
  function open() {
    opened ??= new Promise((resolve, reject) => {
      const request = indexedDB.open(DB_NAME, 1);
      request.onupgradeneeded = () => request.result.createObjectStore(STORE);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    return opened;
  }
  async function run(mode, action) {
    const db = await open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction(STORE, mode);
      const result = action(transaction.objectStore(STORE));
      transaction.oncomplete = () => resolve(result.result);
      transaction.onerror = () => reject(transaction.error);
      transaction.onabort = () => reject(transaction.error);
    });
  }
  return {
    get: key => run("readonly", store => store.get(key)),
    put: (key, value) => run("readwrite", store => store.put(value, key)),
    delete: key => run("readwrite", store => store.delete(key)),
    keys: () => run("readonly", store => store.getAllKeys()),
    // Emptied in one transaction rather than by deleting the database: a delete waits for every
    // other open connection (another tab, another page of the app) and would stall later opens.
    destroy: () => run("readwrite", store => store.clear()),
  };
}

export function memoryStore() {
  const map = new Map();
  return {
    get: async key => map.get(key),
    put: async (key, value) => { map.set(key, value); },
    delete: async key => { map.delete(key); },
    keys: async () => [...map.keys()],
    destroy: async () => map.clear(),
  };
}

// ── The vault ──

export function createVault({ store, crypto = globalThis.crypto, now = () => Date.now() }) {
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  async function key() {
    let found = await store.get(KEY);
    if (!found) {
      found = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
      await store.put(KEY, found);
    }
    return found;
  }

  async function seal(value, context) {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const data = await crypto.subtle.encrypt(
      { name: "AES-GCM", iv, additionalData: encoder.encode(`${VERSION}:${context}`) },
      await key(),
      encoder.encode(JSON.stringify(value)),
    );
    return { iv, data };
  }

  // Resolves to null for a missing record, or one that fails authentication (tampered, wrong owner).
  async function open(record, context) {
    if (!record) return null;
    try {
      const plain = await crypto.subtle.decrypt(
        { name: "AES-GCM", iv: record.iv, additionalData: encoder.encode(`${VERSION}:${context}`) },
        await key(),
        record.data,
      );
      return JSON.parse(decoder.decode(plain));
    } catch {
      return null;
    }
  }

  async function wipe() {
    await store.destroy();
  }

  // The current lease, or null. An expired lease is cleared along with everything it protected.
  async function lease() {
    const found = await open(await store.get(LEASE), LEASE);
    if (!found) return null;
    if (!(found.expiresAt > now())) {
      await wipe();
      return null;
    }
    return found;
  }

  // Records a signed-in user's session. Another user's copies are wiped first.
  async function startLease({ userId, username, expiresAt }) {
    if (!Number.isFinite(expiresAt)) throw new Error("A lease needs an expiry");
    const current = await open(await store.get(LEASE), LEASE);
    if (current && current.userId !== userId) await wipe();
    await store.put(LEASE, await seal({ userId, username, expiresAt }, LEASE));
  }

  const poolContext = (userId, id, kind) => `${kind}:${userId}:${id}`;

  async function savePool(pool) {
    const owner = await lease();
    if (!owner) throw new Error("No active session for offline storage");
    const { questions, books, ...meta } = pool;
    await store.put(`pool:${pool.id}`, await seal({ questions, books }, poolContext(owner.userId, pool.id, "pool")));
    await store.put(`meta:${pool.id}`, await seal(meta, poolContext(owner.userId, pool.id, "meta")));
  }

  async function pools() {
    const owner = await lease();
    if (!owner) return [];
    const found = [];
    for (const name of await store.keys()) {
      if (typeof name !== "string" || !name.startsWith("meta:")) continue;
      const meta = await open(await store.get(name), poolContext(owner.userId, name.slice(5), "meta"));
      if (meta) found.push(meta);
    }
    return found;
  }

  async function loadPool(id) {
    const owner = await lease();
    if (!owner) return null;
    const meta = await open(await store.get(`meta:${id}`), poolContext(owner.userId, id, "meta"));
    const body = await open(await store.get(`pool:${id}`), poolContext(owner.userId, id, "pool"));
    return meta && body ? { ...meta, ...body } : null;
  }

  async function dropPool(id) {
    await store.delete(`pool:${id}`);
    await store.delete(`meta:${id}`);
  }

  return { lease, startLease, savePool, pools, loadPool, dropPool, wipe };
}

let shared;
// The device's vault, or null outside the installed app.
export function deviceVault(env = globalThis) {
  if (!isInstalledApp(env)) return null;
  shared ??= createVault({ store: indexedDbStore(env.indexedDB), crypto: env.crypto });
  return shared;
}

// Wipes the device's vault if there is one; safe to call anywhere.
export async function wipeDeviceVault() {
  try {
    await deviceVault()?.wipe();
  } catch (error) {
    console.error("Could not clear offline data:", error);
  }
}

// Keeps the vault in step with the signed-in state. `fetchMe` resolves to the signed-in user
// (with `session_expires_at`), to null when signed out, or rejects with a TypeError when offline.
// Signed out wipes the vault; offline leaves it to the stored lease.
export async function syncDeviceVault(fetchMe) {
  const vault = deviceVault();
  if (!vault) return { vault: null, me: null, offline: false };
  try {
    const me = await fetchMe();
    if (!me) {
      await vault.wipe();
      return { vault, me: null, offline: false };
    }
    await vault.startLease({
      userId: me.id,
      username: me.username,
      expiresAt: Date.parse(me.session_expires_at),
    });
    navigator.storage?.persist?.().catch(() => {});
    return { vault, me, offline: false };
  } catch (error) {
    if (!(error instanceof TypeError)) console.error("Offline sync failed:", error);
    return { vault, me: null, offline: error instanceof TypeError };
  }
}
