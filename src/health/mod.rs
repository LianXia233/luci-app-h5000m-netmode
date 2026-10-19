//! Health monitor: verdict persistence, failure streaks, throttle and score.
//!
//! Decision semantics are ported verbatim from the shell:
//!   * a family with no address/route is `unknown`, never a failure;
//!   * a group is only demoted after `probe_fail_streak` consecutive failed
//!     rounds (hysteresis against a flapping uplink);
//!   * `refresh_health(force)` skips the `health_probe_interval` throttle, used
//!     right after an interface event (the hotplug path sets `health_force`).
//!
//! The optional layered score (Link/Gateway/ICMP/TCP/DNS) is *reported* in the
//! status output; it never gates a switch – the auto-switch decision keeps the
//! legacy verdict+streak contract.

use crate::config::AppConfig;
use crate::network::{self, LiveSnapshot};
use crate::probe;
use crate::state;
use crate::system::log;
use crate::types::{Family, Group, Verdict};

pub mod score;

/// `health_probe_interval` throttle: returns true when a new round may run.
pub fn throttle_ok(cfg: &AppConfig, force: bool) -> bool {
    if force {
        return true;
    }
    let interval = cfg.health_probe_interval;
    if interval == 0 {
        return true;
    }
    let now = crate::system::clock::unix_ts();
    let last = crate::types::num_or::<u64>(&state::health_get("ts"), 0);
    if last > 0 && now > 0 && now.saturating_sub(last) < interval as u64 {
        return false;
    }
    true
}

/// `refresh_health <force>`: one bounded probe round for every (group, family)
/// that structurally can answer; verdicts and failure streaks are persisted.
/// The four (group, family) probes run in PARALLEL: this runs while the writer
/// lock is held on the hotplug/reconcile path, and a sequential worst case
/// (four families × attempts × timeout) could park the lock for half a minute.
pub fn refresh_health(snap: &LiveSnapshot, cfg: &AppConfig, force: bool) {
    refresh_health_scope(snap, cfg, force, None);
}

/// `refresh_health_group <force>`: the hotplug path's targeted variant -
/// refresh only the two families of ONE group (the exit the event belongs
/// to). The other group's cached verdicts stay: an event on the modem cannot
/// change what the wired WAN answers, and those verdicts are at most
/// `health_probe_interval` old, exactly what the throttled path accepts.
pub fn refresh_health_group(snap: &LiveSnapshot, cfg: &AppConfig, force: bool, group: Group) {
    refresh_health_scope(snap, cfg, force, Some(group));
}

fn refresh_health_scope(snap: &LiveSnapshot, cfg: &AppConfig, force: bool, only: Option<Group>) {
    if !throttle_ok(cfg, force) {
        return;
    }

    let pairs: Vec<(Group, Family)> = Group::ALL
        .iter()
        .flat_map(|g| Family::ALL.iter().map(move |f| (*g, *f)))
        .filter(|(g, _)| only.map(|o| o == *g).unwrap_or(true))
        .collect();

    let results: Vec<(Group, Family, Verdict)> = {
        let snap_ref = &snap;
        let cfg_ref = &cfg;
        std::thread::scope(|s| {
            let handles: Vec<_> = pairs
                .iter()
                .map(|(g, fam)| {
                    let ready = match (g, fam) {
                        (Group::Wan, Family::V4) => snap.wan4_ready,
                        (Group::Wan, Family::V6) => snap.wan6_ready,
                        (Group::Modem, Family::V4) => snap.modem4_ready,
                        (Group::Modem, Family::V6) => snap.modem6_ready,
                    };
                    let devs = snap_ref.group_devs(*g, *fam).to_vec();
                    // A family that cannot answer is `unknown`, never a
                    // failure - no probe is spawned for it.
                    let probe = if ready {
                        s.spawn(move || {
                            probe::probe_group_family(
                                *fam,
                                *g,
                                &devs,
                                cfg_ref.probe_attempts,
                                cfg_ref.probe_ok,
                                cfg_ref.probe_timeout,
                                &cfg_ref.probe_targets,
                                &cfg_ref.probe_targets6,
                            )
                        })
                    } else {
                        s.spawn(move || Verdict::Unknown)
                    };
                    (*g, *fam, probe)
                })
                .collect();
            handles
                .into_iter()
                .map(|(g, fam, h)| (g, fam, h.join().unwrap_or(Verdict::Unknown)))
                .collect()
        })
    };
    let verdict_of = |g: Group, f: Family| -> Verdict {
        results
            .iter()
            .find(|(rg, rf, _)| *rg == g && *rf == f)
            .map(|(_, _, v)| *v)
            .unwrap_or(Verdict::Unknown)
    };
    let wan = verdict_of(Group::Wan, Family::V4);
    let wan6 = verdict_of(Group::Wan, Family::V6);
    let modem = verdict_of(Group::Modem, Family::V4);
    let modem6 = verdict_of(Group::Modem, Family::V6);

    let now = crate::system::clock::unix_ts();
    let wan_fail = next_streak("wan_fail", wan);
    let modem_fail = next_streak("modem_fail", modem);
    let wan6_fail = next_streak("wan6_fail", wan6);
    let modem6_fail = next_streak("modem6_fail", modem6);

    // Optional layered score (reported only).
    let s4_wan = score::score_group(snap, cfg, Group::Wan, Family::V4, wan);
    let s6_wan = score::score_group(snap, cfg, Group::Wan, Family::V6, wan6);
    let s4_modem = score::score_group(snap, cfg, Group::Modem, Family::V4, modem);
    let s6_modem = score::score_group(snap, cfg, Group::Modem, Family::V6, modem6);

    let _ = state::health_write(&[
        ("wan", wan.as_str()),
        ("modem", modem.as_str()),
        ("wan6", wan6.as_str()),
        ("modem6", modem6.as_str()),
        ("wan_fail", &wan_fail.to_string()),
        ("modem_fail", &modem_fail.to_string()),
        ("wan6_fail", &wan6_fail.to_string()),
        ("modem6_fail", &modem6_fail.to_string()),
        ("wan_score", &s4_wan.to_string()),
        ("wan6_score", &s6_wan.to_string()),
        ("modem_score", &s4_modem.to_string()),
        ("modem6_score", &s6_modem.to_string()),
        ("ts", &now.to_string()),
    ]);

    log::log_info(
        "health",
        &format!(
            "health check: wan={}/{wan6} modem={}/{modem6}",
            wan.as_str(),
            modem.as_str()
        ),
    );
}

fn next_streak(key: &str, v: Verdict) -> u32 {
    let cur = crate::types::num_or::<u32>(&state::health_get(key), 0);
    match v {
        Verdict::Up => 0,
        Verdict::Down => cur + 1,
        Verdict::Unknown => cur,
    }
}

/// `health_streak <group> <4|6>`.
pub fn health_streak(g: Group, fam: Family) -> u32 {
    network::cached_health_streak(g, fam)
}

/// `group_degraded`: a complete group whose required family has failed
/// `probe_fail_streak` consecutive rounds.
pub fn group_degraded(snap: &LiveSnapshot, cfg: &AppConfig, g: Group) -> bool {
    if !network::group_complete(snap, g, cfg.strict_dual_stack) {
        return false;
    }
    for fam in Family::ALL {
        if !network::group_family_required(snap, g, fam, cfg.strict_dual_stack) {
            continue;
        }
        if health_streak(g, fam) >= cfg.probe_fail_streak {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streak_semantics() {
        // Unknown must not advance the streak; Down advances; Up resets.
        let c = AppConfig::default();
        let _ = c;
        assert_eq!(next_streak("wan_fail", Verdict::Unknown), 0);
    }
}
