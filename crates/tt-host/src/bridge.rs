//! ActionScript <-> native bridge.
//!
//! The iOS build registers four objects into AS3 and receives
//! `ExternalInterface.call("<Interface>.<method>", ...)` for 29 functions.
//! For M1 we log every call and return `null` (which is exactly what Ruffle's
//! `--dummy-external-interface` does — and the game boots fine with that).

use std::sync::{Arc, Mutex};

use ruffle_core::context::UpdateContext;
use ruffle_core::external::{ExternalInterfaceProvider, FsCommandProvider, Value};

use tt_platform::Platform;

/// A call the AS3 side should receive on the root movie clip.
///
/// Ruffle's only host->AS3 channel is `Player::call_root_method` (added in our
/// vendored patch); the bridge queues calls and the host drains them after each
/// tick, avoiding re-entrancy into the player.
pub struct RootCall {
    pub name: &'static str,
    pub args: Vec<Value>,
}

pub type Outbox = Arc<Mutex<Vec<RootCall>>>;

pub struct Bridge {
    platform: Platform,
    outbox: Outbox,
}

impl Bridge {
    /// Build a provider that shares an existing outbox (one per player).
    pub fn with_outbox(platform: Platform, outbox: Outbox) -> Self {
        Self { platform, outbox }
    }

    fn post(&self, name: &'static str, args: Vec<Value>) {
        tracing::debug!(%name, ?args, "queue root call");
        self.outbox.lock().unwrap().push(RootCall { name, args });
    }

    fn dispatch(&self, name: &str, args: &[Value]) -> Value {
        tracing::debug!(%name, ?args, "ExternalInterface call");
        // Names observed in the iOS build (see TinyThief_架构分析.md §6).
        let (interface, method) = match name.split_once('.') {
            Some(parts) => parts,
            None => {
                tracing::warn!(%name, "external interface call without interface prefix");
                return Value::Null;
            }
        };

        match (interface, method) {
            // --- GameServicesInterface: 9 functions -------------------------
            ("GameServicesInterface", "init") => {
                tracing::info!("GameServicesInterface.init");
                Value::Null
            }
            ("GameServicesInterface", "loginUser") => {
                let result = self.platform.game_services.login();
                tracing::info!(?result, "GameServicesInterface.loginUser");
                Value::Null
            }
            ("GameServicesInterface", "getLoginErrorStatus") => {
                Value::Number(self.platform.game_services.login_error_status() as f64)
            }
            ("GameServicesInterface", "openLeaderboardPane") => {
                let id = string_arg(args, 0);
                self.platform.game_services.show_leaderboard(&id);
                Value::Null
            }
            ("GameServicesInterface", "setLeaderboardScore") => {
                let id = string_arg(args, 0);
                let score = number_arg(args, 1).unwrap_or(0.0) as i64;
                self.platform.game_services.submit_score(&id, score);
                Value::Null
            }
            ("GameServicesInterface", "openAchievementPane") => {
                self.platform.game_services.show_achievements();
                Value::Null
            }
            ("GameServicesInterface", "setAchievementCompletion") => {
                let id = string_arg(args, 0);
                let pct = number_arg(args, 1).unwrap_or(0.0);
                self.platform.game_services.set_achievement(&id, pct);
                Value::Null
            }
            ("GameServicesInterface", "resetAchievements") => {
                self.platform.game_services.reset_achievements();
                Value::Null
            }
            ("GameServicesInterface", "openMatchmakingMenu") => {
                self.platform.game_services.show_matchmaking();
                Value::Null
            }
            ("GameServicesInterface", "receiveOnlineResult") => self
                .platform
                .game_services
                .receive_online_result()
                .map(Value::String)
                .unwrap_or(Value::Null),

            // --- NativeUtils: 10 functions --------------------------------
            ("NativeUtils", "init") => {
                tracing::info!("NativeUtils.init");
                Value::Null
            }
            ("NativeUtils", "getDeviceLanguage") => {
                Value::String(self.platform.native_utils.device_language())
            }
            ("NativeUtils", "isLargeScreen") => {
                Value::Bool(self.platform.native_utils.is_large_screen())
            }
            ("NativeUtils", "isLowMemoryDevice") => {
                Value::Bool(self.platform.native_utils.is_low_memory_device())
            }
            ("NativeUtils", "shareText") => {
                let text = string_arg(args, 0);
                self.platform.native_utils.share_text(&text);
                Value::Null
            }
            ("NativeUtils", "getPlatformID") => {
                Value::String(self.platform.native_utils.platform_id())
            }
            ("NativeUtils", "openRovioNews") => {
                let url = string_arg(args, 0);
                self.platform.native_utils.open_rovio_news(&url);
                Value::Null
            }
            ("NativeUtils", "closeRovioNews") => {
                self.platform.native_utils.close_rovio_news();
                Value::Null
            }
            ("NativeUtils", "promptUserToRateApp") => {
                self.platform.native_utils.prompt_user_to_rate_app();
                Value::Null
            }
            ("NativeUtils", "removeSplashImage") => {
                self.platform.native_utils.remove_splash_image();
                Value::Null
            }
            ("NativeUtils", "getNTPTime") => {
                Value::Number(self.platform.native_utils.ntp_time() as f64)
            }

            // --- FlurryInterface: 6 functions -----------------------------
            ("FlurryInterface", "init") => {
                tracing::info!("FlurryInterface.init");
                Value::Null
            }
            ("FlurryInterface", "logEvent") => {
                let name = string_arg(args, 0);
                let params = object_pairs(args.get(1));
                self.platform.analytics.log_event(&name, &params);
                Value::Null
            }
            ("FlurryInterface", "endTimedEvent") => {
                let name = string_arg(args, 0);
                self.platform.analytics.end_timed_event(&name);
                Value::Null
            }
            ("FlurryInterface", "logError") => {
                let msg = string_arg(args, 0);
                self.platform.analytics.log_error(&msg);
                Value::Null
            }
            ("FlurryInterface", "setAge") => {
                let age = number_arg(args, 0).unwrap_or(0.0) as i32;
                self.platform.analytics.set_age(age);
                Value::Null
            }
            ("FlurryInterface", "setGender") => {
                let gender = string_arg(args, 0);
                self.platform.analytics.set_gender(&gender);
                Value::Null
            }
            ("FlurryInterface", "setUserId") => {
                let id = string_arg(args, 0);
                self.platform.analytics.set_user_id(&id);
                Value::Null
            }

            // --- IAPInterface: 3 functions --------------------------------
            ("IAPInterface", "init") => {
                tracing::info!("IAPInterface.init");
                Value::Null
            }
            ("IAPInterface", "retrieveProductsInfo") => {
                let ids = string_args(args);
                self.platform.iap.retrieve_products(&ids);
                // Report every product as available for 0.0 ("Free"), which is what
                // the StoreKit sandbox would return for a zero-priced product.
                let count = ids.len();
                let id_values = ids.into_iter().map(Value::String).collect();
                let prices = (0..count).map(|_| Value::Number(0.0)).collect();
                let localised = (0..count).map(|_| Value::String("Free".into())).collect();
                self.post(
                    "IAPInterfaceOnProductInfoReceived",
                    vec![
                        Value::List(id_values),
                        Value::List(prices),
                        Value::List(localised),
                    ],
                );
                Value::Null
            }
            ("IAPInterface", "purchaseItem") | ("IAPInterface", "requestPayment") => {
                let id = string_arg(args, 0);
                self.platform.iap.purchase(&id);
                // Free purchase: tell AS3 the transaction already completed.
                tracing::info!(%id, "iap: granting purchase for free");
                self.post(
                    "IAPInterfaceOnPaymentCompleted",
                    vec![Value::String(id), Value::Bool(true)],
                );
                Value::Null
            }
            ("IAPInterface", "restoreCompletedPurchases") => {
                self.platform.iap.restore_completed_purchases();
                tracing::info!("iap: reporting restoration as complete");
                self.post(
                    "IAPInterfaceOnRestorationCompleted",
                    vec![Value::Bool(true)],
                );
                Value::Null
            }

            _ => {
                tracing::warn!(%name, ?args, "unhandled ExternalInterface call");
                Value::Null
            }
        }
    }
}

impl ExternalInterfaceProvider for Bridge {
    fn call_method(&self, _context: &mut UpdateContext<'_>, name: &str, args: &[Value]) -> Value {
        self.dispatch(name, args)
    }

    fn on_callback_available(&self, name: &str) {
        tracing::debug!(%name, "AS3 callback registered");
    }

    fn get_id(&self) -> Option<String> {
        Some("tiny-thief-rust-host".to_string())
    }
}

impl FsCommandProvider for Bridge {
    fn on_fs_command(&self, command: &str, args: &str) -> bool {
        tracing::info!(%command, %args, "fscommand");
        false
    }
}

/// Flatten nested `Value::List`s into strings (AS3 passes a Vector/Array of ids).
fn string_args(args: &[Value]) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Number(n) => out.push(n.to_string()),
            Value::List(items) => items.iter().for_each(|i| walk(i, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    args.iter().for_each(|a| walk(a, &mut out));
    out
}

/// Flatten an AS3 metadata object into (key, value) string pairs.
fn object_pairs(v: Option<&Value>) -> Vec<(String, String)> {
    fn flat(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null | Value::Undefined => String::new(),
            other => format!("{other:?}"),
        }
    }
    match v {
        Some(Value::Object(map)) => map.iter().map(|(k, v)| (k.clone(), flat(v))).collect(),
        _ => Vec::new(),
    }
}

fn string_arg(args: &[Value], index: usize) -> String {
    match args.get(index) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn number_arg(args: &[Value], index: usize) -> Option<f64> {
    match args.get(index) {
        Some(Value::Number(n)) => Some(*n),
        Some(Value::String(s)) => s.parse().ok(),
        Some(Value::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}
