//! `SharedObject` persistence backed by **IndexedDB**.
//!
//! Ruffle's [`StorageBackend`] is synchronous, while IndexedDB is not, so this
//! backend keeps an in-memory mirror that is loaded once at startup and writes
//! through to IndexedDB asynchronously (fire-and-forget, last write wins).
//!
//! Saves from the older `localStorage` backend are intentionally **not**
//! imported: the IndexedDB store starts empty.

use async_channel::bounded;
use js_sys::Uint8Array;
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{IdbDatabase, IdbObjectStore, IdbRequest, IdbTransactionMode};

use ruffle_core::backend::storage::StorageBackend;

const DB_NAME: &str = "tinythief";
const STORE: &str = "sharedobjects";

pub struct IdbStorage {
    db: IdbDatabase,
    cache: RefCell<HashMap<String, Vec<u8>>>,
}

impl IdbStorage {
    /// Open (or create) the database and preload every stored shared object.
    pub async fn open() -> Result<Self, JsValue> {
        let window = web_sys::window().ok_or("no window")?;
        let factory = window
            .indexed_db()
            .map_err(|e| format!("indexedDB unavailable: {e:?}"))?
            .ok_or("indexedDB is not available in this browser context")?;

        let open_req = factory.open_with_u32(DB_NAME, 1)?;
        {
            let req = open_req.clone();
            let on_upgrade = Closure::once_into_js(move |_: web_sys::Event| {
                if let Ok(db) = req.result().and_then(|v| v.dyn_into::<IdbDatabase>()) {
                    let _ = db.create_object_store(STORE);
                }
            });
            open_req.set_onupgradeneeded(Some(on_upgrade.unchecked_ref()));
        }
        let db: IdbDatabase = await_request(open_req.into()).await?.dyn_into()?;

        let cache: HashMap<String, Vec<u8>> = load_all(&db).await?.into_iter().collect();
        tracing::info!(keys = cache.len(), "indexeddb: opened shared object store");

        Ok(Self {
            db,
            cache: RefCell::new(cache),
        })
    }

    fn persist(&self, key: &str, value: &[u8]) {
        let db = self.db.clone();
        let key = key.to_string();
        let bytes = value.to_vec();
        spawn_local(async move {
            if let Err(e) = write_one(&db, &key, &bytes).await {
                tracing::warn!("indexeddb: failed to store {key}: {e:?}");
            }
        });
    }

    fn delete(&self, key: &str) {
        let db = self.db.clone();
        let key = key.to_string();
        spawn_local(async move {
            if let Err(e) = delete_one(&db, &key).await {
                tracing::warn!("indexeddb: failed to delete {key}: {e:?}");
            }
        });
    }
}

impl StorageBackend for IdbStorage {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        self.cache.borrow().get(name).cloned()
    }

    fn put(&mut self, name: &str, value: &[u8]) -> bool {
        self.cache
            .borrow_mut()
            .insert(name.to_string(), value.to_vec());
        self.persist(name, value);
        true
    }

    fn remove_key(&mut self, name: &str) {
        self.cache.borrow_mut().remove(name);
        self.delete(name);
    }
}

// ---------------------------------------------------------------------------
// IndexedDB plumbing
// ---------------------------------------------------------------------------

/// Resolve an `IdbRequest` through its `success`/`error` events.
async fn await_request(req: IdbRequest) -> Result<JsValue, JsValue> {
    let (tx, rx) = bounded::<Result<JsValue, JsValue>>(1);

    let ok_tx = tx.clone();
    let ok_req = req.clone();
    let on_success = Closure::once_into_js(move |_: web_sys::Event| {
        let value = ok_req.result().unwrap_or(JsValue::UNDEFINED);
        let _ = ok_tx.try_send(Ok(value));
    });

    let err_tx = tx;
    let err_req = req.clone();
    let on_error = Closure::once_into_js(move |_: web_sys::Event| {
        let err = err_req
            .error()
            .ok()
            .flatten()
            .map(JsValue::from)
            .unwrap_or_else(|| JsValue::from_str("indexeddb request failed"));
        let _ = err_tx.try_send(Err(err));
    });

    req.set_onsuccess(Some(on_success.unchecked_ref()));
    req.set_onerror(Some(on_error.unchecked_ref()));

    rx.recv()
        .await
        .map_err(|e| JsValue::from_str(&format!("indexeddb channel: {e}")))?
}

fn store(db: &IdbDatabase, mode: IdbTransactionMode) -> Result<IdbObjectStore, JsValue> {
    let tx = db.transaction_with_str_and_mode(STORE, mode)?;
    tx.object_store(STORE)
}

/// Load every stored shared object in a single readonly transaction.
///
/// Both requests are issued before awaiting, so the transaction cannot
/// auto-commit between them (a per-key transaction was observed to stall in
/// Chrome when the transaction handle was dropped early).
async fn load_all(db: &IdbDatabase) -> Result<Vec<(String, Vec<u8>)>, JsValue> {
    let tx = db.transaction_with_str_and_mode(STORE, IdbTransactionMode::Readonly)?;
    let os = tx.object_store(STORE)?;
    let keys_req = os.get_all_keys()?;
    let values_req = os.get_all()?;

    let keys: js_sys::Array = await_request(keys_req).await?.dyn_into()?;
    let values: js_sys::Array = await_request(values_req).await?.dyn_into()?;

    let mut out = Vec::with_capacity(keys.length() as usize);
    for i in 0..keys.length() {
        let Some(key) = keys.get(i).as_string() else {
            continue;
        };
        let value = values.get(i);
        let Some(bytes) = value.dyn_ref::<Uint8Array>() else {
            continue;
        };
        out.push((key, bytes.to_vec()));
    }
    Ok(out)
}

async fn write_one(db: &IdbDatabase, key: &str, value: &[u8]) -> Result<(), JsValue> {
    let os = store(db, IdbTransactionMode::Readwrite)?;
    let array = Uint8Array::from(value);
    await_request(os.put_with_key(&array, &JsValue::from_str(key))?).await?;
    Ok(())
}

async fn delete_one(db: &IdbDatabase, key: &str) -> Result<(), JsValue> {
    let os = store(db, IdbTransactionMode::Readwrite)?;
    await_request(os.delete(&JsValue::from_str(key))?).await?;
    Ok(())
}
