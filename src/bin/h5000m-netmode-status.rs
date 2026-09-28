//! `/usr/sbin/h5000m-netmode-status`: the thin, fast front end the LuCI page
//! polls. Ported verbatim from the shell version (same cache file, same mark
//! semantics, same task-line rebuild):
//!   * a running switch is served from the snapshot with the worker's task
//!     lines rebuilt from the state file (one small read, live to the second);
//!   * everything else is served from a `status_cache`-second snapshot;
//!   * the full `status` pass is only paid when the snapshot is stale or
//!     its `gen:applied_mode` mark no longer matches the state file.

use std::fs;
use std::path::Path;

use h5000m_netmode::state;
use h5000m_netmode::system::clock::unix_ts;

const BACKEND: &str = "/usr/sbin/h5000m-netmode";
const CACHE: &str = "/var/run/h5000m-netmode-status.cache";

fn ttl() -> u32 {
    let v = h5000m_netmode::config::uci::uci_get("h5000m_netmode.settings.status_cache");
    v.parse::<u32>().unwrap_or(3)
}

fn state_mark() -> String {
    format!(
        "{}:{}",
        state::state_get("gen"),
        state::state_get("applied_mode")
    )
}

fn snapshot_mark() -> String {
    let Ok(text) = fs::read_to_string(CACHE) else {
        return String::new();
    };
    text.lines()
        .find_map(|l| l.strip_prefix("cache_mark=").map(|v| v.to_string()))
        .unwrap_or_default()
}

fn emit_task() {
    let map = state::kv_read(&state::state_file());
    let get = |k: &str| map.get(k).cloned().unwrap_or_default();
    let now = unix_ts();
    let started: u64 = get("started").parse().unwrap_or(0);
    let state_val = get("state");

    println!(
        "switch_state={}",
        if state_val.is_empty() {
            "IDLE"
        } else {
            &state_val
        }
    );
    println!("switch_kind={}", get("kind"));
    println!("switch_target={}", get("target"));
    println!("switch_target_mode={}", get("target_mode"));
    println!("switch_result={}", get("result"));
    println!("switch_reason={}", get("reason"));
    println!("switch_message={}", get("message"));
    println!("switch_elapsed={}", get("elapsed"));
    println!("switch_phases={}", get("phases"));
    println!("switch_gen={}", get("gen"));
    println!("requested_mode={}", get("requested_mode"));
    println!("last_align_reason={}", get("last_align_reason"));
    println!("last_align_target={}", get("last_align_target"));
    println!("switch_started={started}");
    if now > 0 && started > 0 {
        println!("switch_age={}", now.saturating_sub(started));
    } else {
        println!("switch_age=0");
    }
    let pid: u32 = get("pid").parse().unwrap_or(0);
    if pid > 0 && state::proc_alive(pid) {
        println!("switch_busy=1");
    } else {
        println!("switch_busy=0");
    }
}

fn serve_cached() {
    let Ok(text) = fs::read_to_string(CACHE) else {
        return;
    };
    for line in text.lines() {
        let key = line.split('=').next().unwrap_or("");
        let stripped = matches!(
            key,
            "switch_state"
                | "switch_kind"
                | "switch_target"
                | "switch_target_mode"
                | "switch_result"
                | "switch_reason"
                | "switch_message"
                | "switch_elapsed"
                | "switch_phases"
                | "switch_gen"
                | "switch_started"
                | "switch_age"
                | "switch_busy"
                | "requested_mode"
                | "last_align_reason"
                | "last_align_target"
                | "cache_mark"
        );
        if !stripped {
            println!("{line}");
        }
    }
    emit_task();
}

/// Run the backend once and replace the snapshot. Falls back to streaming the
/// backend output when it produced nothing (never poison the cache window).
fn cold_path() {
    let out = std::process::Command::new(BACKEND).arg("status").output();
    let Ok(out) = out else {
        return;
    };
    if out.status.success() && !out.stdout.is_empty() {
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        print!("{text}");
        text.push_str(&format!("cache_mark={}\n", state_mark()));
        let tmp = format!("{CACHE}.{}", std::process::id());
        if fs::write(&tmp, &text).is_ok() {
            let _ = fs::rename(&tmp, CACHE);
        }
    } else {
        // The backend failed: serve its raw answer (empty included) rather than
        // a stale snapshot.
        let _ = std::io::Write::write_all(&mut std::io::stdout(), &out.stdout);
    }
}

fn main() {
    let ttl = ttl();
    if ttl == 0 {
        // Caching disabled: always ask the backend.
        let status = std::process::Command::new(BACKEND).arg("status").status();
        let _ = status;
        return;
    }

    let mut serve = false;
    if Path::new(CACHE).exists() && snapshot_mark() == state_mark() {
        let pid: u32 = state::state_get("pid").parse().unwrap_or(0);
        let busy = pid > 0 && state::proc_alive(pid);
        if busy {
            serve = true;
        } else {
            let now = unix_ts();
            let mt = fs::metadata(CACHE)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if now > 0 && mt > 0 && now.saturating_sub(mt) < ttl as u64 {
                serve = true;
            }
        }
    }

    if serve {
        serve_cached();
        return;
    }
    cold_path();
}
