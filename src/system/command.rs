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

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
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
    run_bounded_child(program, &mut child, timeout)
}

/// `run_bounded` with the given bytes on the child's stdin (used by
/// `uci batch`, which reads its commands one per line from stdin).
pub fn run_bounded_stdin(
    program: &str,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::system(format!("spawn {program}: {e}")))?;
    if let Some(mut si) = child.stdin.take() {
        // A wedged child that never reads must not park the caller: write in
        // non-blocking fashion is overkill for a <1 KiB script (it fits the
        // pipe buffer), so a plain write + drop is bounded by the pipe size.
        let _ = si.write_all(input);
    }
    run_bounded_child(program, &mut child, timeout)
}

fn run_bounded_child(
    program: &str,
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<String> {
    // Make the stdout pipe non-blocking so the read loop below can respect the
    // deadline without a separate reader thread.
    if let Some(so) = child.stdout.as_ref() {
        use std::os::fd::AsRawFd;
        let fd = so.as_raw_fd();
        set_nonblocking(fd, true);
    }

    let deadline = std::time::Instant::now() + timeout;
    let mut stdout = Vec::new();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The child is gone but its output can still sit in the pipe
                // buffer: judging the result before draining it loses the tail
                // of a fast command (a ubus status, say) and makes callers see
                // an empty answer for no reason. Switch the pipe back to
                // blocking - the write end is closed now, so this cannot hang.
                if let Some(mut so) = child.stdout.take() {
                    use std::io::Read;
                    use std::os::fd::AsRawFd;
                    set_nonblocking(so.as_raw_fd(), false);
                    let mut rest = Vec::new();
                    // Whatever the read reports, `rest` holds what was still
                    // in the pipe, and that is all this wrapper cares about.
                    let _ = so.read_to_end(&mut rest);
                    stdout.extend_from_slice(&rest);
                }
                if status.success() {
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

// ---------------------------------------------------------------------------
// ubus answer memoisation (one snapshot = one query per section)
// ---------------------------------------------------------------------------

type UbusCache = BTreeMap<String, Option<String>>;

fn ubus_cache() -> &'static Mutex<UbusCache> {
    static CACHE: std::sync::OnceLock<Mutex<UbusCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Drop the memoised ubus answers. `read_live_state` calls this once at the
/// start of every snapshot: within one snapshot the same section is queried
/// once (a snapshot build + the status DNS pass used to fork `ubus` six
/// times for at most four distinct sections).
pub fn invalidate_ubus_cache() {
    match ubus_cache().lock() {
        Ok(mut g) => g.clear(),
        Err(e) => e.into_inner().clear(),
    }
}

/// Flip O_NONBLOCK on a pipe fd.
fn set_nonblocking(fd: i32, on: bool) {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return;
    }
    let next = if on {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    unsafe {
        libc::fcntl(fd, libc::F_SETFL, next);
    }
}

/// Escape a string for embedding in a JSON literal.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `run_bounded` with the default 5s bound.
pub fn run_bounded_default(program: &str, args: &[&str]) -> Result<String> {
    run_bounded(program, args, Duration::from_secs(5))
}

/// The `ubus` binary (test seam `H5000M_UBUS_BIN`, default `/usr/bin/ubus`).
fn ubus_bin() -> String {
    std::env::var("H5000M_UBUS_BIN").unwrap_or_else(|_| "/usr/bin/ubus".into())
}

/// The `ifup` binary (test seam `H5000M_IFUP_BIN`, default `/sbin/ifup`).
fn ifup_bin() -> String {
    std::env::var("H5000M_IFUP_BIN").unwrap_or_else(|_| "/sbin/ifup".into())
}

/// `ubus call network.interface.<sec> status` with a 1s connect-timeout hint.
/// Returns None on any failure (the caller decides the fallback). Memoised
/// for the lifetime of one snapshot (see `invalidate_ubus_cache`).
pub fn ubus_section_status(sec: &str) -> Option<String> {
    if let Ok(map) = ubus_cache().lock() {
        if let Some(v) = map.get(sec) {
            return v.clone();
        }
    }
    let v = run_bounded_default(
        &ubus_bin(),
        &["call", &format!("network.interface.{sec}"), "status"],
    )
    .ok();
    if let Ok(mut map) = ubus_cache().lock() {
        map.insert(sec.to_string(), v.clone());
    }
    v
}

/// `ubus call network.wireless status`.
pub fn ubus_wireless_status() -> Option<String> {
    run_bounded_default(&ubus_bin(), &["call", "network.wireless", "status"]).ok()
}

/// `ubus call iwinfo assoclist {"device": "<dev>"}`.
pub fn ubus_iwinfo_assoclist(dev: &str) -> Option<String> {
    let payload = format!("{{\"device\":\"{}\"}}", json_escape(dev));
    run_bounded_default(&ubus_bin(), &["call", "iwinfo", "assoclist", &payload]).ok()
}

/// `ifup <sec>` – bounded; never blocks a switch beyond the timeout.
pub fn ifup_section(sec: &str) -> Result<()> {
    run_bounded_default(&ifup_bin(), &[sec]).map(|_| ())
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
