//! Test-only mock binary for the deterministic backend test suite.
//!
//! The backend shells out to exactly three OpenWrt binaries on the write /
//! daemon-query paths (`uci`, `ubus`, `ifup`); the test suite redirects each
//! of them to a copy of THIS binary through the `H5000M_UCI_BIN` /
//! `H5000M_UBUS_BIN` / `H5000M_IFUP_BIN` seams and the mockbin directory on
//! PATH. The role is selected by the file name the copy is invoked as
//! (`ubus`, `ifup`, `uci`).
//!
//! * `uci`   - a faithful-enough mini-uci operating on a scenario config
//!   directory (`H5M_MOCK_DIR`): `set`/`get`/`commit`/`batch`, section and
//!   option writes, value round-trips through the SAME parser the backend
//!   uses, and an append-only operations log (`uci-ops.log`).
//! * `ubus`  - `call network.interface.<sec> status` answers from
//!   `<dir>/ubus/<sec>.json`; `network.wireless` answers from
//!   `<dir>/ubus/_wireless.json`; everything else fails like a missing object.
//! * `ifup`  - appends one line to `<dir>/ifup.log`.
//!
//! Not installed on any device, never shipped in the package.

use std::io::Read;
use std::path::PathBuf;

fn mock_dir() -> PathBuf {
    PathBuf::from(
        std::env::var("H5M_MOCK_DIR").expect("H5M_MOCK_DIR must point at the scenario dir"),
    )
}

fn role() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "unknown".into())
}

// ---------------------------------------------------------------------------
// mini-uci
// ---------------------------------------------------------------------------

fn uci_main(args: Vec<String>) -> i32 {
    let args: Vec<String> = args.into_iter().filter(|a| a != "-q").collect();
    let dir = mock_dir().join("config");
    let log = mock_dir().join("uci-ops.log");
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let rest = &args[1.min(args.len())..];

    let append_log = |line: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            let _ = writeln!(f, "{line}");
        }
    };

    match cmd {
        "get" => {
            let Some(key) = rest.first() else {
                return 1;
            };
            match uci_get(&dir, key) {
                Some(v) => {
                    println!("{v}");
                    0
                }
                None => 1,
            }
        }
        "set" => {
            let Some(kv) = rest.first() else {
                return 1;
            };
            append_log(&format!("set {kv}"));
            match uci_set(&dir, kv) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        }
        "commit" => {
            let cfg = rest.first().map(|s| s.as_str()).unwrap_or("");
            append_log(&format!("commit {cfg}"));
            0
        }
        "batch" => {
            let mut script = String::new();
            if std::io::stdin().read_to_string(&mut script).is_err() {
                return 1;
            }
            for line in script.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let mut parts = line.splitn(2, ' ');
                let verb = parts.next().unwrap_or("");
                let body = parts.next().unwrap_or("");
                match verb {
                    "set" => {
                        append_log(&format!("set {body}"));
                        if uci_set(&dir, body).is_err() {
                            return 1;
                        }
                    }
                    "commit" => {
                        append_log(&format!("commit {body}"));
                    }
                    _ => {}
                }
            }
            0
        }
        _ => 1,
    }
}

/// Split `cfg.section[.option]`.
fn split_dotted(key: &str) -> Option<(String, String, Option<String>)> {
    let mut it = key.split('.');
    let cfg = it.next()?.to_string();
    let sec = it.next()?.to_string();
    let opt = it.next().map(|s| s.to_string());
    if it.next().is_some() {
        return None;
    }
    Some((cfg, sec, opt))
}

fn uci_get(dir: &std::path::Path, key: &str) -> Option<String> {
    let (cfg, sec, opt) = split_dotted(key)?;
    let doc = h5000m_netmode::config::uci::parse_uci(&dir.join(cfg).to_string_lossy());
    let (opts, _) = doc.get(&sec)?;
    Some(opts.get(&opt?)?.clone())
}

fn uci_set(dir: &std::path::Path, kv: &str) -> Result<(), String> {
    let (key, val) = kv
        .split_once('=')
        .ok_or_else(|| format!("not a key=value: {kv}"))?;
    let (cfg, sec, opt) = split_dotted(key).ok_or_else(|| format!("bad key {key}"))?;
    let path = dir.join(&cfg);
    let mut doc = h5000m_netmode::config::uci::parse_uci(&path.to_string_lossy());
    match opt {
        Some(opt) => {
            doc.entry(sec).or_default().0.insert(opt, val.to_string());
        }
        None => {
            // Section type declaration: `uci set cfg.section=type`.
            doc.entry(sec).or_default();
        }
    }
    render_doc(&doc, &path);
    Ok(())
}

/// Rewrite the file from the parsed document. Section headers keep their
/// ORIGINAL type (scanned from the previous file; `interface` for new
/// sections) - `parse_uci` drops the type, and `interface_sections()` in the
/// backend selects on it. Comments and blank lines are normalised away;
/// fixtures are machine-written anyway.
fn render_doc(doc: &h5000m_netmode::config::uci::UciDoc, path: &std::path::Path) {
    let mut types: std::collections::BTreeMap<String, String> = Default::default();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() >= 3 && f[0] == "config" {
                types.insert(f[2].trim_matches('\'').to_string(), f[1].to_string());
            }
        }
    }
    let mut out = String::new();
    for (sec, (opts, lists)) in doc {
        let ty = types.get(sec).map(|s| s.as_str()).unwrap_or("interface");
        out.push_str(&format!("config {ty} '{sec}'\n"));
        for (k, v) in opts {
            out.push_str(&format!("\toption {k} '{v}'\n"));
        }
        for (k, vs) in lists {
            for v in vs {
                out.push_str(&format!("\tlist {k} '{v}'\n"));
            }
        }
        out.push('\n');
    }
    let _ = std::fs::write(path, out);
}

// ---------------------------------------------------------------------------
// mini-ubus
// ---------------------------------------------------------------------------

fn ubus_main(args: Vec<String>) -> i32 {
    // `ubus call network.interface.<sec> status` / `ubus call network.wireless status`.
    let is_call = args.first().map(|s| s.as_str()) == Some("call");
    let object = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let dir = mock_dir().join("ubus");
    let file = if let Some(sec) = object.strip_prefix("network.interface.") {
        dir.join(format!("{sec}.json"))
    } else if object == "network.wireless" {
        dir.join("_wireless.json")
    } else {
        return 1;
    };
    if !is_call || !file.exists() {
        return 1;
    }
    match std::fs::read(&file) {
        Ok(bytes) => {
            use std::io::Write;
            let _ = std::io::stdout().write_all(&bytes);
            0
        }
        Err(_) => 1,
    }
}

// ---------------------------------------------------------------------------
// mini-ifup
// ---------------------------------------------------------------------------

fn ifup_main(args: Vec<String>) -> i32 {
    use std::io::Write;
    let sec = args.first().cloned().unwrap_or_default();
    let path = mock_dir().join("ifup.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{sec}");
    }
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match role().trim_end_matches(".exe").to_string().as_str() {
        "uci" => uci_main(args),
        "ubus" => ubus_main(args),
        "ifup" => ifup_main(args),
        _ => 1,
    };
    std::process::exit(code);
}
