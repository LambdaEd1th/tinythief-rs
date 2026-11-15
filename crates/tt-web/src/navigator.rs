//! HTTP asset loading for the browser host.
//!
//! Every `TinyThief.app` asset is fetched over HTTP and decrypted in memory with
//! the same AES-256-CBC/PKCS7 scheme the native host uses. Because GitHub Pages
//! serves static files, the encrypted bundle can be published as-is.

use std::sync::Arc;

use ruffle_core::backend::navigator::{
    ErrorResponse, NavigationMethod, NavigatorBackend, OwnedFuture, Request, SuccessResponse,
};
use ruffle_core::indexmap::IndexMap;
use ruffle_core::loader::Error;
use url::{ParseError, Url};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use tt_assets::{classify, decrypt, AssetKind};

/// Serves `TinyThief.app` over HTTP, decrypting `.swf` / `.xml` transparently.
#[derive(Clone)]
pub struct WebNavigator {
    base_url: String,
}

impl WebNavigator {
    /// `base_url` is where the app bundle lives, e.g. `assets/` or
    /// `https://user.github.io/repo/assets/`.
    pub fn new(base_url: String) -> Self {
        let mut base = base_url.trim().to_string();
        // Resolve a page-relative base to an absolute one so `fetch` and Ruffle's
        // URL handling agree.
        if !base.starts_with("http://") && !base.starts_with("https://") {
            if let Some(origin) = web_sys::window()
                .map(|w| w.location().origin().ok())
                .flatten()
            {
                let path = web_sys::window()
                    .and_then(|w| w.location().pathname().ok())
                    .unwrap_or_default();
                let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                base = format!("{origin}{dir}/{base}");
            }
        }
        if !base.ends_with('/') {
            base.push('/');
        }
        Self { base_url: base }
    }

    /// Absolute URL for a bundle-relative path.
    pub fn abs_url(&self, rel: &str) -> String {
        let rel = rel.trim_start_matches('/');
        if rel.starts_with("http://") || rel.starts_with("https://") || rel.starts_with("data:") {
            return rel.to_string();
        }
        format!("{}{}", self.base_url, rel)
    }

    /// Fetch + decrypt one asset.
    pub async fn load_asset(&self, rel: &str) -> Result<Vec<u8>, String> {
        let url = self.abs_url(rel);
        tracing::info!(asset = %rel, "asset fetched");
        let bytes = fetch_bytes(&url).await?;
        match classify(rel, &bytes) {
            AssetKind::Encrypted => decrypt(&bytes).map_err(|e| format!("decrypt {rel}: {e}")),
            _ => Ok(bytes),
        }
    }

    /// Fetch + decrypt the boot movie; returns bytes and its absolute URL.
    pub async fn load_movie(&self, rel: &str) -> Result<(Vec<u8>, String), String> {
        Ok((self.load_asset(rel).await?, self.abs_url(rel)))
    }
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let resp_value = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("fetch {url}: {e:?}"))?;
    let resp: web_sys::Response = resp_value
        .dyn_into()
        .map_err(|_| format!("fetch {url}: not a Response"))?;
    if !resp.ok() {
        return Err(format!("fetch {url}: HTTP {}", resp.status()));
    }
    let buf =
        wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(|_| "no array_buffer")?)
            .await
            .map_err(|e| format!("read {url}: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// Response type carrying already-decrypted bytes.
struct WebResponse {
    url: String,
    body: Arc<Vec<u8>>,
    cursor: usize,
}

impl SuccessResponse for WebResponse {
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

impl NavigatorBackend for WebNavigator {
    fn navigate_to_url(
        &self,
        url: &str,
        target: &str,
        vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
        let _ = vars_method;
        tracing::info!(%url, %target, "navigate_to_url (opening in new tab)");
        if let Some(window) = web_sys::window() {
            let _ = window.open_with_url_and_target_and_features(
                &self.abs_url(url),
                target,
                "noopener",
            );
        }
    }

    fn fetch(&self, request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        let raw = request.url().to_string();
        let this = self.clone();
        Box::pin(async move {
            let rel = strip_base(&raw, &this.base_url);
            match this.load_asset(&rel).await {
                Ok(bytes) => Ok(Box::new(WebResponse {
                    url: raw,
                    body: Arc::new(bytes),
                    cursor: 0,
                }) as Box<dyn SuccessResponse>),
                Err(e) => Err(ErrorResponse {
                    url: raw,
                    error: Error::FetchError(e),
                }),
            }
        })
    }

    fn resolve_url(&self, url: &str) -> Result<Url, ParseError> {
        if let Ok(parsed) = Url::parse(url) {
            return Ok(parsed);
        }
        Url::parse(&self.abs_url(url))
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), Error>) {
        spawn_local(async move {
            if let Err(e) = future.await {
                tracing::warn!("spawned future failed: {e}");
            }
        });
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
        tracing::warn!(%host, port, "sockets are not supported in the browser build");
    }
}

/// Turn an absolute URL back into a bundle-relative path.
fn strip_base(url: &str, base: &str) -> String {
    if let Some(rest) = url.strip_prefix(base) {
        return rest.to_string();
    }
    // Ruffle may hand us the relative path directly.
    url.trim_start_matches('/')
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_string()
}
