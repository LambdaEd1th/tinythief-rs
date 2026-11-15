//! `SharedObject` persistence backed by `localStorage`.

use base64::prelude::BASE64_STANDARD;
use base64::Engine as _;
use ruffle_core::backend::storage::StorageBackend;

pub struct LocalStorageBackend {
    storage: web_sys::Storage,
}

impl LocalStorageBackend {
    pub fn new(storage: web_sys::Storage) -> Self {
        Self { storage }
    }

    fn key(name: &str) -> String {
        // Ruffle passes `TinyThief.swf/tinyThiefDataNew`; keep it readable but namespaced.
        format!("tinythief:{name}")
    }
}

impl StorageBackend for LocalStorageBackend {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        let raw = self.storage.get(&Self::key(name)).ok().flatten()?;
        BASE64_STANDARD.decode(raw).ok()
    }

    fn put(&mut self, name: &str, value: &[u8]) -> bool {
        self.storage
            .set(&Self::key(name), &BASE64_STANDARD.encode(value))
            .is_ok()
    }

    fn remove_key(&mut self, name: &str) {
        let _ = self.storage.remove_item(&Self::key(name));
    }
}
