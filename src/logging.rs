//! Minimal rotating file logger.
//!
//! Secrets cannot reach it through the type system ([`crate::auth::SecretBytes`] has no
//! Display and a redacted Debug). As defence in depth every line is also passed through
//! [`redact`], which masks anything shaped like a JWT or API key, and newlines are
//! neutralised to prevent log forging.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::LogLevel;

const MAX_LOG_BYTES: u64 = 1024 * 1024;

struct Logger {
    path: PathBuf,
    level: AtomicU8,
    write_lock: Mutex<()>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

pub fn init(path: PathBuf, level: LogLevel) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = LOGGER.set(Logger { path, level: AtomicU8::new(level as u8), write_lock: Mutex::new(()) });
}

pub fn set_level(level: LogLevel) {
    if let Some(l) = LOGGER.get() {
        l.level.store(level as u8, Ordering::Relaxed);
    }
}

pub fn write(level: LogLevel, args: fmt::Arguments<'_>) {
    let Some(logger) = LOGGER.get() else { return };
    if level as u8 > logger.level.load(Ordering::Relaxed) {
        return;
    }
    let _guard = logger.write_lock.lock().unwrap_or_else(|p| p.into_inner());
    if fs::metadata(&logger.path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = fs::rename(&logger.path, logger.path.with_extension("log.1"));
    }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("{} {:<5} {}\r\n", timestamp(secs), level.name().to_ascii_uppercase(), redact(&args.to_string()));
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&logger.path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! log_error { ($($a:tt)*) => { $crate::logging::write($crate::config::LogLevel::Error, format_args!($($a)*)) }; }
#[macro_export]
macro_rules! log_warn { ($($a:tt)*) => { $crate::logging::write($crate::config::LogLevel::Warn, format_args!($($a)*)) }; }
#[macro_export]
macro_rules! log_info { ($($a:tt)*) => { $crate::logging::write($crate::config::LogLevel::Info, format_args!($($a)*)) }; }
#[macro_export]
macro_rules! log_debug { ($($a:tt)*) => { $crate::logging::write($crate::config::LogLevel::Debug, format_args!($($a)*)) }; }

/// Masks token-shaped words and flattens control characters.
pub fn redact(message: &str) -> String {
    let flat: String = message.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut out = String::with_capacity(flat.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let looks_secret = (word.starts_with("eyJ") && word.len() >= 20)
            || ((word.starts_with("sk-") || word.starts_with("rt_") || word.starts_with("at_")) && word.len() >= 16);
        out.push_str(if looks_secret { "[redacted]" } else { word });
        word.clear();
    };
    for c in flat.chars() {
        if c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ':' | '=' | '(' | ')' | '{' | '}' | '[' | ']') {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// `YYYY-MM-DDTHH:MM:SSZ` from Unix seconds (civil-from-days, H. Hinnant).
fn timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_token_shapes_and_newlines() {
        let msg = "failed {\"id_token\":\"eyJhbGciOiJub25lIn0.eyJzdWIiOiIxIn0.sig\"} key=sk-proj-abcdefghijklmnop\nFORGED";
        let r = redact(msg);
        assert!(!r.contains("eyJhbGci"));
        assert!(!r.contains("sk-proj"));
        assert!(!r.contains('\n'));
        assert!(r.contains("FORGED"));
        assert_eq!(redact("profile 3f2a switched"), "profile 3f2a switched");
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(1_789_000_000), "2026-09-10T00:26:40Z");
    }
}
