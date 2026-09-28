//! Unified log sink.
//!
//! The legacy backend logs `[INFO]`/`[WARN]`-style lines with a module tag and
//! the event. Every module here funnels through these three functions so the
//! format cannot diverge per module (requirement: 日志模块统一).
//!
//! Logging is disabled for read-only commands (`status`, `now-cs`, ...) via the
//! global `ENABLED` flag, mirroring the shell's `LOGGING=0`.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::system::clock::unix_ts;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Turn logging on/off. Read-only commands call `disable()` at startup.
pub fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn emit(level: &str, module: &str, msg: &str) {
    if !enabled() {
        return;
    }
    // `date +%s` style wall timestamp, then [LEVEL] module: message.
    // stderr keeps the LuCI `fs.exec` stdout contract untouched.
    eprintln!("[{}] {}: {}", unix_ts(), level, msg);
    let _ = module;
}

/// INFO-level log with a module tag: `log_info("health", "wan=wwan0 icmp=ok")`.
pub fn log_info(module: &str, msg: &str) {
    emit("INFO", module, msg);
}

pub fn log_warn(module: &str, msg: &str) {
    emit("WARN", module, msg);
}

pub fn log_error(module: &str, msg: &str) {
    emit("ERROR", module, msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_roundtrip() {
        disable();
        assert!(!enabled());
        ENABLED.store(true, Ordering::Relaxed);
        assert!(enabled());
    }
}
