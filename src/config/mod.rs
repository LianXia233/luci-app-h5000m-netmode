//! Configuration model: UCI parsing, defaults and validation.
//!
//! No business module parses UCI directly: they all read `AppConfig` and the
//! `uci_get` helper in `uci.rs`. The legacy defaults are preserved verbatim so
//! a stock install behaves identically.

pub mod uci;

use std::collections::BTreeMap;

use crate::types::{num_or, parse_u32, Mode};

/// Fully resolved settings for `h5000m_netmode.settings`.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub mode: Mode,
    pub watcher: bool,
    pub watch_interval: u32,
    pub health_check: bool,
    pub health_probe_interval: u32,
    pub strict_dual_stack: bool,
    pub switch_wait_ipv4: u32,
    pub switch_wait_ipv6: u32,
    pub switch_budget: u32,
    pub switch_settle: u32,
    pub switch_settle_warm: u32,
    pub probe_attempts: u32,
    pub probe_ok: u32,
    pub probe_timeout: u32,
    pub probe_fail_streak: u32,
    pub switch_cooldown: u32,
    pub align_confirm: u32,
    pub hotplug_debounce: u32,
    pub reconcile_wait: u32,
    pub switch_lock_wait: u32,
    pub status_cache: u32,
    pub probe_targets: Vec<String>,
    pub probe_targets6: Vec<String>,
    pub gw_required: bool,
    pub dns_check: bool,
    pub tcp_check: bool,
    pub eth_fallback: Vec<String>,
    pub wan_device: String,
    pub modem_device: String,
    pub ipv6_owner: String,
    pub dns_probe_name: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            mode: Mode::WanFirst,
            watcher: true,
            watch_interval: 10,
            health_check: true,
            health_probe_interval: 60,
            strict_dual_stack: false,
            switch_wait_ipv4: 15,
            switch_wait_ipv6: 20,
            switch_budget: 60,
            switch_settle: 1,
            switch_settle_warm: 0,
            probe_attempts: 3,
            probe_ok: 2,
            probe_timeout: 2,
            probe_fail_streak: 3,
            switch_cooldown: 20,
            align_confirm: 2,
            hotplug_debounce: 2,
            reconcile_wait: 5,
            switch_lock_wait: 10,
            status_cache: 3,
            probe_targets: vec!["223.5.5.5".into(), "119.29.29.29".into()],
            probe_targets6: vec!["2400:3200::1".into(), "2402:4e00::".into()],
            gw_required: false,
            dns_check: false,
            tcp_check: false,
            eth_fallback: Vec::new(),
            wan_device: String::new(),
            modem_device: String::new(),
            ipv6_owner: String::new(),
            dns_probe_name: "openwrt.org".into(),
        }
    }
}

/// `cfg_int` with default; `cfg_bool` accepts 1/0/true/false/yes/no/on/off.
impl AppConfig {
    /// Resolve from a raw `settings` section map (already normalized to
    /// "settings" section of the h5000m_netmode config).
    pub fn from_section(
        map: &BTreeMap<String, String>,
        lists: &BTreeMap<String, Vec<String>>,
    ) -> AppConfig {
        let mut c = AppConfig::default();

        let get = |k: &str| map.get(k).map(|s| s.as_str()).unwrap_or("");
        c.mode = Mode::parse(get("mode")).unwrap_or(Mode::WanFirst);
        c.watcher = bool_val(get("watcher"), true);
        c.watch_interval = clamp(num_or(get("watch_interval"), 10), 3, 3600);
        c.health_check = bool_val(get("health_check"), true);
        c.health_probe_interval = parse_u32(get("health_probe_interval"), 60);
        c.strict_dual_stack = bool_val(get("strict_dual_stack"), false);
        c.switch_wait_ipv4 = parse_u32(get("switch_wait_ipv4"), 15);
        c.switch_wait_ipv6 = parse_u32(get("switch_wait_ipv6"), 20);
        c.switch_budget = parse_u32(get("switch_budget"), 60);
        c.switch_settle = parse_u32(get("switch_settle"), 1);
        c.switch_settle_warm = parse_u32(get("switch_settle_warm"), 0);
        c.probe_attempts = parse_u32(get("probe_attempts"), 3).max(1);
        c.probe_ok = parse_u32(get("probe_ok"), 2);
        c.probe_timeout = parse_u32(get("probe_timeout"), 2).max(1);
        c.probe_fail_streak = parse_u32(get("probe_fail_streak"), 3).max(1);
        c.switch_cooldown = parse_u32(get("switch_cooldown"), 20);
        c.align_confirm = parse_u32(get("align_confirm"), 2).max(1);
        c.hotplug_debounce = parse_u32(get("hotplug_debounce"), 2);
        c.reconcile_wait = parse_u32(get("reconcile_wait"), 5).max(1);
        c.switch_lock_wait = parse_u32(get("switch_lock_wait"), 10).max(1);
        c.status_cache = parse_u32(get("status_cache"), 3);
        c.gw_required = bool_val(get("gw_required"), false);
        c.dns_check = bool_val(get("dns_check"), false);
        c.tcp_check = bool_val(get("tcp_check"), false);
        c.wan_device = get("wan_device").to_string();
        c.modem_device = get("modem_device").to_string();
        c.ipv6_owner = get("ipv6_owner").to_string();
        c.dns_probe_name = {
            let v = get("dns_probe_name");
            if v.is_empty() {
                "openwrt.org".to_string()
            } else {
                v.to_string()
            }
        };

        // Targets: option value (space-separated) takes precedence; fall back
        // to the documented defaults so a fresh install probes domestic
        // resolvers even without explicit configuration.
        let split_words =
            |s: &str| -> Vec<String> { s.split_whitespace().map(|w| w.to_string()).collect() };
        c.probe_targets = split_words(get("probe_targets"));
        if c.probe_targets.is_empty() {
            if let Some(l) = lists.get("probe_targets") {
                if !l.is_empty() {
                    c.probe_targets = l.clone();
                }
            }
        }
        if c.probe_targets.is_empty() {
            c.probe_targets = AppConfig::default().probe_targets;
        }
        c.probe_targets6 = split_words(get("probe_targets6"));
        if c.probe_targets6.is_empty() {
            if let Some(l) = lists.get("probe_targets6") {
                if !l.is_empty() {
                    c.probe_targets6 = l.clone();
                }
            }
        }
        if c.probe_targets6.is_empty() {
            c.probe_targets6 = AppConfig::default().probe_targets6;
        }

        if let Some(l) = lists.get("eth_fallback") {
            c.eth_fallback = l.clone();
        } else {
            c.eth_fallback = split_words(get("eth_fallback"));
        }
        c
    }
}

/// `cfg_bool` semantics: 1/true/yes/on -> true; 0/false/no/off -> false.
pub fn bool_val(v: &str, default: bool) -> bool {
    match v.trim() {
        "" => default,
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

impl AppConfig {
    /// Load the live settings from UCI (the single entry point for every
    /// command; unknown options keep their documented defaults).
    pub fn load() -> AppConfig {
        let (opts, lists) = crate::config::uci::settings_section();
        AppConfig::from_section(&opts, &lists)
    }
}

fn clamp(v: u32, lo: u32, hi: u32) -> u32 {
    v.max(lo).min(hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_are_documented() {
        let c = AppConfig::from_section(&BTreeMap::new(), &BTreeMap::new());
        assert_eq!(c.mode, Mode::WanFirst);
        assert_eq!(c.probe_targets, vec!["223.5.5.5", "119.29.29.29"]);
        assert_eq!(c.probe_targets6, vec!["2400:3200::1", "2402:4e00::"]);
        // The dual-stack gate is opt-in since v1.8.3: a missing or broken
        // IPv6 must never block the switch.
        assert!(!c.strict_dual_stack);
        assert_eq!(c.watch_interval, 10);
    }

    #[test]
    fn section_overrides() {
        let m = map(&[
            ("mode", "modem_first"),
            ("watch_interval", "25"),
            ("strict_dual_stack", "1"),
            ("probe_targets", "1.1.1.1 8.8.8.8"),
            ("health_check", "off"),
        ]);
        let c = AppConfig::from_section(&m, &BTreeMap::new());
        assert_eq!(c.mode, Mode::ModemFirst);
        assert_eq!(c.watch_interval, 25);
        assert!(c.strict_dual_stack);
        assert!(!c.health_check);
        assert_eq!(c.probe_targets, vec!["1.1.1.1", "8.8.8.8"]);
    }

    #[test]
    fn watch_interval_clamped() {
        let m = map(&[("watch_interval", "1")]);
        assert_eq!(
            AppConfig::from_section(&m, &BTreeMap::new()).watch_interval,
            3
        );
        let m = map(&[("watch_interval", "99999")]);
        assert_eq!(
            AppConfig::from_section(&m, &BTreeMap::new()).watch_interval,
            3600
        );
    }
}
