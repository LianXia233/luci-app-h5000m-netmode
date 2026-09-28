//! The `status` command: full key=value snapshot for `h5000m-netmode-status`.
//!
//! Every field name, value and default is byte-compatible with the legacy
//! shell `print_status` – the LuCI page and `tests/test_exit_card.js` parse
//! this output directly. The status path is read-only: no lock, no probes,
//! no UCI writes; verdicts come from the health cache.

use crate::config::uci;
use crate::config::AppConfig;
use crate::network::{self, LiveSnapshot};
use crate::state;
use crate::system::clock::unix_ts;
use crate::system::command;
use crate::types::{Family, Group, Mode};

fn cached_health(g: Group, fam: Family) -> String {
    let k = format!("{}{}", g.as_str(), fam.suffix());
    let v = state::health_get(&k);
    if v.is_empty() {
        "unknown".to_string()
    } else {
        v
    }
}

fn bool1(v: bool) -> &'static str {
    if v {
        "1"
    } else {
        "0"
    }
}

fn watch_interval(cfg: &AppConfig) -> u32 {
    cfg.watch_interval.clamp(3, 3600)
}

fn health_check_enabled(cfg: &AppConfig) -> bool {
    cfg.health_check
}

fn wireless_state() -> (u32, u32, String, u32) {
    let mut up = 0u32;
    let mut ssid = String::new();
    let mut clients = 0u32;
    let Some(json) = command::ubus_wireless_status() else {
        return (0, 0, String::new(), 0);
    };
    // Count `"up":true` occurrences per radio; ssid from interfaces[].config.ssid.
    for token in json.split("\"up\":") {
        if token.starts_with("true") {
            up += 1;
        }
    }
    let total = json.matches("\"up\"").count() as u32;
    if let Some(pos) = json.find("\"ssid\":") {
        let rest = &json[pos + 8..];
        let rest = rest.trim_start();
        if let Some(v) = rest.strip_prefix('"') {
            if let Some(end) = v.find('"') {
                ssid = v[..end].to_string();
            }
        }
    }
    // Interfaces: `"ifname":"..."`.
    let mut ifnames = Vec::new();
    for line in json.split('"') {
        // crude: any quoted value that looks like wlanX
        if line.len() >= 4 && line.starts_with("wlan") {
            ifnames.push(line.to_string());
        }
    }
    for dev in ifnames {
        if let Some(out) = command::ubus_iwinfo_assoclist(&dev) {
            clients += out.lines().count() as u32;
        }
    }
    (total, up, ssid, clients)
}

/// Emit the full status snapshot on stdout.
pub fn print_status(cfg: &AppConfig, snap: &LiveSnapshot) {
    let primary = cfg.mode.primary_group();
    let backup = primary.other();
    let active = network::slot_owner(snap);

    let mut split = 0;
    if !snap.egress4.is_empty()
        && !snap.egress6.is_empty()
        && snap.active4 != "none"
        && snap.active6 != "none"
        && snap.active4 != snap.active6
    {
        split = 1;
    }
    let mut split_detail = String::new();
    let s4 = network::slot_owner_family(snap, Family::V4);
    let s6 = network::slot_owner_family(snap, Family::V6);
    if s4 != s6 {
        split_detail = format!("ipv4={s4} ipv6={s6}");
    }

    let mut out = String::new();

    line(&mut out, "mode", cfg.mode.as_str());
    line(&mut out, "ipv6_policy", "follow_active_group");
    line(
        &mut out,
        "ipv6_owner",
        uci::uci_get("h5000m_netmode.settings.ipv6_owner"),
    );
    line(&mut out, "ipv6_desired", &active);
    line(&mut out, "ipv6_capable_wan", bool1(snap.wan6_capable));
    line(&mut out, "ipv6_capable_modem", bool1(snap.modem6_capable));
    line(&mut out, "strict_dual_stack", bool1(cfg.strict_dual_stack));

    line(&mut out, "group_primary", primary.as_str());
    line(&mut out, "group_backup", backup.as_str());
    line(&mut out, "group_active", &active);

    line(&mut out, "wan_present", bool1(snap.wan.present));
    line(&mut out, "wan_available", bool1(snap.wan.available));
    line(&mut out, "wan_pending", bool1(snap.wan.pending));
    line(&mut out, "wan_carrier", &snap.wan.carrier);
    line(&mut out, "wan_up", bool1(snap.wan.up));
    line(&mut out, "wan6_up", bool1(snap.wan6.up));
    line(&mut out, "wan4_ready", bool1(snap.wan4_ready));
    line(&mut out, "wan6_ready", bool1(snap.wan6_ready));
    line(&mut out, "wan_device", &snap.wan_device);
    line(&mut out, "wan_devices", snap.wan_devs.join(" "));
    line(&mut out, "wan_device_source", &snap.wan_device_source);

    line(&mut out, "modem_present", bool1(snap.modem.present));
    line(&mut out, "modem_available", bool1(snap.modem.available));
    line(&mut out, "modem_pending", bool1(snap.modem.pending));
    line(&mut out, "modem_carrier", &snap.modem.carrier);
    line(&mut out, "modem_up", bool1(snap.modem.up));
    line(&mut out, "modem6_up", bool1(snap.modem6.up));
    line(&mut out, "modem4_ready", bool1(snap.modem4_ready));
    line(&mut out, "modem6_ready", bool1(snap.modem6_ready));
    line(&mut out, "modem_device", &snap.modem_device);
    line(&mut out, "modem_devices", snap.modem_devs.join(" "));
    line(&mut out, "modem_device_source", &snap.modem_device_source);
    line(&mut out, "modem_interface", &snap.modem4_sec);
    line(&mut out, "modem6_interface", &snap.modem6_sec);

    line(
        &mut out,
        "egress4",
        if snap.egress4.is_empty() {
            "none"
        } else {
            &snap.egress4
        },
    );
    line(
        &mut out,
        "egress6",
        if snap.egress6.is_empty() {
            "none"
        } else {
            &snap.egress6
        },
    );
    line(&mut out, "active4", &snap.active4);
    line(&mut out, "active6", &snap.active6);
    line(&mut out, "split", split);
    line(&mut out, "split_detail", &split_detail);
    line(
        &mut out,
        "default4",
        snap.default4
            .as_ref()
            .map(|r| r.render())
            .unwrap_or_else(|| "none".into()),
    );
    line(
        &mut out,
        "default6",
        snap.default6
            .as_ref()
            .map(|r| r.render())
            .unwrap_or_else(|| "none".into()),
    );

    line(
        &mut out,
        "win4_dev",
        if snap.win4_dev.is_empty() {
            "none"
        } else {
            &snap.win4_dev
        },
    );
    line(
        &mut out,
        "win6_dev",
        if snap.win6_dev.is_empty() {
            "none"
        } else {
            &snap.win6_dev
        },
    );
    line(
        &mut out,
        "win4_owner",
        if snap.win4_owner.is_empty() {
            "none"
        } else {
            &snap.win4_owner
        },
    );
    line(
        &mut out,
        "win6_owner",
        if snap.win6_owner.is_empty() {
            "none"
        } else {
            &snap.win6_owner
        },
    );
    line(&mut out, "win4_metric", snap.win4_metric);
    line(&mut out, "win6_metric", snap.win6_metric);
    line(&mut out, "win4_ecmp", snap.win4_ecmp);
    line(&mut out, "win6_ecmp", snap.win6_ecmp);
    // External (virtual) routing on the FIB winner. external_route=1 is
    // informational: the plugin keeps managing the physical WAN/5G uplinks
    // underneath, and external_physical_owner names the carrier they ride.
    line(&mut out, "external_route", snap.external_route);
    line(
        &mut out,
        "external_route_source",
        &snap.external_route_source,
    );
    line(
        &mut out,
        "external_physical_owner",
        &snap.external_physical_owner,
    );

    line(
        &mut out,
        "slot4_wan",
        opt_or0(snap.slot_metric(Group::Wan, Family::V4)),
    );
    line(
        &mut out,
        "gw4_wan",
        snap.group_gateway(Group::Wan, Family::V4)
            .unwrap_or_default(),
    );
    line(
        &mut out,
        "addr4_wan",
        bool1(network::group_family_has_address(
            snap,
            Group::Wan,
            Family::V4,
        )),
    );
    line(
        &mut out,
        "req4_wan",
        bool1(network::group_family_required(
            snap,
            Group::Wan,
            Family::V4,
            cfg.strict_dual_stack,
        )),
    );
    line(
        &mut out,
        "slot4_modem",
        opt_or0(snap.slot_metric(Group::Modem, Family::V4)),
    );
    line(
        &mut out,
        "gw4_modem",
        snap.group_gateway(Group::Modem, Family::V4)
            .unwrap_or_default(),
    );
    line(
        &mut out,
        "addr4_modem",
        bool1(network::group_family_has_address(
            snap,
            Group::Modem,
            Family::V4,
        )),
    );
    line(
        &mut out,
        "req4_modem",
        bool1(network::group_family_required(
            snap,
            Group::Modem,
            Family::V4,
            cfg.strict_dual_stack,
        )),
    );
    line(
        &mut out,
        "slot6_wan",
        opt_or0(snap.slot_metric(Group::Wan, Family::V6)),
    );
    line(
        &mut out,
        "gw6_wan",
        snap.group_gateway(Group::Wan, Family::V6)
            .unwrap_or_default(),
    );
    line(
        &mut out,
        "addr6_wan",
        bool1(network::group_family_has_address(
            snap,
            Group::Wan,
            Family::V6,
        )),
    );
    line(
        &mut out,
        "req6_wan",
        bool1(network::group_family_required(
            snap,
            Group::Wan,
            Family::V6,
            cfg.strict_dual_stack,
        )),
    );
    line(
        &mut out,
        "slot6_modem",
        opt_or0(snap.slot_metric(Group::Modem, Family::V6)),
    );
    line(
        &mut out,
        "gw6_modem",
        snap.group_gateway(Group::Modem, Family::V6)
            .unwrap_or_default(),
    );
    line(
        &mut out,
        "addr6_modem",
        bool1(network::group_family_has_address(
            snap,
            Group::Modem,
            Family::V6,
        )),
    );
    line(
        &mut out,
        "req6_modem",
        bool1(network::group_family_required(
            snap,
            Group::Modem,
            Family::V6,
            cfg.strict_dual_stack,
        )),
    );
    line(
        &mut out,
        "group_ready_wan",
        bool1(network::group_complete(
            snap,
            Group::Wan,
            cfg.strict_dual_stack,
        )),
    );
    line(
        &mut out,
        "group_ready_modem",
        bool1(network::group_complete(
            snap,
            Group::Modem,
            cfg.strict_dual_stack,
        )),
    );
    line(
        &mut out,
        "group_online_wan",
        bool1(network::group_complete_online(
            snap,
            Group::Wan,
            cfg.strict_dual_stack,
        )),
    );
    line(
        &mut out,
        "group_online_modem",
        bool1(network::group_complete_online(
            snap,
            Group::Modem,
            cfg.strict_dual_stack,
        )),
    );

    line(&mut out, "daed_exit_state", "none");
    line(
        &mut out,
        "watcher",
        if state::proc_alive(pid_of("watch")) {
            "on"
        } else {
            "off"
        },
    );
    line(&mut out, "watch_interval", watch_interval(cfg));
    line(&mut out, "health_check", bool1(health_check_enabled(cfg)));
    line(
        &mut out,
        "wan_health",
        cached_health(Group::Wan, Family::V4),
    );
    line(
        &mut out,
        "modem_health",
        cached_health(Group::Modem, Family::V4),
    );
    line(
        &mut out,
        "wan6_health",
        cached_health(Group::Wan, Family::V6),
    );
    line(
        &mut out,
        "modem6_health",
        cached_health(Group::Modem, Family::V6),
    );
    let probe_ts: u64 = crate::types::num_or(&state::health_get("ts"), 0);
    line(&mut out, "probe_ts", probe_ts);
    line(
        &mut out,
        "probe_ok_threshold",
        format!(
            "{}/{}",
            cfg.probe_ok.min(cfg.probe_attempts),
            cfg.probe_attempts
        ),
    );
    line(
        &mut out,
        "wan_fail_streak",
        network::cached_health_streak(Group::Wan, Family::V4),
    );
    line(
        &mut out,
        "modem_fail_streak",
        network::cached_health_streak(Group::Modem, Family::V4),
    );
    line(
        &mut out,
        "wan6_fail_streak",
        network::cached_health_streak(Group::Wan, Family::V6),
    );
    line(
        &mut out,
        "modem6_fail_streak",
        network::cached_health_streak(Group::Modem, Family::V6),
    );

    line(&mut out, "dns_check", bool1(cfg.dns_check));
    let dns_ok = state::health_get("dns");
    line(
        &mut out,
        "dns_ok",
        if dns_ok.is_empty() {
            "unknown"
        } else {
            &dns_ok
        },
    );
    line(
        &mut out,
        "dns_servers_active",
        dns_servers_of(active.as_str()),
    );

    let sw_state = state::switch_state();
    let sw_pid = state::state_get("pid");
    let started: u64 = crate::types::num_or(&state::state_get("started"), 0);
    let now = unix_ts();
    line(&mut out, "switch_state", &sw_state);
    line(&mut out, "switch_kind", state::state_get("kind"));
    line(&mut out, "switch_target", state::state_get("target"));
    line(
        &mut out,
        "switch_target_mode",
        state::state_get("target_mode"),
    );
    line(&mut out, "switch_result", state::state_get("result"));
    line(&mut out, "switch_reason", state::state_get("reason"));
    line(&mut out, "switch_message", state::state_get("message"));
    line(&mut out, "switch_elapsed", state::state_get("elapsed"));
    line(&mut out, "switch_phases", state::state_get("phases"));
    line(&mut out, "switch_started", started);
    line(
        &mut out,
        "switch_age",
        if started > 0 {
            now.saturating_sub(started)
        } else {
            0
        },
    );
    line(
        &mut out,
        "switch_gen",
        crate::types::num_or::<u32>(&state::state_get("gen"), 0),
    );
    line(
        &mut out,
        "requested_mode",
        state::state_get("requested_mode"),
    );
    line(&mut out, "switch_busy", bool1(state::switch_in_progress()));
    line(
        &mut out,
        "last_align_reason",
        state::state_get("last_align_reason"),
    );
    line(
        &mut out,
        "last_align_target",
        state::state_get("last_align_target"),
    );

    let (wifi_total, wifi_up, wifi_ssid, wifi_clients) = wireless_state();
    line(&mut out, "wifi_total", wifi_total);
    line(&mut out, "wifi_up", wifi_up);
    line(&mut out, "wifi_ssid", wifi_ssid);
    line(&mut out, "wifi_clients", wifi_clients);

    line(
        &mut out,
        "eth_fallback",
        uci::uci_get("h5000m_netmode.settings.eth_fallback"),
    );
    line(&mut out, "wan_metric", uci::uci_get("network.wan.metric"));
    line(&mut out, "wan6_metric", uci::uci_get("network.wan6.metric"));
    line(
        &mut out,
        "usb_metric",
        uci::uci_get(&format!("network.{}.metric", snap.modem4_sec)),
    );
    line(
        &mut out,
        "usbv6_metric",
        uci::uci_get(&format!("network.{}.metric", snap.modem6_sec)),
    );
    line(
        &mut out,
        "wan_defaultroute",
        uci::uci_get("network.wan.defaultroute"),
    );
    line(
        &mut out,
        "wan6_defaultroute",
        uci::uci_get("network.wan6.defaultroute"),
    );
    line(&mut out, "wan6_auto", uci::uci_get("network.wan6.auto"));
    line(
        &mut out,
        "usb_defaultroute",
        uci::uci_get(&format!("network.{}.defaultroute", snap.modem4_sec)),
    );
    line(
        &mut out,
        "usbv6_defaultroute",
        uci::uci_get(&format!("network.{}.defaultroute", snap.modem6_sec)),
    );
    line(
        &mut out,
        "usbv6_auto",
        uci::uci_get(&format!("network.{}.auto", snap.modem6_sec)),
    );
    let mut modem_metric = uci::uci_get("network.MT5700M.metric");
    if modem_metric.is_empty() {
        modem_metric = uci::uci_get(&format!("network.{}.metric", snap.modem4_sec));
    }
    if modem_metric.is_empty() {
        modem_metric = uci::uci_get("mt5700m.connection.metric");
    }
    line(&mut out, "modem_metric", modem_metric);

    line(&mut out, "switch_wait_ipv4", cfg.switch_wait_ipv4);
    line(&mut out, "switch_wait_ipv6", cfg.switch_wait_ipv6);
    line(&mut out, "switch_budget", cfg.switch_budget);
    line(&mut out, "switch_settle", cfg.switch_settle);
    line(&mut out, "switch_cooldown", cfg.switch_cooldown);
    line(&mut out, "hotplug_debounce", cfg.hotplug_debounce);
    line(&mut out, "align_confirm", cfg.align_confirm);
    line(&mut out, "gw_required", bool1(cfg.gw_required));

    let _ = sw_pid;
    print!("{out}");
}

fn opt_or0(v: Option<u32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "0".into())
}

/// Generic key=value line emitter for the status snapshot.
fn line<T: std::fmt::Display>(out: &mut String, k: &str, v: T) {
    out.push_str(&format!("{k}={v}\n"));
}

/// Watcher detection: the legacy shell checks `pgrep -f 'h5000m-netmode watch'`;
/// in Rust we look for a `h5000m-netmode` process whose argv contains "watch".
fn pid_of(arg: &str) -> u32 {
    let _ = arg;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    for e in entries.flatten() {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(cmdline) = std::fs::read(format!("/proc/{name}/cmdline")) else {
            continue;
        };
        let cmd = String::from_utf8_lossy(&cmdline);
        if cmd.contains("h5000m-netmode") && cmd.contains("watch") {
            if let Ok(pid) = name.parse::<u32>() {
                return pid;
            }
        }
    }
    0
}

/// `dns_servers_of <group>`: the DNS servers netifd assigned to the group's
/// sections (read-only, from the bounded ubus snapshot).
fn dns_servers_of(group: &str) -> String {
    let mut out = Vec::new();
    for fam in Family::ALL {
        let sec = match (group, fam) {
            ("wan", Family::V4) => "wan",
            ("wan", Family::V6) => "wan6",
            ("modem", Family::V4) => "modem",
            ("modem", Family::V6) => "modem6",
            _ => "",
        };
        if sec.is_empty() {
            continue;
        }
        if let Some(json) = command::ubus_section_status(sec) {
            for val in json_values(&json, "dns-server") {
                if !out.contains(&val) {
                    out.push(val);
                }
            }
        }
    }
    out.join(" ")
}

/// Collect every string inside a JSON array under the given key.
fn json_values(json: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = format!("\"{key}\":[");
    let Some(pos) = json.find(&needle) else {
        return out;
    };
    let rest = &json[pos + needle.len()..];
    let end = rest.find(']').unwrap_or(rest.len());
    for part in rest[..end].split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix('"') {
            if let Some(e) = v.find('"') {
                out.push(v[..e].to_string());
            }
        }
    }
    out
}

/// `mode` of the snapshot for tests.
pub fn current_mode(cfg: &AppConfig) -> Mode {
    cfg.mode
}
