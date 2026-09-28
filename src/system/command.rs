//! Bounded external-command wrapper.
//!
//! The Rust core performs all reads in-process; a small set of OpenWrt-native
//! operations still requires its own binaries and is wrapped here with a hard
//! timeout so a wedged daemon can never hang a switch or an RPC:
//!   * `ubus` – network.interface.<sec> status (per-section up/available
//!     state), wireless status (status output only);
//!   * `ifup` – one bounded retry inside `wait_group_family`;
//!   * `uci`  – mutations only (see config/uci.rs);
//!   * `mt5700m-manager sync` – post-commit vendor integration.
//!
//! No business module ever invokes a subprocess directly.

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::types::{Error, Result};

/// Run a command with a hard timeout. Returns stdout on rc 0, Err otherwise.
pub fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::system(format!("spawn {program}: {e}")))?;

    // Make the stdout pipe non-blocking so the read loop below can respect the
    // deadline without a separate reader thread.
    if let Some(so) = child.stdout.as_ref() {
        use std::os::fd::AsRawFd;
        let fd = so.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags >= 0 {
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        }
    }

    let deadline = std::time::Instant::now() + timeout;
    let mut stdout = Vec::new();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    let _ = child.stdout.take();
                    return Ok(String::from_utf8_lossy(&stdout).into_owned());
                }
                return Err(Error::system(format!(
                    "{program} rc={}",
                    status.code().unwrap_or(-1)
                )));
            }
            Ok(None) => {}
            Err(e) => return Err(Error::system(format!("wait {program}: {e}"))),
        }
        // Read whatever is available without blocking forever.
        if let Some(so) = child.stdout.as_mut() {
            use std::io::Read;
            let mut buf = [0u8; 4096];
            match so.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => stdout.extend_from_slice(&buf[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(Error::system(format!("read {program}: {e}"))),
            }
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::system(format!(
                "{program} timed out after {}s",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Run with the default 5s bound.
pub fn run_bounded_default(program: &str, args: &[&str]) -> Result<String> {
    run_bounded(program, args, Duration::from_secs(5))
}

/// `ubus call network.interface.<sec> status` with a 1s connect-timeout hint.
/// Returns None on any failure (the caller decides the fallback).
pub fn ubus_section_status(sec: &str) -> Option<String> {
    run_bounded_default(
        "ubus",
        &["call", &format!("network.interface.{sec}"), "status"],
    )
    .ok()
}

/// `ubus call network.wireless status`.
pub fn ubus_wireless_status() -> Option<String> {
    run_bounded_default("ubus", &["call", "network.wireless", "status"]).ok()
}

/// `ubus call iwinfo assoclist {"device": "<dev>"}`.
pub fn ubus_iwinfo_assoclist(dev: &str) -> Option<String> {
    run_bounded_default(
        "ubus",
        &[
            "call",
            "iwinfo",
            "assoclist",
            &format!("{{\"device\":\"{dev}\"}}"),
        ],
    )
    .ok()
}

/// `ifup <sec>` – bounded; never blocks a switch beyond the timeout.
pub fn ifup_section(sec: &str) -> Result<()> {
    run_bounded_default("ifup", &[sec]).map(|_| ())
}

/// `mt5700m-manager sync` (vendor integration after a committed switch).
pub fn vendor_sync() {
    if !std::path::Path::new("/usr/sbin/mt5700m-manager").exists() {
        return;
    }
    match run_bounded_default("mt5700m-manager", &["sync"]) {
        Ok(_) => {}
        Err(e) => {
            crate::system::log::log_warn("vendor", &format!("mt5700m-manager sync failed: {e}"))
        }
    }
}
