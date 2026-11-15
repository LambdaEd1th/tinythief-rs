//! Cross-platform service abstraction for the Tiny Thief host.
//!
//! The iOS build exposes exactly four native extension objects to ActionScript
//! (`GameServicesInterface`, `NativeUtils`, `FlurryInterface`, `IAPInterface`),
//! called through `ExternalInterface.call("<Interface>.<method>", ...)`.
//! This crate models those 29 functions as plain Rust traits so the host can
//! swap in real backends (Game Center / Play Games / Steamworks, StoreKit /
//! Play Billing, analytics, NTP…) without touching the AS3 bridge.

use std::sync::Arc;

use tracing::{debug, info};

/// Result of the platform login handshake (`_loginUser`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginResult {
    LoggedIn { player_id: String, alias: String },
    Failed { code: i32, message: String },
}

/// `GameServicesInterface` — 9 functions.
pub trait GameServices: Send + Sync {
    fn login(&self) -> LoginResult;
    fn login_error_status(&self) -> i32;
    fn show_leaderboard(&self, category: &str);
    fn submit_score(&self, category: &str, score: i64);
    fn show_achievements(&self);
    fn set_achievement(&self, id: &str, percent: f64);
    fn reset_achievements(&self);
    fn show_matchmaking(&self);
    /// Value handed back to AS3 for `_receiveOnlineResult`.
    fn receive_online_result(&self) -> Option<String>;
}

/// `NativeUtils` — 10 functions.
pub trait NativeUtils: Send + Sync {
    fn device_language(&self) -> String;
    fn is_large_screen(&self) -> bool;
    fn is_low_memory_device(&self) -> bool;
    fn share_text(&self, text: &str);
    fn platform_id(&self) -> String;
    fn open_rovio_news(&self, url: &str);
    fn close_rovio_news(&self);
    fn prompt_user_to_rate_app(&self);
    fn remove_splash_image(&self);
    /// Trusted wall-clock time in seconds (SNTP in the original).
    fn ntp_time(&self) -> i64;
}

/// `FlurryInterface` — 6 functions.
pub trait Analytics: Send + Sync {
    /// `params` carries the AS3 event metadata object (e.g. Customer_ID, Session_ID).
    fn log_event(&self, name: &str, params: &[(String, String)]);
    fn end_timed_event(&self, name: &str);
    fn log_error(&self, message: &str);
    fn set_age(&self, age: i32);
    fn set_gender(&self, gender: &str);
    fn set_user_id(&self, id: &str);
}

/// `IAPInterface` — 3 functions.
pub trait Iap: Send + Sync {
    fn retrieve_products(&self, product_ids: &[String]);
    /// Result is delivered back to AS3 asynchronously by the host.
    fn purchase(&self, product_id: &str);
    fn restore_completed_purchases(&self);
}

/// The four interfaces, bundled.
#[derive(Clone)]
pub struct Platform {
    pub game_services: Arc<dyn GameServices>,
    pub native_utils: Arc<dyn NativeUtils>,
    pub analytics: Arc<dyn Analytics>,
    pub iap: Arc<dyn Iap>,
}

impl Platform {
    pub fn new(
        game_services: Arc<dyn GameServices>,
        native_utils: Arc<dyn NativeUtils>,
        analytics: Arc<dyn Analytics>,
        iap: Arc<dyn Iap>,
    ) -> Self {
        Self {
            game_services,
            native_utils,
            analytics,
            iap,
        }
    }

    /// Everything stubbed out: the game boots happily (proven with Ruffle's
    /// `--dummy-external-interface`), which is what M0/M1 need.
    pub fn null() -> Self {
        Self::new(
            Arc::new(NullGameServices),
            Arc::new(NullNativeUtils::default()),
            Arc::new(NullAnalytics),
            Arc::new(NullIap),
        )
    }
}

/// Current unix time in seconds, wasm-safe.
///
/// `std::time::SystemTime::now()` panics on `wasm32-unknown-unknown`
/// ("time not implemented on this platform"); browsers expose the clock
/// through `Date`.
#[cfg(target_family = "wasm")]
fn unix_now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

#[cfg(not(target_family = "wasm"))]
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Null backends — log everything, never fail.
// ---------------------------------------------------------------------------

pub struct NullGameServices;
impl GameServices for NullGameServices {
    fn login(&self) -> LoginResult {
        info!("[game-services] login (null backend)");
        LoginResult::Failed {
            code: 1,
            message: "no platform game services in this build".into(),
        }
    }
    fn login_error_status(&self) -> i32 {
        1
    }
    fn show_leaderboard(&self, category: &str) {
        debug!("[game-services] show_leaderboard({category})");
    }
    fn submit_score(&self, category: &str, score: i64) {
        debug!("[game-services] submit_score({category}, {score})");
    }
    fn show_achievements(&self) {
        debug!("[game-services] show_achievements");
    }
    fn set_achievement(&self, id: &str, percent: f64) {
        debug!("[game-services] set_achievement({id}, {percent})");
    }
    fn reset_achievements(&self) {
        debug!("[game-services] reset_achievements");
    }
    fn show_matchmaking(&self) {
        debug!("[game-services] show_matchmaking");
    }
    fn receive_online_result(&self) -> Option<String> {
        None
    }
}

pub struct NullNativeUtils {
    pub language: String,
    pub large_screen: bool,
}

impl Default for NullNativeUtils {
    fn default() -> Self {
        Self {
            language: "en".to_string(),
            large_screen: true,
        }
    }
}

impl NativeUtils for NullNativeUtils {
    fn device_language(&self) -> String {
        self.language.clone()
    }
    fn is_large_screen(&self) -> bool {
        self.large_screen
    }
    fn is_low_memory_device(&self) -> bool {
        false
    }
    fn share_text(&self, text: &str) {
        info!("[native-utils] share_text({text:?})");
    }
    fn platform_id(&self) -> String {
        "rust-host".to_string()
    }
    fn open_rovio_news(&self, url: &str) {
        info!("[native-utils] open_rovio_news({url})");
    }
    fn close_rovio_news(&self) {
        debug!("[native-utils] close_rovio_news");
    }
    fn prompt_user_to_rate_app(&self) {
        debug!("[native-utils] prompt_user_to_rate_app");
    }
    fn remove_splash_image(&self) {
        debug!("[native-utils] remove_splash_image");
    }
    fn ntp_time(&self) -> i64 {
        unix_now()
    }
}

pub struct NullAnalytics;
impl Analytics for NullAnalytics {
    fn log_event(&self, name: &str, params: &[(String, String)]) {
        debug!("[analytics] log_event({name}) params={params:?}");
    }
    fn end_timed_event(&self, name: &str) {
        debug!("[analytics] end_timed_event({name})");
    }
    fn log_error(&self, message: &str) {
        debug!("[analytics] log_error({message})");
    }
    fn set_age(&self, age: i32) {
        debug!("[analytics] set_age({age})");
    }
    fn set_gender(&self, gender: &str) {
        debug!("[analytics] set_gender({gender})");
    }
    fn set_user_id(&self, id: &str) {
        debug!("[analytics] set_user_id({id})");
    }
}

pub struct NullIap;
impl Iap for NullIap {
    fn retrieve_products(&self, product_ids: &[String]) {
        info!("[iap] retrieve_products({product_ids:?}) (null backend)");
    }
    fn purchase(&self, product_id: &str) {
        info!("[iap] purchase({product_id}) (null backend)");
    }
    fn restore_completed_purchases(&self) {
        info!("[iap] restore_completed_purchases (null backend)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_platform_is_usable() {
        let p = Platform::null();
        assert_eq!(p.native_utils.platform_id(), "rust-host");
        assert!(matches!(
            p.game_services.login(),
            LoginResult::Failed { .. }
        ));
        assert!(p.native_utils.ntp_time() > 1_600_000_000);
        p.iap
            .retrieve_products(&["com.roviostars.tinythief.full".into()]);
    }
}
