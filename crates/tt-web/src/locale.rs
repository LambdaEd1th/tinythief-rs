//! JS-backed locale/time backend.
//!
//! Ruffle's `DefaultLocaleBackend` calls `chrono::Utc::now()`, which on
//! `wasm32-unknown-unknown` goes through `std::time::SystemTime` and panics
//! ("time not implemented on this platform"). The browser has the clock, so we
//! read it from `Date` instead.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use ruffle_core::backend::locale::LocaleBackend;

pub struct WebLocale;

impl LocaleBackend for WebLocale {
    fn get_current_date_time(&self) -> DateTime<Utc> {
        let millis = js_sys::Date::now() as i64;
        Utc.timestamp_millis_opt(millis)
            .single()
            .unwrap_or_else(Utc::now)
    }

    fn get_timezone(&self) -> FixedOffset {
        // `getTimezoneOffset` is minutes *behind* UTC (e.g. UTC+8 -> -480).
        let minutes_east = -(js_sys::Date::new_0().get_timezone_offset() as i32);
        FixedOffset::east_opt(minutes_east * 60)
            .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC offset"))
    }
}
