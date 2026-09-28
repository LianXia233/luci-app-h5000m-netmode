//! Key=value state files and the writer lock.
//!
//! The legacy backend keeps two state files (the switch task state and the
//! health cache) plus a lock directory. The Rust backend preserves those exact
//! files and semantics:
//!   * `/var/run/h5000m-netmode.state`  – switch task state machine fields;
//!   * `/var/run/h5000m-netmode.health` – probe verdicts and failure streaks;
//!   * `/var/run/h5000m-netmode.lock/`   – mkdir-based writer lock.
//!
//! `kv_write` replaces only the keys it names, so a worker's phase fields
//! survive the next update (a write must never drop keys it does not mention).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::types::Result;

pub fn state_file() -> PathBuf {
    PathBuf::from(
        std::env::var("H5000M_STATE_FILE")
            .unwrap_or_else(|_| "/var/run/h5000m-netmode.state".into()),
    )
}

pub fn health_file() -> PathBuf {
    PathBuf::from(
        std::env::var("H5000M_HEALTH_STATE")
            .unwrap_or_else(|_| "/var/run/h5000m-netmode.health".into()),
    )
}

pub fn lock_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("H5000M_LOCK_DIR").unwrap_or_else(|_| "/var/run/h5000m-netmode.lock".into()),
    )
}

/// Read a whole kv file into a map (first value wins per key).
pub fn kv_read(path: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return map;
    };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if is_valid_key(k) && !map.contains_key(k) {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    map
}

/// `state_get <key>` from the state file.
pub fn state_get(key: &str) -> String {
    kv_read(&state_file()).get(key).cloned().unwrap_or_default()
}

/// `state_get` from the health cache.
pub fn health_get(key: &str) -> String {
    kv_read(&health_file())
        .get(key)
        .cloned()
        .unwrap_or_default()
}

fn is_valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Sanitize a value: tabs/newlines are replaced with spaces (the shell's
/// `sanitize_value`).
fn sanitize_value(v: &str) -> String {
    if v.contains('\n') || v.contains('\r') || v.contains('\t') {
        v.chars()
            .map(|c| match c {
                '\n' | '\r' | '\t' => ' ',
                _ => c,
            })
            .collect()
    } else {
        v.to_string()
    }
}

/// `kv_write <file> <key=value> ...`: rewrite the file, dropping the named
/// keys and appending the new pairs. Atomic via write-to-temp + rename.
pub fn kv_write(path: &Path, pairs: &[(&str, &str)]) -> Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let mut skip: Vec<String> = Vec::new();
    for (k, _) in pairs {
        if !is_valid_key(k) {
            return Err(crate::types::Error::system(format!(
                "invalid state key: {k}"
            )));
        }
        skip.push(k.to_string());
    }
    let old = kv_read(path);
    let mut out = String::new();
    for (k, v) in &old {
        if skip.iter().any(|s| s == k) {
            continue;
        }
        out.push_str(&format!("{k}={v}\n"));
    }
    for (k, v) in pairs {
        out.push_str(&format!("{k}={}\n", sanitize_value(v)));
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp).map_err(crate::types::error::io_err)?;
        f.write_all(out.as_bytes())
            .map_err(crate::types::error::io_err)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).map_err(crate::types::error::io_err)?;
    Ok(())
}

/// `state_write <key=value> ...`.
pub fn state_write(pairs: &[(&str, &str)]) -> Result<()> {
    kv_write(&state_file(), pairs)
}

/// `health_write <key=value> ...`.
pub fn health_write(pairs: &[(&str, &str)]) -> Result<()> {
    kv_write(&health_file(), pairs)
}

/// Ensure the state directory exists (mirrors `state_dir()`).
pub fn state_dir() {
    if let Some(dir) = state_file().parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Some(dir) = health_file().parent() {
        let _ = fs::create_dir_all(dir);
    }
}

// ---------------------------------------------------------------------------
// writer lock
// ---------------------------------------------------------------------------

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn lock_owner_alive(dir: &Path) -> bool {
    let Ok(pid_text) = fs::read_to_string(dir.join("pid")) else {
        return false;
    };
    let pid: u32 = match pid_text.trim().parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    pid_alive(pid)
}

/// `acquire_lock <wait_seconds>`: mkdir-based lock with a bounded wait.
/// Returns true when the lock was acquired (with our pid written).
pub fn acquire_lock(wait_s: u32) -> bool {
    let dir = lock_dir();
    let mine = std::process::id().to_string();
    let mut waited: u32 = 0;
    let mut stale_rounds: u32 = 0;
    loop {
        match fs::create_dir(&dir) {
            Ok(()) => {
                let _ = fs::write(dir.join("pid"), format!("{mine}\n"));
                // Two processes can both conclude the owner is stale and both
                // unlink the directory, and then both succeed at create_dir -
                // which is two holders for one lock. Confirm the directory
                // still names us before claiming it.
                let still_mine = fs::read_to_string(dir.join("pid"))
                    .map(|t| t.trim() == mine)
                    .unwrap_or(false);
                if still_mine {
                    return true;
                }
                continue;
            }
            Err(_) => {
                if !lock_owner_alive(&dir) && stale_rounds < 3 {
                    // Stale owner: remove and retry immediately.
                    stale_rounds += 1;
                    let _ = fs::remove_dir_all(&dir);
                    continue;
                }
                if waited >= wait_s {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
                waited += 1;
            }
        }
    }
}

/// `release_lock`: best effort.
pub fn release_lock() {
    let _ = fs::remove_dir_all(lock_dir());
}

/// `/proc/<pid>` liveness (the `kill -0` / `[ -d /proc/$pid ]` oracle).
pub fn proc_alive(pid: u32) -> bool {
    pid_alive(pid)
}

/// `switch_state`: the current task state name.
pub fn switch_state() -> String {
    let st = state_get("state");
    if st.is_empty() {
        "IDLE".to_string()
    } else {
        st
    }
}

/// `switch_in_progress`: a non-terminal state owned by a live pid.
pub fn switch_in_progress() -> bool {
    let st = crate::types::SmState::parse(&state_get("state"));
    if st.is_terminal() {
        return false;
    }
    if st == crate::types::SmState::Idle {
        return false;
    }
    let pid: u32 = crate::types::num_or(&state_get("pid"), 0);
    pid > 0 && pid_alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_write_merges() {
        let dir = std::env::temp_dir();
        let p = dir.join(format!("h5state_test_{}", std::process::id()));
        let _ = fs::remove_file(&p);
        kv_write(&p, &[("a", "1"), ("b", "2")]).unwrap();
        kv_write(&p, &[("a", "3"), ("c", "4")]).unwrap();
        let m = kv_read(&p);
        assert_eq!(m["a"], "3");
        assert_eq!(m["b"], "2");
        assert_eq!(m["c"], "4");
        fs::remove_file(&p).ok();
    }

    #[test]
    fn sanitize_tabs() {
        let dir = std::env::temp_dir();
        let p = dir.join(format!("h5state_tab_{}", std::process::id()));
        let _ = fs::remove_file(&p);
        kv_write(&p, &[("msg", "a\tb\nc")]).unwrap();
        let m = kv_read(&p);
        assert_eq!(m["msg"], "a b c");
        fs::remove_file(&p).ok();
    }

    #[test]
    fn lock_exclusion() {
        let dir = std::env::temp_dir();
        std::env::set_var(
            "H5000M_LOCK_DIR",
            dir.join(format!("h5lock_{}", std::process::id())),
        );
        let _ = fs::remove_dir_all(lock_dir());
        assert!(acquire_lock(0));
        assert!(!acquire_lock(0)); // second caller blocked
        release_lock();
        assert!(acquire_lock(0));
        release_lock();
    }
}
