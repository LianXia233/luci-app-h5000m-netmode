//! The command-line dispatcher (the RPC surface the LuCI frontend calls).
//!
//! LuCI uses `fs.exec` against `/usr/sbin/h5000m-netmode` with these exact
//! commands; every command, argument shape, stdout contract and exit code is
//! preserved. All mutating commands serialise on the state-file writer lock;
//! `status` is lock-free and read-only.

use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::config::{uci, AppConfig};
use crate::network::{self};
use crate::state;
use crate::system::clock::{now_cs, unix_ts};
use crate::system::log;
use crate::types::{Mode, Result};

pub fn usage() {
    eprintln!(
        "Usage: h5000m-netmode <command> [args]\n\n\
  status                     print the current state (read-only, no locking)\n\
  apply                      converge the live state on the configured plan\n\
  set <mode> [--wait]        switch the exit group; returns immediately unless\n\
                             --wait is given (mode: wan_first|modem_first|\n\
                             wan_only|modem_only)\n\
  align [--wait]             repair an IPv4/IPv6 split (one-click align)\n\
  switch-worker <mode> <n>   internal: the detached switch worker\n\
  align-worker               internal: the detached align worker\n\
  notify <section> [action]  internal: hotplug event (coalesced, debounced)\n\
  reconcile                  serialised re-evaluation (hotplug + watchdog)\n\
  watch                      procd watchdog loop\n\
  iface-role <section>       classify a network section (wan|modem|other)\n\
  health                     run the bounded health probes now\n\
  list-devices               physical netdevs for the mapping picker\n\
  get-device-map             current manual mapping\n\
  set-device-map <role> <dev>\n\
  eth-candidates             sections that could be the wired fallback\n\
  eth-fallback get|set <sections>"
    );
}

fn netdev_exists(dev: &str) -> bool {
    crate::network::sysfs::netdev_exists(dev)
}

/// `list-devices`: physical netdevs named like the shell's
/// `ip link show | sed -n 's/^[0-9]*: \([eu][ths][b0-9a-zA-Z.]*\):.*/\1/p' | sort -V`.
fn list_devices() {
    let mut devs: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(crate::network::sysfs::sys_net()) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().into_string().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let first = name.as_bytes()[0];
        if !matches!(first, b'e' | b'u' | b't' | b'h' | b's') {
            continue;
        }
        if name.len() < 2 {
            continue;
        }
        let second = name.as_bytes()[1];
        if !matches!(second, b't' | b'h' | b's' | b'b') {
            continue;
        }
        devs.push(name);
    }
    devs.sort_by(|a, b| version_cmp(a, b));
    for d in devs {
        println!("{d}");
    }
}

/// BusyBox `sort -V` equivalent for device names.
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<&str> = a.split(|c: char| !c.is_ascii_digit()).collect();
    let b_parts: Vec<&str> = b.split(|c: char| !c.is_ascii_digit()).collect();
    let mut an = a_parts.iter().filter_map(|p| p.parse::<u64>().ok());
    let mut bn = b_parts.iter().filter_map(|p| p.parse::<u64>().ok());
    loop {
        match (an.next(), bn.next()) {
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
            }
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (None, None) => return a.cmp(b),
        }
    }
}

fn spawn_worker(args: &[&str]) -> Result<()> {
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("/usr/sbin/h5000m-netmode"));
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // setsid(1): detach from the caller's session so the worker survives the
    // RPC connection (which rides the very exit being switched).
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                let e = std::io::Error::last_os_error();
                // Already a session leader: fine.
                if e.raw_os_error() != Some(libc::EPERM) {
                    return Err(e);
                }
            }
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(_) => Ok(()),
        Err(e) => Err(crate::types::Error::system(format!("spawn worker: {e}"))),
    }
}

/// `cmd_set <mode> [--wait]` (see the shell version for the rc contract:
/// 64 invalid, 3 queued-busy, 0 started).
pub fn cmd_set(cfg: &AppConfig, target_mode: &str, wait_flag: &str) -> i32 {
    let Some(mode) = Mode::parse(target_mode) else {
        eprintln!("invalid exit policy");
        return 64;
    };
    let target = mode.primary_group();
    let gen: u32 = crate::types::num_or(&state::state_get("gen"), 0) + 1;
    let _ = state::state_write(&[
        ("gen", &gen.to_string()),
        ("requested_mode", target_mode),
        ("requested_ts", &unix_ts().to_string()),
        ("requested_by", "luci"),
    ]);

    if state::switch_in_progress() {
        log::log_info(
            "switch",
            &format!(
                "switch request queued: {} is already running",
                state::switch_state()
            ),
        );
        println!("state=busy");
        println!("gen={gen}");
        println!("target={}", target.as_str());
        println!("mode={target_mode}");
        return 3;
    }

    println!("state=started");
    println!("gen={gen}");
    println!("target={}", target.as_str());
    println!("mode={target_mode}");

    if wait_flag == "--wait" || std::env::var("H5000M_FOREGROUND").ok().as_deref() == Some("1") {
        return cmd_switch_worker(cfg, target_mode, gen);
    }
    let _ = spawn_worker(&["switch-worker", target_mode, &gen.to_string()]);
    0
}

/// `cmd_switch_worker <mode> <gen>`.
pub fn cmd_switch_worker(cfg: &AppConfig, applied: &str, my_gen: u32) -> i32 {
    let Some(mode) = Mode::parse(applied) else {
        return 64;
    };
    if !state::acquire_lock(cfg.switch_lock_wait) {
        if state::switch_in_progress() {
            log::log_info("switch", &format!("switch worker deferred: a switch is already running and will pick up request gen={my_gen}"));
            return 2;
        }
        log::log_info(
            "switch",
            "switch worker could not start: another writer holds the lock",
        );
        let _ = state::state_write(&[
            ("state", crate::types::SmState::FAILED),
            ("kind", "switch"),
            ("result", "failed"),
            ("reason", "lock_busy"),
            ("target", mode.primary_group().as_str()),
            ("target_mode", applied),
            ("pid", ""),
            ("gen", &my_gen.to_string()),
            ("started", &unix_ts().to_string()),
            ("elapsed", "0.00s"),
            ("phases", ""),
            (
                "message",
                "switch request dropped: another writer holds the lock",
            ),
        ]);
        return 2;
    }
    let held = true;
    let mut rc = 0;
    let mut applied = applied.to_string();
    let mut gen = my_gen;
    for _ in 0..3 {
        let mut snap = network::read_live_state();
        if crate::switch::switch_run(
            cfg,
            Mode::parse(&applied).unwrap_or(Mode::WanFirst),
            "switch",
            None,
            &mut snap,
        )
        .is_ok()
        {
            rc = 0;
        } else {
            rc = 1;
        }
        let pending = state::state_get("requested_mode");
        let applied_mode = state::state_get("applied_mode");
        let pending_gen: u32 = crate::types::num_or(&state::state_get("gen"), 0);
        if pending_gen > gen && !pending.is_empty() && pending != applied_mode {
            log::log_info(
                "switch",
                &format!("a newer request ({pending}) arrived while switching; applying it now"),
            );
            gen = pending_gen;
            applied = pending;
            continue;
        }
        break;
    }
    // Re-arm the warm standby route and the audit field after the switch.
    let snap = network::read_live_state();
    let _ = crate::reconcile::reconcile_body(cfg, &snap, true);
    if held {
        state::release_lock();
    }
    rc
}

/// `cmd_align [--wait]`.
pub fn cmd_align(cfg: &AppConfig, wait_flag: &str) -> i32 {
    let gen: u32 = crate::types::num_or(&state::state_get("gen"), 0) + 1;
    let _ = state::state_write(&[
        ("gen", &gen.to_string()),
        ("requested_ts", &unix_ts().to_string()),
        ("requested_by", "align"),
    ]);
    if state::switch_in_progress() {
        println!("state=busy");
        return 3;
    }
    println!("state=started");
    println!("gen={gen}");
    if wait_flag == "--wait" || std::env::var("H5000M_FOREGROUND").ok().as_deref() == Some("1") {
        return cmd_align_worker(cfg);
    }
    let _ = spawn_worker(&["align-worker"]);
    0
}

/// `cmd_align_worker`.
pub fn cmd_align_worker(cfg: &AppConfig) -> i32 {
    if !state::acquire_lock(cfg.switch_lock_wait) {
        log::log_info(
            "reconcile",
            "align worker skipped: another writer holds the lock",
        );
        return 2;
    }
    let snap = network::read_live_state();
    let rc = crate::reconcile::reconcile_body(cfg, &snap, true);
    state::release_lock();
    match rc {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// `hotplug_notify <section> [action]` with the coalescing debounce marker.
pub fn hotplug_notify(cfg: &AppConfig, section: &str, action: &str) {
    let marker = state::state_file().with_extension("state.debounce");
    let events: u32 = crate::types::num_or(&state::state_get("hotplug_events"), 0) + 1;
    let _ = state::state_write(&[
        ("hotplug_last", &format!("{section}:{action}")),
        ("hotplug_events", &events.to_string()),
        ("hotplug_last_ts", &unix_ts().to_string()),
        ("health_force", "1"),
    ]);
    // A marker whose owner died (SIGKILL, OOM, a reboot that kept /var/run on
    // a persistent overlay) or that outlived any plausible debounce window
    // would swallow every later event: the interface goes down, hotplug fires,
    // and nothing ever evaluates the exit again. Reap it before trusting it.
    let max_age = (cfg.hotplug_debounce as u64)
        .saturating_mul(4)
        .saturating_add(10);
    if clear_stale_marker(&marker, max_age) {
        log::log_warn("hotplug", "dropped a stale debounce marker");
    }
    if std::fs::create_dir(&marker).is_err() {
        // Coalesced into the run that is already scheduled.
        return;
    }
    let _ = std::fs::write(marker.join("pid"), std::process::id().to_string());
    let runs: u32 = crate::types::num_or(&state::state_get("hotplug_runs"), 0) + 1;
    let _ = state::state_write(&[("hotplug_runs", &runs.to_string())]);
    log::log_info(
        "hotplug",
        &format!(
            "{section}/{action} -> reconcile in {}s",
            cfg.hotplug_debounce
        ),
    );

    let debounce = cfg.hotplug_debounce;
    if debounce > 0 {
        std::thread::sleep(std::time::Duration::from_secs(debounce as u64));
    }
    let _ = std::fs::remove_dir_all(&marker);

    if !state::acquire_lock(cfg.reconcile_wait) {
        return;
    }
    let snap = network::read_live_state();
    let _ = crate::reconcile::reconcile_body(cfg, &snap, true);
    state::release_lock();
}

/// Remove a debounce marker whose owner is gone or that has overrun its
/// window; returns true when one was removed.
fn clear_stale_marker(marker: &std::path::Path, max_age_s: u64) -> bool {
    let Ok(md) = std::fs::metadata(marker) else {
        return false;
    };
    if !md.is_dir() {
        return false;
    }
    let pid: u32 = std::fs::read_to_string(marker.join("pid"))
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    let owner_dead = pid == 0 || !state::proc_alive(pid);
    let expired = md
        .modified()
        .ok()
        .and_then(|m| m.elapsed().ok())
        .map(|e| e.as_secs() > max_age_s)
        .unwrap_or(false);
    if owner_dead || expired {
        let _ = std::fs::remove_dir_all(marker);
        return true;
    }
    false
}

/// `run_watch`: procd-managed watchdog loop.
pub fn run_watch(cfg: &AppConfig) {
    log::log_info(
        "watch",
        &format!("watcher started, interval={}s", cfg.watch_interval),
    );
    // First pass immediately, then every interval.
    let snap = network::read_live_state();
    let _ = crate::reconcile::reconcile_body(cfg, &snap, false);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(cfg.watch_interval as u64));
        // Reload the config every tick: a manual `set` writes uci without
        // restarting this watcher, and a frozen startup snapshot makes the
        // reconcile loop silently drag the FIB back to the OLD mode seconds
        // after a successful manual switch (manual settings outrank
        // everything). Interval changes take effect on the next tick.
        let cfg = AppConfig::load();
        let snap = network::read_live_state();
        let _ = crate::reconcile::reconcile_body(&cfg, &snap, false);
    }
}

/// `get-device-map`.
fn cmd_get_device_map() {
    let snap = network::read_live_state();
    println!("wan={}", snap.wan_device);
    println!("modem={}", snap.modem_device);
    println!("wan_source={}", snap.wan_device_source);
    println!("modem_source={}", snap.modem_device_source);
}

/// `eth-candidates`.
fn cmd_eth_candidates() {
    for sec in uci::interface_sections() {
        match sec.as_str() {
            "wan" | "wan6" | "loopback" | "lan" => continue,
            _ => {}
        }
        println!("{sec}");
    }
}

fn uci_gen_bump() {}

/// `eth-fallback get|set`.
fn cmd_eth_fallback(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        Some("get") => {
            println!("{}", uci::uci_get("h5000m_netmode.settings.eth_fallback"));
            0
        }
        Some("set") if args.len() >= 2 => {
            let sections = args[1..].join(" ");
            if uci::uci_set_commit(
                "h5000m_netmode",
                "h5000m_netmode.settings.eth_fallback",
                &sections,
            )
            .is_err()
            {
                return 1;
            }
            uci_gen_bump();
            log::log_info("config", &format!("eth_fallback={sections}"));
            0
        }
        _ => {
            eprintln!("Usage: h5000m-netmode eth-fallback {{set <sections>|get}}");
            64
        }
    }
}

/// `set-device-map <role> <dev>`.
fn cmd_set_device_map(cfg: &AppConfig, role: &str, dev: &str) -> i32 {
    if !matches!(role, "wan" | "modem") || dev.is_empty() {
        eprintln!("Usage: h5000m-netmode set-device-map {{wan|modem}} <device>");
        return 64;
    }
    if !netdev_exists(dev) {
        log::log_warn(
            "config",
            &format!("warning: {dev} is not present in /sys/class/net"),
        );
    }
    // A device mapped to both exits makes every ownership verdict ambiguous.
    // The snapshot would silently arbitrate (wired wins); a *manual* conflict
    // is rejected outright instead - the user asked for an impossible layout.
    let snap = network::read_live_state();
    let (other_devs, other_name): (&[String], &str) = if role == "wan" {
        (&snap.modem_devs, "modem")
    } else {
        (&snap.wan_devs, "wan")
    };
    if other_devs.iter().any(|d| d == dev) {
        eprintln!(
            "device conflict: {dev} already carries the {other_name} exit; \
             pick a different device or clear the {other_name} mapping first"
        );
        log::log_warn(
            "config",
            &format!("rejected manual map {role}={dev}: conflicts with {other_name}"),
        );
        let _ = cfg;
        return 64;
    }
    let key = format!("h5000m_netmode.settings.{role}_device");
    if uci::uci_set_commit("h5000m_netmode", &key, dev).is_err() {
        return 1;
    }
    uci_gen_bump();
    log::log_info("config", &format!("manual device map {role}={dev}"));
    0
}

/// `health`: one forced probe round; prints the cached verdicts.
fn cmd_health(cfg: &AppConfig) -> i32 {
    let snap = network::read_live_state();
    crate::health::refresh_health(&snap, cfg, true);
    println!("wan={}", state::health_get("wan"));
    println!("modem={}", state::health_get("modem"));
    println!("wan6={}", state::health_get("wan6"));
    println!("modem6={}", state::health_get("modem6"));
    println!("wan_devices={}", snap.wan_devs.join(" "));
    println!("modem_devices={}", snap.modem_devs.join(" "));
    0
}

/// `apply_policy` + `state_init` + `reconcile_body` (the first-install path).
fn cmd_apply(cfg: &AppConfig) -> i32 {
    if crate::switch::transaction::apply_policy(cfg.mode, &network::read_live_state()).is_err() {
        log::log_warn("apply", "failed to persist exit policy");
        eprintln!("failed to persist exit policy");
        return 1;
    }
    let snap = network::read_live_state();
    let _ = crate::reconcile::reconcile_body(cfg, &snap, false);
    0
}

/// `reconcile`: serialised evaluation (hotplug + watchdog path).
fn cmd_reconcile(cfg: &AppConfig) -> i32 {
    if !state::acquire_lock(cfg.reconcile_wait) {
        eprintln!("another exit policy update is already running");
        return 2;
    }
    let snap = network::read_live_state();
    let rc = crate::reconcile::reconcile_body(cfg, &snap, false);
    state::release_lock();
    match rc {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// The main dispatcher. Returns the process exit code.
pub fn dispatch(args: &[String]) -> i32 {
    let cfg = AppConfig::load();
    let action = args.first().map(|s| s.as_str()).unwrap_or("");

    match action {
        "now-cs" => {
            println!("{}", now_cs());
            0
        }
        "status" => {
            crate::system::log::disable();
            let snap = network::read_live_state();
            crate::status::print_status(&cfg, &snap);
            0
        }
        "iface-role" => {
            let sec = args.get(1).map(|s| s.as_str()).unwrap_or("");
            if sec.is_empty()
                || !sec.chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '@' || c == '-'
                })
            {
                return 64;
            }
            println!("{}", network::iface_role(sec));
            0
        }
        "list-devices" => {
            list_devices();
            0
        }
        "get-device-map" => {
            cmd_get_device_map();
            0
        }
        "eth-candidates" => {
            cmd_eth_candidates();
            0
        }
        "eth-fallback" => {
            let sub: Vec<String> = args[1..].to_vec();
            if sub.is_empty() {
                eprintln!("Usage: h5000m-netmode eth-fallback {{set <sections>|get}}");
                return 64;
            }
            if sub[0] == "set" {
                if !state::acquire_lock(0) {
                    eprintln!("another exit policy update is already running");
                    return 2;
                }
                let rc = cmd_eth_fallback(&sub);
                state::release_lock();
                return rc;
            }
            cmd_eth_fallback(&sub)
        }
        "set-device-map" => {
            if !state::acquire_lock(0) {
                eprintln!("another exit policy update is already running");
                return 2;
            }
            let rc = cmd_set_device_map(
                &cfg,
                args.get(1).map(|s| s.as_str()).unwrap_or(""),
                args.get(2).map(|s| s.as_str()).unwrap_or(""),
            );
            state::release_lock();
            rc
        }
        "health" => cmd_health(&cfg),
        "set" | "switch" => {
            if args.len() < 2 {
                usage();
                return 64;
            }
            cmd_set(
                &cfg,
                &args[1],
                args.get(2).map(|s| s.as_str()).unwrap_or(""),
            )
        }
        "align" => cmd_align(&cfg, args.get(1).map(|s| s.as_str()).unwrap_or("")),
        "switch-worker" => {
            if args.len() < 2 {
                return 64;
            }
            let gen: u32 = crate::types::num_or(args.get(2).map(|s| s.as_str()).unwrap_or("0"), 0);
            cmd_switch_worker(&cfg, &args[1], gen)
        }
        "align-worker" => cmd_align_worker(&cfg),
        "notify" => {
            let sec = args.get(1).map(|s| s.as_str()).unwrap_or("unknown");
            if sec.is_empty()
                || !sec.chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '@' || c == '-'
                })
            {
                return 0;
            }
            match network::iface_role(sec) {
                "wan" | "modem" => {}
                _ => return 0,
            }
            hotplug_notify(
                &cfg,
                sec,
                args.get(2).map(|s| s.as_str()).unwrap_or("event"),
            );
            0
        }
        "watch" => {
            run_watch(&cfg);
            0
        }
        "reconcile" => cmd_reconcile(&cfg),
        "apply" | "" => {
            // Every mutating action is serialised.
            if !state::acquire_lock(0) {
                eprintln!("another exit policy update is already running");
                return 2;
            }
            let rc = cmd_apply(&cfg);
            state::release_lock();
            rc
        }
        _ => {
            usage();
            64
        }
    }
}
