#![allow(clippy::arc_with_non_send_sync)] // futures are !Send; navigator is single-threaded
//! Asset navigator: serves `TinyThief.app` through `tt-assets`, decrypting on the fly.
//!
//! Ruffle asks the navigator for every loaded movie / XML / sound. We map the
//! requested `file://` URL back into the bundle and hand back plaintext bytes, so
//! no decrypted asset ever touches the disk.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ruffle_core::backend::navigator::{
    ErrorResponse, NavigationMethod, NavigatorBackend, OwnedFuture, SuccessResponse,
};
use ruffle_core::indexmap::IndexMap;
use ruffle_core::loader::Error;
use url::{ParseError, Url};

use tt_assets::AppBundle;

/// A response carrying already-decrypted bytes.
struct BundleResponse {
    url: String,
    body: Arc<Vec<u8>>,
    cursor: usize,
}

impl SuccessResponse for BundleResponse {
    fn url(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(&self.url)
    }

    fn set_url(&mut self, url: String) {
        self.url = url;
    }

    fn body(self: Box<Self>) -> OwnedFuture<Vec<u8>, Error> {
        Box::pin(async move { Ok((*self.body).clone()) })
    }

    fn text_encoding(&self) -> Option<&'static encoding_rs::Encoding> {
        None
    }

    fn status(&self) -> u16 {
        200
    }

    fn redirected(&self) -> bool {
        false
    }

    fn expected_length(&self) -> Result<Option<u64>, Error> {
        Ok(Some(self.body.len() as u64))
    }

    fn next_chunk(&mut self) -> OwnedFuture<Option<Vec<u8>>, Error> {
        const CHUNK: usize = 64 * 1024;
        if self.cursor >= self.body.len() {
            return Box::pin(async { Ok(None) });
        }
        let end = (self.cursor + CHUNK).min(self.body.len());
        let chunk = self.body[self.cursor..end].to_vec();
        self.cursor = end;
        Box::pin(async move { Ok(Some(chunk)) })
    }
}

/// Serves files from a `TinyThief.app` directory, decrypting on demand.
pub struct BundleNavigator {
    bundle: AppBundle,
    root_url: Url,
    root_path: PathBuf,
    /// Futures Ruffle spawned that we have to drive from the host loop.
    pending: Arc<Mutex<Vec<OwnedFuture<(), Error>>>>,
}

impl BundleNavigator {
    pub fn new(bundle: AppBundle) -> anyhow::Result<Self> {
        let root_path = bundle.root().canonicalize()?;
        let root_url = Url::from_directory_path(&root_path)
            .map_err(|_| anyhow::anyhow!("invalid bundle path: {}", root_path.display()))?;
        Ok(Self {
            bundle,
            root_url,
            root_path,
            pending: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// The URL of the boot movie (`TinyThief.swf`), used as the base URL.
    /// Absolute URL for a bundle-relative movie path.
    pub fn url_for(&self, rel: &str) -> String {
        self.root_url
            .join(rel)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("file:///{rel}"))
    }

    pub fn pending(&self) -> Arc<Mutex<Vec<OwnedFuture<(), Error>>>> {
        self.pending.clone()
    }

    /// Map an absolute `file://` URL onto a bundle-relative path.
    fn relative_path(&self, url: &str) -> Option<String> {
        // Relative URLs (the game asks for "swf/logosState.swf") are bundle-relative already.
        if let Ok(parsed) = Url::parse(url) {
            let path = parsed.to_file_path().ok()?;
            let path = path.canonicalize().unwrap_or(path);
            let rel = path.strip_prefix(&self.root_path).ok()?;
            return Some(rel.to_string_lossy().replace('\\', "/"));
        }
        let trimmed = url
            .trim_start_matches("./")
            .trim_start_matches('/')
            .split(['?', '#'])
            .next()?;
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

impl NavigatorBackend for BundleNavigator {
    fn navigate_to_url(
        &self,
        url: &str,
        target: &str,
        vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
        let _ = vars_method;
        tracing::info!(%url, %target, "navigate_to_url (ignored)");
    }

    fn fetch(
        &self,
        request: ruffle_core::backend::navigator::Request,
    ) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        let url = request.url().to_string();
        tracing::debug!(%url, "fetch");

        let Some(rel) = self.relative_path(&url) else {
            return Box::pin(async move {
                Err(ErrorResponse {
                    url,
                    error: Error::FetchError("URL outside the app bundle".into()),
                })
            });
        };

        match self.bundle.read(&rel) {
            Ok(bytes) => Box::pin(async move {
                Ok(Box::new(BundleResponse {
                    url: request.url().to_string(),
                    body: Arc::new(bytes),
                    cursor: 0,
                }) as Box<dyn SuccessResponse>)
            }),
            Err(e) => Box::pin(async move {
                Err(ErrorResponse {
                    url,
                    error: Error::FetchError(e.to_string()),
                })
            }),
        }
    }

    fn resolve_url(&self, url: &str) -> Result<Url, ParseError> {
        // Absolute URLs (including file://) pass through; relative ones hang off the bundle root.
        if let Ok(parsed) = Url::parse(url) {
            return Ok(parsed);
        }
        self.root_url.join(url)
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), Error>) {
        self.pending.lock().unwrap().push(future);
    }

    fn pre_process_url(&self, url: Url) -> Url {
        url
    }

    fn connect_socket(
        &mut self,
        host: String,
        port: u16,
        _timeout: std::time::Duration,
        _handle: ruffle_core::socket::SocketHandle,
        _receiver: async_channel::Receiver<Vec<u8>>,
        _sender: async_channel::Sender<ruffle_core::socket::SocketAction>,
    ) {
        tracing::warn!(%host, port, "socket connections are not supported by this host");
    }
}
