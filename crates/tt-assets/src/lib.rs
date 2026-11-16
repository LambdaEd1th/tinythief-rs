//! Tiny Thief (iOS 1.2.2) content provider.
//!
//! The shipped app encrypts every `.swf` / `.xml` asset with **AES-256-CBC, zero IV,
//! PKCS7 padding**, using a static key that lives in the native binary
//! (`TinyThief` @ `0x11060`, `FileOpener__OpenFile_AESDecrypt`; key at `0x504550`).
//! `.mp3` files are *not* encrypted in this build.
//!
//! This crate gives the Rust host a virtual filesystem over `TinyThief.app` that
//! decrypts on the fly, so the game never sees ciphertext and we never have to
//! write plaintext assets to disk.

use std::path::{Component, Path, PathBuf};

use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
use thiserror::Error;

/// 32-byte content key recovered from the native binary (`0x504550`).
pub const CONTENT_KEY: &[u8; 32] = b"ypWnks9F4jrKjs1cQv3Tw32fi7epEvlu";

/// Extensions that the original `FileOpener` treats as possibly-encrypted.
const ENCRYPTED_EXTS: &[&str] = &["swf", "xml", "mp3"];

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("io error for {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("asset not found: {0}")]
    NotFound(String),
    #[error("path escapes the bundle: {0}")]
    Escape(String),
    #[error("ciphertext length {0} is not a multiple of the AES block size")]
    BadLength(usize),
    #[error("decryption failed: {0}")]
    Decrypt(String),
    #[error("{0} is not a Tiny Thief.app bundle (missing TinyThief.swf)")]
    NotABundle(String),
}

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// How a given asset is stored on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// Stored as plaintext (already decrypted, or genuinely not encrypted).
    Plain,
    /// AES-256-CBC ciphertext with PKCS7 padding.
    Encrypted,
    /// Unknown: no recognisable magic, ciphertext length not block-aligned.
    Unknown,
}

/// Raw AES-256-CBC decryption (keeps padding).
pub fn decrypt_raw(data: &[u8]) -> Result<Vec<u8>, AssetError> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err(AssetError::BadLength(data.len()));
    }
    let mut buf = data.to_vec();
    let plain = Aes256CbcDec::new(CONTENT_KEY.into(), &[0u8; 16].into())
        .decrypt_padded::<Pkcs7>(&mut buf)
        .map_err(|e| AssetError::Decrypt(format!("{e:?}")))?;
    Ok(plain.to_vec())
}

/// AES-256-CBC decryption + PKCS7 strip, tolerant of inputs that are not
/// block-aligned (some tools produce unpadded ciphertext).
pub fn decrypt(data: &[u8]) -> Result<Vec<u8>, AssetError> {
    if !data.len().is_multiple_of(16) {
        // Not block aligned -> cannot be CBC ciphertext of this scheme.
        return Err(AssetError::BadLength(data.len()));
    }
    let mut buf = data.to_vec();
    match Aes256CbcDec::new(CONTENT_KEY.into(), &[0u8; 16].into()).decrypt_padded::<Pkcs7>(&mut buf)
    {
        Ok(plain) => Ok(plain.to_vec()),
        Err(_) => {
            // No valid padding: decrypt without stripping.
            let mut buf = data.to_vec();
            Ok(Aes256CbcDec::new(CONTENT_KEY.into(), &[0u8; 16].into())
                .decrypt_padded::<cbc::cipher::block_padding::NoPadding>(&mut buf)
                .map_err(|e| AssetError::Decrypt(format!("{e:?}")))?
                .to_vec())
        }
    }
}

fn is_plain_swf(b: &[u8]) -> bool {
    matches!(&b.get(..3), Some(b"FWS" | b"CWS" | b"ZWS"))
}

fn is_plain_xml(b: &[u8]) -> bool {
    b.iter()
        .take(64)
        .copied()
        .find(|c| !(*c == 0xEF || *c == 0xBB || *c == 0xBF || c.is_ascii_whitespace()))
        == Some(b'<')
}

fn is_plain_mp3(b: &[u8]) -> bool {
    matches!(b.get(..3), Some(b"ID3"))
        || matches!(
            b.get(..2),
            Some([0xFF, 0xFB] | [0xFF, 0xFA] | [0xFF, 0xF3] | [0xFF, 0xF2])
        )
}

fn looks_plain(ext: &str, data: &[u8]) -> bool {
    match ext {
        "swf" => is_plain_swf(data),
        "xml" => is_plain_xml(data),
        "mp3" => is_plain_mp3(data),
        _ => false,
    }
}

/// Decide how an asset is stored, by extension + content sniffing.
pub fn classify(path: &str, data: &[u8]) -> AssetKind {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !ENCRYPTED_EXTS.contains(&ext.as_str()) {
        return AssetKind::Plain;
    }
    if looks_plain(&ext, data) {
        return AssetKind::Plain;
    }
    if data.len().is_multiple_of(16) && !data.is_empty() {
        if let Ok(p) = decrypt(data) {
            if looks_plain(&ext, &p) {
                return AssetKind::Encrypted;
            }
        }
    }
    AssetKind::Unknown
}

/// A read-only, decrypting view over `TinyThief.app`.
pub struct AppBundle {
    root: PathBuf,
    /// Number of assets served, by kind (useful for diagnostics).
    pub stats: std::cell::Cell<Stats>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub plain: u32,
    pub encrypted: u32,
    pub unknown: u32,
}

impl AppBundle {
    /// Open an app bundle directory (the one containing `TinyThief.swf`).
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, AssetError> {
        let root = root.into();
        let main = root.join("TinyThief.swf");
        if !main.is_file() {
            return Err(AssetError::NotABundle(root.display().to_string()));
        }
        Ok(Self {
            root,
            stats: std::cell::Cell::new(Stats::default()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a game-relative path inside the bundle, rejecting traversal.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, AssetError> {
        let rel = rel.replace('\\', "/");
        let rel = rel.split(['?', '#']).next().unwrap_or(&rel);
        let rel = rel.trim_start_matches('/');
        let cleaned = Path::new(rel);
        for comp in cleaned.components() {
            match comp {
                Component::Normal(_) => {}
                Component::CurDir => {}
                _ => return Err(AssetError::Escape(rel.to_string())),
            }
        }
        let joined = self.root.join(cleaned);
        // Defence in depth: canonicalise and ensure it stays under root.
        if let (Ok(root), Ok(p)) = (self.root.canonicalize(), joined.canonicalize()) {
            if !p.starts_with(root) {
                return Err(AssetError::Escape(rel.to_string()));
            }
        }
        Ok(joined)
    }

    /// Does this bundle contain `rel`?
    pub fn exists(&self, rel: &str) -> bool {
        self.resolve(rel).map(|p| p.is_file()).unwrap_or(false)
    }

    /// Read and (if needed) decrypt an asset.
    pub fn read(&self, rel: &str) -> Result<Vec<u8>, AssetError> {
        let path = self.resolve(rel)?;
        let data = std::fs::read(&path).map_err(|e| AssetError::Io {
            path: rel.to_string(),
            source: e,
        })?;
        let out = match classify(rel, &data) {
            AssetKind::Plain => {
                let mut s = self.stats.get();
                s.plain += 1;
                self.stats.set(s);
                data
            }
            AssetKind::Encrypted => {
                let mut s = self.stats.get();
                s.encrypted += 1;
                self.stats.set(s);
                decrypt(&data).map_err(|e| AssetError::Decrypt(format!("{rel}: {e}")))?
            }
            AssetKind::Unknown => {
                let mut s = self.stats.get();
                s.unknown += 1;
                self.stats.set(s);
                data
            }
        };
        Ok(out)
    }

    /// The boot movie (`TinyThief.swf`).
    pub fn main_swf(&self) -> Result<Vec<u8>, AssetError> {
        self.read("TinyThief.swf")
    }

    /// List every `swf/*.swf` level/menu movie, sorted.
    pub fn list_level_swfs(&self) -> std::io::Result<Vec<String>> {
        let mut out = Vec::new();
        let dir = self.root.join("swf");
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_ascii_lowercase().ends_with(".swf") {
                out.push(format!("swf/{name}"));
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> Option<AppBundle> {
        let candidates = [
            std::env::var("TT_APP").ok(),
            Some("../TinyThief.app".to_string()),
            Some("/Volumes/Data/ReserveEngineering/tinythief/TinyThief.app".to_string()),
        ];
        for c in candidates.into_iter().flatten() {
            if let Ok(b) = AppBundle::open(&c) {
                return Some(b);
            }
        }
        None
    }

    #[test]
    fn key_length() {
        assert_eq!(CONTENT_KEY.len(), 32);
    }

    #[test]
    fn pkcs7_tail_is_stripped() {
        if let Some(b) = bundle() {
            let xml = b.read("config/sounds/Fx_config.xml").expect("read xml");
            assert!(
                xml.ends_with(b"</sound_configuration>"),
                "padding leaked: {:?}",
                &xml[xml.len().saturating_sub(24)..]
            );
            let swf = b.main_swf().expect("main swf");
            assert_eq!(&swf[..3], b"CWS");
            // declared length must match the decompressed size exactly + 8 byte header
            let declared = u32::from_le_bytes([swf[4], swf[5], swf[6], swf[7]]) as usize;
            assert_eq!(declared, 8 + zlib_len(&swf[8..]), "CWS length mismatch");
        }
    }

    fn zlib_len(body: &[u8]) -> usize {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(body)
            .read_to_end(&mut out)
            .expect("inflate");
        out.len()
    }

    #[test]
    fn all_swfs_decrypt() {
        if let Some(b) = bundle() {
            let mut n = 0;
            for rel in b.list_level_swfs().expect("list") {
                let data = b.read(&rel).expect("read swf");
                assert_eq!(&data[..3], b"CWS", "{rel} did not decrypt to CWS");
                n += 1;
            }
            assert!(n >= 80, "expected >=80 level swfs, got {n}");
        }
    }

    #[test]
    fn mp3_is_plain() {
        if let Some(b) = bundle() {
            let found = walk_first_mp3(b.root()).expect("no mp3 found");
            let bytes = std::fs::read(b.root().join(&found)).expect("read mp3");
            assert_eq!(classify(&found, &bytes), AssetKind::Plain, "{found}");
        }
    }

    fn walk_first_mp3(root: &Path) -> Option<String> {
        fn rec(dir: &Path, base: &Path, out: &mut Option<String>) {
            if out.is_some() {
                return;
            }
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        rec(&p, base, out);
                    } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mp3")) {
                        *out = Some(p.strip_prefix(base).unwrap().to_string_lossy().to_string());
                    }
                }
            }
        }
        let mut out = None;
        rec(&root.join("sounds"), root, &mut out);
        out
    }

    #[test]
    fn traversal_is_rejected() {
        if let Some(b) = bundle() {
            assert!(b.resolve("../../etc/passwd").is_err());
            assert!(b.resolve("swf/menu.swf").is_ok());
        }
    }
}
