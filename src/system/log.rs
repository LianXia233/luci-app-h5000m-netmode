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
    // `[netmode] <ts> LEVEL <module>: <message>`.
    //
    // The `[netmode]` prefix is the contract that makes `logread | grep
    // netmode` usable on the device: stderr of the procd watchdog and of the
    // hotplug helper is forwarded into syslog, so every switch phase, probe
    // round and rollback lands there. The module tag is kept (dropping it
    // cost us the only signal that separated route surgery from health
    // probes); stderr keeps the LuCI `fs.exec` stdout contract untouched.
    eprintln!("[netmode] [{}] {} {}: {}", unix_ts(), level, module, msg);
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
