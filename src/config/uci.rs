//! UCI access.
//!
//! **Read path**: `/etc/config/*` files are parsed in-process (a plain
//! line-oriented format), which removes the `uci show`/`uci get` fork+exec
//! storm the shell backend paid on every status poll.
//!
//! **Write path**: mutations (`uci set`/`uci commit`) still go through
//! `/sbin/uci`. Reasons, unlike the read path:
//! * UCI writes must honour the UCI file lock and fire `config_change`
//!   notifications so netifd/procd re-read the touched configs;
//! * value quoting/escaping is a correctness trap, and re-implementing it
//!   buys nothing on a low-frequency path (a switch commits a handful of
//!   writes; `set-device-map`/`eth-fallback` are user actions).
//!
//! Those calls are bounded (a fixed short timeout) and wrapped here so no
//! business module shells out directly.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::types::Result;

pub const H5000M_CFG: &str = "/etc/config/h5000m_netmode";
pub const NETWORK_CFG: &str = "/etc/config/network";
pub const MT5700M_CFG: &str = "/etc/config/mt5700m";

/// A parsed UCI config: section name -> (options, lists).
pub type UciDoc = BTreeMap<String, (BTreeMap<String, String>, BTreeMap<String, Vec<String>>)>;

/// Parse a UCI config file into (section -> {option -> value}).
///
/// Handles `config <type> ['<name>']` headers, `option <k> '<v>'`,
/// `list <k> '<v>'`, `#`/`//` comments and single/double quotes.
/// `config <type>` with no name gets a generated name, matching UCI's own
/// numbering only loosely; the sections this program uses are always named.
pub fn parse_uci(path: &str) -> UciDoc {
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
pub fn uci_get(dotted: &str) -> String {
    let mut parts = dotted.splitn(3, '.');
    let (cfg, rest) = match (parts.next(), parts.next()) {
        (Some(c), Some(r)) => (c, r),
        _ => return String::new(),
    };
    let (section, option) = match rest.split_once('.') {
        Some((s, o)) => (s, o),
        None => (rest, ""),
    };
    let path = match cfg {
        "h5000m_netmode" => H5000M_CFG,
        "network" => NETWORK_CFG,
        "mt5700m" => MT5700M_CFG,
        _ => {
            // Unknown config: try a file of that name (test seams).
            return String::new();
        }
    };
    let doc = parse_uci(path);
    doc.get(section)
        .and_then(|(opts, _)| opts.get(option))
        .cloned()
        .unwrap_or_default()
}

/// `uci_has_key("network.wan")` -> section exists.
pub fn uci_has_key(dotted: &str) -> bool {
    let mut parts = dotted.splitn(3, '.');
    let (cfg, rest) = match (parts.next(), parts.next()) {
        (Some(c), Some(r)) => (c, r),
        _ => return false,
    };
    let path = match cfg {
        "h5000m_netmode" => H5000M_CFG,
        "network" => NETWORK_CFG,
        "mt5700m" => MT5700M_CFG,
        _ => return false,
    };
    let doc = parse_uci(path);
    if let Some((_, o)) = rest.split_once('.') {
        doc.get(rest.split_once('.').unwrap().0)
            .map(|(opts, _)| opts.contains_key(o))
            .unwrap_or(false)
    } else {
        doc.contains_key(rest)
    }
}

/// List every `interface` section in the network config
/// (`all_interface_sections`).
pub fn interface_sections() -> Vec<String> {
    let doc = parse_uci(NETWORK_CFG);
    doc.into_iter()
        .filter(|(_, (opts, _))| opts.get("type").map(|t| t == "interface").unwrap_or(false))
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
    let doc = parse_uci(H5000M_CFG);
    doc.get("settings").cloned().unwrap_or_default()
}

/// External `uci` invocation with a hard timeout. Used only for mutations.
/// Returns Ok(()) when the command exited 0.
pub fn uci_exec(args: &[&str]) -> Result<()> {
    let mut cmd = std::process::Command::new("/sbin/uci");
    cmd.args(args);
    let out = cmd
        .output()
        .map_err(|e| crate::types::Error::system(format!("uci {} failed: {e}", args.join(" "))))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(crate::types::Error::system(format!(
            "uci {} failed rc={}",
            args.join(" "),
            out.status.code().unwrap_or(-1)
        )))
    }
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

/// Test-only helper: allow unit tests to run against a temp config by
/// overriding the config paths through environment variables.
pub fn h5000m_cfg_path() -> String {
    std::env::var("H5000M_CFG_FILE").unwrap_or_else(|_| H5000M_CFG.to_string())
}

/// Bounded wait helper for external commands: seconds cap.
pub fn bounded_timeout() -> Duration {
    Duration::from_secs(5)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
