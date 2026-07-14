const LEGACY_DB_NAME = "pico-transfer-v1";
const DB_NAME = "pico-transfer-v2";
const DB_VERSION = 1;
const CHUNK_STORE = "chunks";
const WRITE_BATCH_SIZE = 64;
const READ_BATCH_SIZE = 256;

let dbPromise = null;
let persistQueue = Promise.resolve();
let persistError = null;
let pendingChunkWrites = [];
let chunkFlushScheduled = false;

// Secure transfer sessions are not resumable. Remove legacy plaintext staging
// and clear this version's abandoned chunks when the first database handle is
// opened after a page load.
try {
  indexedDB.deleteDatabase(LEGACY_DB_NAME);
} catch (_) {
  // Storage may be unavailable in privacy modes; normal operations report it.
}

function normalizeTransferId(transferId) {
  // wasm-bindgen passes Rust u64 values as BigInt. IndexedDB values may contain
  // BigInt, but BigInt is not a valid IndexedDB key and therefore cannot back
  // the by_transfer index. A decimal string is stable and lossless.
  return String(transferId);
}

function keyFor(transferId, chunkIndex) {
  // Keep lexical ordering stable for indexed cursor/debug views.
  return `${normalizeTransferId(transferId)}:${String(chunkIndex).padStart(10, "0")}`;
}

function requestToPromise(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = async () => {
      const db = request.result;
      try {
        const transaction = db.transaction(CHUNK_STORE, "readwrite");
        transaction.objectStore(CHUNK_STORE).clear();
        await transactionDone(transaction);
        resolve(db);
      } catch (error) {
        db.close();
        reject(error);
      }
    };
    request.onerror = () => reject(request.error || new Error("IndexedDB request failed"));
  });
}

function transactionDone(transaction) {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error || new Error("IndexedDB transaction failed"));
    transaction.onabort = () => reject(transaction.error || new Error("IndexedDB transaction aborted"));
  });
}

function rememberPersistError(error) {
  const normalized = error instanceof Error ? error : new Error(String(error));
  if (!persistError) {
    persistError = normalized;
  }
  return persistError;
}

function enqueuePersistOperation(operation) {
  persistQueue = persistQueue
    .catch(() => {})
    .then(async () => {
      if (persistError) {
        throw persistError;
      }
      await operation();
    })
    .catch((error) => {
      throw rememberPersistError(error);
    });

  // Prevent unhandled rejection noise; callers observe errors via waitForChunkPersistQueue.
  persistQueue.catch(() => {});
}

function flushPendingChunkWrites() {
  if (pendingChunkWrites.length === 0) {
    return;
  }

  const writes = pendingChunkWrites;
  pendingChunkWrites = [];

  enqueuePersistOperation(async () => {
    const db = await openDb();
    const transaction = db.transaction(CHUNK_STORE, "readwrite");
    const store = transaction.objectStore(CHUNK_STORE);
    for (const record of writes) {
      store.put(record);
    }
    await transactionDone(transaction);
  });
}

function scheduleChunkFlush() {
  if (pendingChunkWrites.length >= WRITE_BATCH_SIZE) {
    chunkFlushScheduled = false;
    flushPendingChunkWrites();
    return;
  }

  if (chunkFlushScheduled) {
    return;
  }

  chunkFlushScheduled = true;
  setTimeout(() => {
    chunkFlushScheduled = false;
    flushPendingChunkWrites();
  }, 0);
}

async function loadChunkBatch(db, transferId, startChunkIndex, count) {
  const transaction = db.transaction(CHUNK_STORE, "readonly");
  const store = transaction.objectStore(CHUNK_STORE);
  const requests = [];
  for (let i = 0; i < count; i += 1) {
    requests.push(requestToPromise(store.get(keyFor(transferId, startChunkIndex + i))));
  }
  const records = await Promise.all(requests);
  await transactionDone(transaction);
  return records;
}

function openDb() {
  if (dbPromise) {
    return dbPromise;
  }

  dbPromise = new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);

    request.onupgradeneeded = () => {
      const db = request.result;
      let store;
      if (!db.objectStoreNames.contains(CHUNK_STORE)) {
        store = db.createObjectStore(CHUNK_STORE, { keyPath: "key" });
      } else {
        store = request.transaction.objectStore(CHUNK_STORE);
      }

      if (!store.indexNames.contains("by_transfer")) {
        store.createIndex("by_transfer", "transferId", { unique: false });
      }
    };

    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("IndexedDB open failed"));
  });

  return dbPromise;
}

async function clearTransferChunksInternal(db, transferId) {
  const transaction = db.transaction(CHUNK_STORE, "readwrite");
  const store = transaction.objectStore(CHUNK_STORE);
  const index = store.index("by_transfer");
  const range = IDBKeyRange.only(normalizeTransferId(transferId));

  await new Promise((resolve, reject) => {
    const cursorRequest = index.openCursor(range);
    cursorRequest.onsuccess = () => {
      const cursor = cursorRequest.result;
      if (!cursor) {
        resolve();
        return;
      }
      store.delete(cursor.primaryKey);
      cursor.continue();
    };
    cursorRequest.onerror = () => reject(cursorRequest.error || new Error("Failed to iterate transfer chunks"));
  });

  await transactionDone(transaction);
}

export function queueChunkPersist(transferId, chunkIndex, payload) {
  if (persistError) {
    return;
  }
  const data = payload instanceof Uint8Array ? payload.slice() : new Uint8Array(payload);
  pendingChunkWrites.push({
    key: keyFor(transferId, chunkIndex),
    transferId: normalizeTransferId(transferId),
    chunkIndex,
    data,
  });
  scheduleChunkFlush();
}

export function queueClearTransferChunks(transferId) {
  // New transfer boundaries should be able to recover from a prior transfer-local write failure.
  persistError = null;
  persistQueue = persistQueue.catch(() => {});
  flushPendingChunkWrites();
  enqueuePersistOperation(async () => {
    const db = await openDb();
    await clearTransferChunksInternal(db, transferId);
  });
}

export async function waitForChunkPersistQueue() {
  flushPendingChunkWrites();
  await persistQueue.catch(() => {});
  if (persistError) {
    throw persistError;
  }
}

export async function readChunk(transferId, chunkIndex) {
  await waitForChunkPersistQueue();
  const db = await openDb();
  const transaction = db.transaction(CHUNK_STORE, "readonly");
  const store = transaction.objectStore(CHUNK_STORE);
  const record = await requestToPromise(store.get(keyFor(transferId, chunkIndex)));
  await transactionDone(transaction);
  return record ? record.data : null;
}

export async function clearTransferChunks(transferId) {
  await waitForChunkPersistQueue();
  const db = await openDb();
  await clearTransferChunksInternal(db, transferId);
}

export async function downloadTransferFromDb(transferId, chunkCount, fileName, maxBlobBytes) {
  await waitForChunkPersistQueue();
  const db = await openDb();

  if (typeof window.showSaveFilePicker === "function") {
    const handle = await window.showSaveFilePicker({
      suggestedName: fileName,
      types: [{
        description: "Binary file",
        accept: { "application/octet-stream": [".*"] },
      }],
    });
    const writable = await handle.createWritable();
    try {
      for (let i = 0; i < chunkCount; i += READ_BATCH_SIZE) {
        const batchSize = Math.min(READ_BATCH_SIZE, chunkCount - i);
        const records = await loadChunkBatch(db, transferId, i, batchSize);
        for (let j = 0; j < batchSize; j += 1) {
          const chunkIndex = i + j;
          const record = records[j];
          if (!record || !record.data || record.chunkIndex !== chunkIndex) {
            throw new Error(`Missing chunk ${chunkIndex}`);
          }
          await writable.write(record.data);
        }
      }
      await writable.close();
      return "file";
    } catch (error) {
      await writable.abort();
      throw error;
    }
  }

  const parts = [];
  let totalBytes = 0;
  for (let i = 0; i < chunkCount; i += READ_BATCH_SIZE) {
    const batchSize = Math.min(READ_BATCH_SIZE, chunkCount - i);
    const records = await loadChunkBatch(db, transferId, i, batchSize);
    for (let j = 0; j < batchSize; j += 1) {
      const chunkIndex = i + j;
      const record = records[j];
      if (!record || !record.data || record.chunkIndex !== chunkIndex) {
        throw new Error(`Missing chunk ${chunkIndex}`);
      }
      const chunk = record.data;
      totalBytes += chunk.byteLength || chunk.length || 0;
      if (totalBytes > maxBlobBytes) {
        throw new Error("File too large for blob fallback; use a browser with File System Access API");
      }
      parts.push(chunk);
    }
  }

  const blob = new Blob(parts, { type: "application/octet-stream" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  anchor.style.display = "none";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 5000);
  return "blob";
}
