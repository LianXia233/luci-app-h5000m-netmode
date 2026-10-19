//! UCI access.
//!
//! **Read path**: `/etc/config/*` files are parsed in-process (a plain
//! line-oriented format), which removes the `uci show`/`uci get` fork+exec
//! storm the shell backend paid on every status poll. One `read_live_state`
//! pass used to re-parse the network config twenty-plus times (every
//! `uci_get`/`uci_has_key` is a full parse); the parsed documents are now
//! memoised per path and validated against the file's `(mtime, size)` so any
//! external writer (netifd, modemd, another `uci` invocation) invalidates the
//! entry on its next read.
//!
//! **Write path**: mutations (`uci set`/`uci commit`) still go through
//! `/sbin/uci`. Reasons, unlike the read path:
//! * UCI writes must honour the UCI file lock and fire `config_change`
//!   notifications so netifd/procd re-read the touched configs;
//! * value quoting/escaping is a correctness trap, and re-implementing it
//!   buys nothing on a low-frequency path (a switch commits a handful of
//!   writes; `set-device-map`/`eth-fallback` are user actions).
//!
//! Multiple writes of one transaction are sent as ONE `uci batch` invocation
//! instead of one fork per option; on any batch error the caller falls back
//! to per-line `uci_exec`.
//!
//! Those calls are bounded (a fixed short timeout) and wrapped here so no
//! business module shells out directly.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use crate::types::Result;

pub const H5000M_CFG: &str = "/etc/config/h5000m_netmode";
pub const NETWORK_CFG: &str = "/etc/config/network";
pub const MT5700M_CFG: &str = "/etc/config/mt5700m";

/// Config paths, overridable for the deterministic test suite (`H5000M_CFG_FILE`,
/// `H5000M_NETWORK_CFG`, `H5000M_MT5700M_CFG`).
pub fn h5000m_cfg() -> String {
    std::env::var("H5000M_CFG_FILE").unwrap_or_else(|_| H5000M_CFG.to_string())
}

pub fn network_cfg() -> String {
    std::env::var("H5000M_NETWORK_CFG").unwrap_or_else(|_| NETWORK_CFG.to_string())
}

pub fn mt5700m_cfg() -> String {
    std::env::var("H5000M_MT5700M_CFG").unwrap_or_else(|_| MT5700M_CFG.to_string())
}

/// A parsed UCI config: section name -> (options, lists).
pub type UciDoc = BTreeMap<String, (BTreeMap<String, String>, BTreeMap<String, Vec<String>>)>;

// ---------------------------------------------------------------------------
// parse cache (mtime + size validated)
// ---------------------------------------------------------------------------

type CacheEntry = (std::time::SystemTime, u64, UciDoc);

fn cache() -> &'static Mutex<BTreeMap<String, CacheEntry>> {
    static CACHE: std::sync::OnceLock<Mutex<BTreeMap<String, CacheEntry>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn lock_cache() -> std::sync::MutexGuard<'static, BTreeMap<String, CacheEntry>> {
    match cache().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(), // poisoned: reuse the map, reads are read-only
    }
}

/// Drop every cached document. Called after our own `uci` writes so the next
/// read observes them even when the filesystem keeps a coarse timestamp.
pub fn invalidate_cache() {
    lock_cache().clear();
}

fn file_stamp(path: &str) -> Option<(std::time::SystemTime, u64)> {
    let md = fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?;
    Some((mtime, md.len()))
}

/// Parse a UCI config file into (section -> {option -> value}).
///
/// Handles `config <type> ['<name>']` headers, `option <k> '<v>'`,
/// `list <k> '<v>'`, `#`/`//` comments and single/double quotes.
/// `config <type>` with no name gets a generated name, matching UCI's own
/// numbering only loosely; the sections this program uses are always named.
pub fn parse_uci(path: &str) -> UciDoc {
    if let Some((mtime, size, doc)) = lock_cache().get(path) {
        if file_stamp(path)
            .map(|s| s == (*mtime, *size))
            .unwrap_or(false)
        {
            return doc.clone();
        }
    }
    let doc = parse_uci_uncached(path);
    if let Some(stamp) = file_stamp(path) {
        lock_cache().insert(path.to_string(), (stamp.0, stamp.1, doc.clone()));
    }
    doc
}

fn parse_uci_uncached(path: &str) -> UciDoc {
    let mut doc: UciDoc = BTreeMap::new();
    let mut cur: Option<String> = None;
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return doc,
    };
    let mut anon = 0usize;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let stripped = strip_comment(line);
        if stripped.is_empty() {
            continue;
        }
        let toks = tokenize(&stripped);
        if toks.is_empty() {
            continue;
        }
        match toks[0].as_str() {
            "config" => {
                // config <type> [<name>]
                let name = if toks.len() >= 3 {
                    toks[2].clone()
                } else {
                    anon += 1;
                    format!("__anon{}", anon)
                };
                doc.entry(name.clone()).or_default();
                cur = Some(name);
            }
            "option" | "list" if toks.len() >= 3 => {
                if let Some(name) = &cur {
                    let entry = doc.entry(name.clone()).or_default();
                    if toks[0] == "option" {
                        entry.0.insert(toks[1].clone(), toks[2].clone());
                    } else {
                        entry
                            .1
                            .entry(toks[1].clone())
                            .or_default()
                            .push(toks[2].clone());
                    }
                }
            }
            _ => {}
        }
    }
    doc
}

/// Strip a UCI comment (a `#` not inside quotes).
fn strip_comment(line: &str) -> String {
    let mut in_s = false;
    let mut in_d = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '\'' if !in_d => in_s = !in_s,
            '"' if !in_s => in_d = !in_d,
            '#' if !in_s && !in_d => return line[..i].to_string(),
            _ => {}
        }
    }
    line.to_string()
}

/// Split a UCI line into words honouring quotes.
fn tokenize(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_s = false;
    let mut in_d = false;
    for ch in line.chars() {
        match ch {
            '\'' if !in_d => {
                in_s = !in_s;
                if !in_s {
                    out.push(std::mem::take(&mut cur));
                }
            }
            '"' if !in_s => {
                in_d = !in_d;
                if !in_d {
                    out.push(std::mem::take(&mut cur));
                }
            }
            ' ' | '\t' if !in_s && !in_d => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Read a single option: `uci_get("network.wan.metric")`.
/// Split a `cfg.section[.option]` key into its three parts; the option is
/// empty for two-segment keys.
///
/// `uci_get` originally used `splitn(3, '.')` here and pulled the *second*
/// component out as the remainder, so for "network.wan.device" the option
/// ended up empty and every three-segment read returned "" on real devices -
/// exactly why wan_device/modem_device stayed blank while section-presence
/// checks (two-segment keys) kept working.
fn split_key(dotted: &str) -> Option<(&str, &str, &str)> {
    let (cfg, rest) = dotted.split_once('.')?;
    let (section, option) = match rest.split_once('.') {
        Some((s, o)) => (s, o),
        None => (rest, ""),
    };
    Some((cfg, section, option))
}

pub fn uci_get(dotted: &str) -> String {
    let Some((cfg, section, option)) = split_key(dotted) else {
        return String::new();
    };
    let path = match cfg {
        "h5000m_netmode" => h5000m_cfg(),
        "network" => network_cfg(),
        "mt5700m" => mt5700m_cfg(),
        _ => {
            // Unknown config: try a file of that name (test seams).
            return String::new();
        }
    };
    let doc = parse_uci(&path);
    doc.get(section)
        .and_then(|(opts, _)| opts.get(option))
        .cloned()
        .unwrap_or_default()
}

/// `uci_has_key("network.wan")` -> section exists.
/// `uci_has_key("network.wan.device")` -> the option exists.
pub fn uci_has_key(dotted: &str) -> bool {
    let Some((cfg, rest)) = dotted.split_once('.') else {
        return false;
    };
    let path = match cfg {
        "h5000m_netmode" => h5000m_cfg(),
        "network" => network_cfg(),
        "mt5700m" => mt5700m_cfg(),
        _ => return false,
    };
    let doc = parse_uci(&path);
    if let Some((section, option)) = rest.split_once('.') {
        doc.get(section)
            .map(|(opts, _)| opts.contains_key(option))
            .unwrap_or(false)
    } else {
        doc.contains_key(rest)
    }
}

/// Section type as written in the `config <type> ['<name>']` header.
///
/// `parse_uci` only stores *options*; the section type lives in the header and
/// is lost there. Role discovery needs it, so this walks the header lines
/// directly (same comment/quote handling as `parse_uci`). Anonymous sections
/// are skipped: netifd requires named interfaces, and `parse_uci`'s synthetic
/// `__anonN` names would not match anything downstream either.
fn section_types(path: &str) -> Vec<(String, String)> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for raw in text.lines() {
        let line = strip_comment(raw.trim());
        if line.is_empty() {
            continue;
        }
        let toks = tokenize(&line);
        if toks.len() >= 2 && toks[0] == "config" && toks.len() >= 3 {
            out.push((toks[2].clone(), toks[1].clone()));
        }
    }
    out
}

/// List every `interface` section in the network config
/// (`all_interface_sections`).
///
/// The type check is against the section *header* (`config interface 'wan'`),
/// not an option: earlier this matched `opts.get("type") == "interface"`,
/// which no standard network file ever satisfies (the `option type 'bridge'`
/// lines belong to anonymous `config device` sections), so this returned an
/// empty list on every real device and the modem group was never discovered.
pub fn interface_sections() -> Vec<String> {
    section_types(&network_cfg())
        .into_iter()
        .filter(|(_, t)| t == "interface")
        .map(|(name, _)| name)
        .collect()
}

/// `section_device_raw`: `device` option, falling back to `ifname`.
pub fn section_device_raw(section: &str) -> String {
    let v = uci_get(&format!("network.{section}.device"));
    if !v.is_empty() {
        return v;
    }
    uci_get(&format!("network.{section}.ifname"))
}

/// Full `settings` section of h5000m_netmode as a map (for `AppConfig`).
pub fn settings_section() -> (BTreeMap<String, String>, BTreeMap<String, Vec<String>>) {
    let doc = parse_uci(&h5000m_cfg());
    doc.get("settings").cloned().unwrap_or_default()
}

/// The `uci` binary (test seam `H5000M_UCI_BIN`, default `/sbin/uci`).
fn uci_bin() -> String {
    std::env::var("H5000M_UCI_BIN").unwrap_or_else(|_| "/sbin/uci".into())
}

/// External `uci` invocation with a hard timeout. Used only for mutations.
/// Returns Ok(()) when the command exited 0.
///
/// The timeout is not cosmetic: `uci commit` takes the UCI file lock, and a
/// writer that never returns would park a switch worker holding the state lock
/// forever. The previous implementation used `output()` with no bound at all,
/// which contradicts what this function promises. Every write also drops the
/// parse cache so our own reads never serve the pre-write document.
pub fn uci_exec(args: &[&str]) -> Result<()> {
    let joined = args.join(" ");
    let rc = crate::system::command::run_bounded(&uci_bin(), args, bounded_timeout())
        .map(|_| ())
        .map_err(|e| crate::types::Error::system(format!("uci {joined} failed: {e}")));
    invalidate_cache();
    rc
}

/// Run several `uci` operations as ONE `uci batch` invocation (one fork
/// instead of one per line), then drop the parse cache. On any failure the
/// caller should fall back to per-line `uci_exec`.
///
/// `lines` are full uci commands WITHOUT the leading `uci`, e.g.
/// `"set network.wan.metric=50"`, `"commit network"`.
pub fn uci_exec_batch(lines: &[String]) -> Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    let script = format!("{}\n", lines.join("\n"));
    let rc = crate::system::command::run_bounded_stdin(
        &uci_bin(),
        &["-q", "batch"],
        script.as_bytes(),
        bounded_timeout(),
    )
    .map(|_| ())
    .map_err(|e| crate::types::Error::system(format!("uci batch failed: {e}")));
    invalidate_cache();
    rc
}

/// `uci -q set <key>=<value>` + `uci -q commit <config>` with bounded timeout.
pub fn uci_set_commit(cfg: &str, key: &str, value: &str) -> Result<()> {
    uci_exec(&["-q", "set", &format!("{key}={value}")])?;
    uci_exec(&["-q", "commit", cfg])
}

/// Is `/sbin/uci` present at all (a missing uci means config cannot be
/// mutated; reads still work from the files).
pub fn uci_present() -> bool {
    Path::new("/sbin/uci").exists()
}

/// Bounded wait helper for external commands: seconds cap.
pub fn bounded_timeout() -> Duration {
    Duration::from_secs(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_key_segments() {
        // Regression: splitn(3) used to swallow the option on three-segment
        // keys, making every uci_get("cfg.section.option") return "".
        assert_eq!(
            split_key("network.wan.device"),
            Some(("network", "wan", "device"))
        );
        assert_eq!(split_key("network.wan"), Some(("network", "wan", "")));
        assert_eq!(split_key("network"), None);
        assert_eq!(split_key(""), None);
    }

    #[test]
    fn tokenize_quotes() {
        assert_eq!(
            tokenize("config settings 'settings'"),
            vec!["config", "settings", "settings"]
        );
        assert_eq!(
            tokenize("option probe_targets '223.5.5.5 119.29.29.29'"),
            vec!["option", "probe_targets", "223.5.5.5 119.29.29.29"]
        );
        assert_eq!(
            tokenize("option mode wan_first"),
            vec!["option", "mode", "wan_first"]
        );
        assert_eq!(tokenize("option x \"a b\""), vec!["option", "x", "a b"]);
    }

    #[test]
    fn strip_comments() {
        assert_eq!(strip_comment("option a '1' # comment"), "option a '1' ");
        assert_eq!(
            strip_comment("option a '#notcomment'"),
            "option a '#notcomment'"
        );
    }

    #[test]
    fn parse_uci_doc() {
        let dir = std::env::temp_dir();
        let p = dir.join("h5_test_uci.cfg");
        fs::write(
            &p,
            "config settings 'settings'\n\toption mode 'modem_first'\n\tlist probe_targets '223.5.5.5'\n\tlist probe_targets '119.29.29.29'\n",
        )
        .unwrap();
        let doc = parse_uci(p.to_str().unwrap());
        let (opts, lists) = &doc["settings"];
        assert_eq!(opts["mode"], "modem_first");
        assert_eq!(lists["probe_targets"], vec!["223.5.5.5", "119.29.29.29"]);
        fs::remove_file(&p).ok();
    }

    #[test]
    fn section_types_skips_devices_and_anonymous() {
        let dir = std::env::temp_dir();
        let p = dir.join("h5_test_sections.cfg");
        // Mirrors a real OpenWrt network file: anonymous `config device`
        // sections (one carrying `option type 'bridge'`) interleaved with
        // named `config interface` sections.
        fs::write(
            &p,
            "config interface 'loopback'\n\toption device 'lo'\n\n\
             config device\n\toption name 'br-lan'\n\toption type 'bridge'\n\tlist ports 'eth1'\n\n\
             config interface 'wan'\n\toption device 'eth0'\n\toption proto 'dhcp'\n\n\
             config device\n\toption name 'eth0'\n\n\
             config interface 'fm350'\n\toption device 'eth2'\n\n\
             # config interface 'ghost'\n",
        )
        .unwrap();
        let types = section_types(p.to_str().unwrap());
        let interfaces: Vec<String> = types
            .iter()
            .filter(|(_, t)| t == "interface")
            .map(|(n, _)| n.clone())
            .collect();
        assert_eq!(interfaces, vec!["loopback", "wan", "fm350"]);
        // The bridge device section must not leak into interface discovery
        // (the regression behind "5G hardware not configured").
        assert!(!interfaces.iter().any(|n| n == "br-lan"));
        fs::remove_file(&p).ok();
    }
}
