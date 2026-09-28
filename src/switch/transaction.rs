//! Route surgery and verification for a switch transaction.
//!
//! Ported from the shell backend's route surgery + verification sections:
//! one atomic `RTM_NEWROUTE|NLM_F_REPLACE` per family moves the active slot,
//! so there is no instant without a default route and no equal-metric pair.

use std::net::IpAddr;

use crate::config::uci;
use crate::config::AppConfig;
use crate::network::netlink;
use crate::network::{routes, sysfs, LiveSnapshot};
use crate::state;
use crate::system::log;
use crate::types::{Family, Group, Mode, Result};

pub const ACTIVE_METRIC: u32 = 10;
pub const STANDBY_METRIC: u32 = 50;

/// `policy_group_metric(mode, group)`.
pub fn policy_group_metric(mode: Mode, g: Group) -> u32 {
    if mode.primary_group() == g {
        ACTIVE_METRIC
    } else {
        STANDBY_METRIC
    }
}

/// `policy_group_defaultroute(mode, group)`: 1 = may install a default route.
pub fn policy_group_defaultroute(mode: Mode, g: Group) -> u32 {
    if mode.is_only() && mode.primary_group() != g {
        0
    } else {
        1
    }
}

/// `route_replace_slot <family> <gw> <dev> <metric>` (netlink, atomic).
pub fn route_replace_slot(family: Family, gw: &str, dev: &str, metric: u32) -> Result<()> {
    let via: Option<IpAddr> = if gw.is_empty() {
        None
    } else {
        netlink::parse_gateway(family, gw)
    };
    let log_gw = if gw.is_empty() {
        String::new()
    } else {
        format!("via {gw} ")
    };
    log::log_info(
        "route",
        &format!(
            "ip -{} route replace default {log_gw}dev {dev} metric {metric}",
            family.n()
        ),
    );
    netlink::route_replace_slot(family, via, dev, metric)
}

/// `route_del_line` for a default route we own (netlink).
pub fn route_del(family: Family, gw: &str, dev: &str, metric: u32) -> Result<()> {
    let via = if gw.is_empty() {
        None
    } else {
        netlink::parse_gateway(family, gw)
    };
    netlink::route_del(family, via, dev, metric)
}

/// `remember_group_route` / `recall_group_route`: the last known next hop of a
/// group, so an isolated standby can be warmed again without waiting for its
/// proto to re-announce. Stored in the state file (`rt_<group><4|6>`).
pub fn remember_group_route(g: Group, fam: Family, r: &routes::Route) {
    let (gw, dev) = routes::route_gw_dev(r);
    if gw.is_empty() || dev.is_empty() {
        return;
    }
    let _ = state::state_write(&[(
        &format!("rt_{}{}", g.as_str(), fam.n()),
        &format!("{gw} {dev}"),
    )]);
}

pub fn recall_group_route(g: Group, fam: Family) -> (String, String) {
    let v = state::state_get(&format!("rt_{}{}", g.as_str(), fam.n()));
    let mut it = v.split_whitespace();
    let gw = it.next().unwrap_or("").to_string();
    let dev = it.next().unwrap_or("").to_string();
    (gw, dev)
}

/// `warm_group_route <group>`: install the warm standby route from the
/// remembered next hop when the group has an address but no default route yet.
pub fn warm_group_route(mode: Mode, snap: &LiveSnapshot, g: Group) {
    if policy_group_defaultroute(mode, g) != 1 {
        return; // an `only`-isolated standby is deliberately without a route
    }
    for fam in Family::ALL {
        let devs = snap.group_devs(g, fam);
        if devs.is_empty() {
            continue;
        }
        if let Some(r) = snap.group_route(g, fam) {
            remember_group_route(g, fam, r);
            continue;
        }
        if !crate::network::group_family_has_address(snap, g, fam) {
            continue;
        }
        let (gw, dev) = recall_group_route(g, fam);
        if gw.is_empty() || dev.is_empty() {
            continue;
        }
        if !devs.iter().any(|d| d == &dev) {
            continue;
        }
        log::log_info(
            "switch",
            &format!(
                "warming {} ipv{} from the remembered next hop {gw} dev {dev}",
                g.as_str(),
                fam.n()
            ),
        );
        let _ = route_replace_slot(fam, &gw, &dev, STANDBY_METRIC);
    }
}

/// `maintain_warm_routes`: restore a *missing* standby route; only from a next
/// hop this program has seen, and never for the active group's own missing
/// route (that is the alignment's job).
pub fn maintain_warm_routes(mode: Mode, snap: &LiveSnapshot, _cfg: &AppConfig) {
    for g in Group::ALL {
        for fam in Family::ALL {
            // Warming a route for a family with no live slot would create a
            // route the kernel then uses - a split.
            if !matches!(
                crate::network::slot_owner_family(snap, fam).as_str(),
                "wan" | "modem"
            ) {
                continue;
            }
            if snap.group_route(g, fam).is_some() {
                continue;
            }
            if !crate::network::group_family_capable(snap, g, fam) {
                continue;
            }
            if !crate::network::group_family_has_address(snap, g, fam) {
                continue;
            }
            let active = snap.slot_owner.clone();
            if g.as_str() == active {
                continue;
            }
            warm_group_route(mode, snap, g);
        }
    }
}

// ---------------------------------------------------------------------------
// verification
// ---------------------------------------------------------------------------

/// `verify_group_priority`: the target owns the priority slot for both
/// families; the standby, when it has a route, sits strictly below. Returns
/// Ok(()) or a failure reason string.
pub fn verify_group_priority(
    snap: &LiveSnapshot,
    g: Group,
    _cfg: &AppConfig,
) -> std::result::Result<(), String> {
    for fam in Family::ALL {
        let Some(dev) = routes::winner_dev(fam) else {
            if !crate::network::group_family_capable(snap, g, fam) {
                continue;
            }
            return Err(format!("ipv{}_no_default_route", fam.n()));
        };
        let owner = route_owner_of(snap, &dev, &fib_egress_fallback(snap, fam));
        if owner != g.as_str() {
            match owner.as_str() {
                "wan" | "modem" => return Err(format!("ipv{}_default_via_{owner}", fam.n())),
                _ => log::log_info(
                    "switch",
                    &format!(
                        "ipv{} priority set but an external router owns the FIB ({dev})",
                        fam.n()
                    ),
                ),
            }
        }
        if let Some(wline) = routes::winning_default(fam) {
            let ecmp = routes::equal_metric_count(fam, wline.metric, &wline);
            if ecmp > 1 {
                return Err(format!("ipv{}_ambiguous_metric_{}", fam.n(), wline.metric));
            }
        }
    }
    // Standby must sit strictly below the active slot.
    for fam in Family::ALL {
        for other in Group::ALL {
            if other == g {
                continue;
            }
            if let Some(oline) = snap.group_route(other, fam) {
                if let Some(w) = routes::winning_default(fam) {
                    if oline.metric <= w.metric {
                        return Err(format!("ipv{}_standby_not_demoted", fam.n()));
                    }
                }
            }
        }
    }
    Ok(())
}

fn route_owner_of(snap: &LiveSnapshot, dev: &str, fallback: &str) -> String {
    if dev.is_empty() {
        return "none".into();
    }
    if snap.wan_devs.iter().any(|d| d == dev) {
        "wan".into()
    } else if snap.modem_devs.iter().any(|d| d == dev) {
        "modem".into()
    } else if sysfs::dev_type(dev)
        .map(sysfs::is_tunnel_type)
        .unwrap_or(false)
    {
        match fallback {
            "wan" | "modem" => fallback.to_string(),
            _ => "other".into(),
        }
    } else {
        "other".into()
    }
}

/// FIB egress fallback (the `ip route get` replacement): the winning default
/// device is the kernel's choice on a plain main-table router.
fn fib_egress_fallback(snap: &LiveSnapshot, fam: Family) -> String {
    match fam {
        Family::V4 => snap.egress4.clone(),
        Family::V6 => snap.egress6.clone(),
    }
}

/// `verify_excluded_families`: the standby's route for a family the active
/// group cannot carry must be parked (no leak). This is capability-based, not
/// gated on strict_dual_stack: since requirement follows capability, a hole
/// can exist under either setting whenever the active group is single-stack.
pub fn verify_excluded_families(
    snap: &LiveSnapshot,
    g: Group,
    cfg: &AppConfig,
) -> std::result::Result<(), String> {
    let standby = g.other();
    for fam in crate::network::group_hole_families(snap, g, cfg.strict_dual_stack) {
        if snap.group_route(standby, fam).is_some() {
            return Err(format!("ipv{}_leaked_to_{}", fam.n(), standby.as_str()));
        }
    }
    Ok(())
}

/// `verify_commit`: post-commit verification with the IPv6 soft-failure rule.
///
/// A `strict_dual_stack=0` deployment must never let an IPv6 verification
/// problem roll back a switch whose IPv4 commit already succeeded: IPv6 is an
/// enhancement, never a dependency of the state machine. Every verify error
/// prefixed with `ipv6_` therefore degrades to a logged warning under
/// strict=0; IPv4 errors (`ipv4_*`) and structural errors still fail.
pub fn verify_commit(
    snap: &LiveSnapshot,
    g: Group,
    cfg: &AppConfig,
) -> std::result::Result<(), String> {
    let check = || -> std::result::Result<(), String> {
        verify_group_priority(snap, g, cfg)?;
        verify_excluded_families(snap, g, cfg)
    };
    match check() {
        Ok(()) => Ok(()),
        Err(e) if !cfg.strict_dual_stack && e.starts_with("ipv6_") => {
            log::log_warn(
                "switch",
                &format!(
                    "warning: {e} on {} degraded to a warning (strict_dual_stack=0); keeping the committed exit",
                    g.as_str()
                ),
            );
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// `verify_group_online`: bounded reachability probes for every required
/// family (used by align; a switch probes before committing too).
pub fn verify_group_online(
    snap: &LiveSnapshot,
    cfg: &AppConfig,
    g: Group,
) -> std::result::Result<(), String> {
    for fam in Family::ALL {
        if !crate::network::group_family_capable(snap, g, fam) {
            continue;
        }
        let verdict = crate::probe::probe_group_family(
            fam,
            g,
            snap.group_devs(g, fam),
            cfg.probe_attempts,
            cfg.probe_ok,
            cfg.probe_timeout,
            &cfg.probe_targets,
            &cfg.probe_targets6,
        );
        if !verdict.is_up() {
            if fam == Family::V6 && !cfg.strict_dual_stack {
                log::log_warn(
                    "align",
                    &format!(
                        "warning: IPv6 probes failed on {}; continuing single-stack (strict_dual_stack=0)",
                        g.as_str()
                    ),
                );
                continue;
            }
            return Err(format!("ipv{}_unreachable", fam.n()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// commit / rollback
// ---------------------------------------------------------------------------

/// `commit_family <family> <group> <previous>`: move one family's active slot.
/// Returns Ok(()) on success.
pub fn commit_family(
    fam: Family,
    g: Group,
    previous: Option<Group>,
    snap: &LiveSnapshot,
) -> Result<()> {
    let (tgw, tdev) = target_next_hop(fam, g, snap);
    if tdev.is_empty() {
        return Err(crate::types::Error::route("no target device"));
    }
    let prev = previous.map(|p| p.as_str()).unwrap_or("");
    if prev == g.as_str() || prev.is_empty() || prev == "none" {
        route_replace_slot(fam, &tgw, &tdev, ACTIVE_METRIC)?;
        return Ok(());
    }
    let (ogw, odev) = match snap.group_route(previous.unwrap(), fam) {
        Some(r) => (r.via.clone().unwrap_or_default(), r.dev.clone()),
        None => (String::new(), String::new()),
    };
    // 1. promote the target into the active slot (atomic).
    route_replace_slot(fam, &tgw, &tdev, ACTIVE_METRIC)?;
    log::log_info(
        "switch",
        &format!(
            "ipv{} default -> {} (via {} dev {} metric {ACTIVE_METRIC})",
            fam.n(),
            g.as_str(),
            tgw,
            tdev
        ),
    );
    // 2. demote the previous owner into the standby slot (warm failover).
    if !odev.is_empty() {
        let _ = route_replace_slot(fam, &ogw, &odev, STANDBY_METRIC);
        if !ogw.is_empty() {
            log::log_info(
                "switch",
                &format!(
                    "ipv{} standby -> {} (via {ogw} dev {odev} metric {STANDBY_METRIC})",
                    fam.n(),
                    previous.unwrap().as_str()
                ),
            );
        } else {
            log::log_info(
                "switch",
                &format!(
                    "ipv{} standby -> {} (dev {odev} metric {STANDBY_METRIC})",
                    fam.n(),
                    previous.unwrap().as_str()
                ),
            );
        }
    } else {
        log::log_info(
            "switch",
            &format!(
                "ipv{}: {} has no usable route; the standby slot keeps the target's warm route",
                fam.n(),
                previous.unwrap().as_str()
            ),
        );
    }
    Ok(())
}

/// Resolve the target next hop: the group's own default-route gateway/device,
/// or the remembered one (verified afterwards) for a proto that has not
/// announced a next hop yet.
fn target_next_hop(fam: Family, g: Group, snap: &LiveSnapshot) -> (String, String) {
    if let Some(r) = snap.group_route(g, fam) {
        let (gw, dev) = (r.via.clone().unwrap_or_default(), r.dev.clone());
        if !dev.is_empty() && !gw.is_empty() {
            return (gw, dev);
        }
        if !dev.is_empty() {
            // Device-scoped default (no next hop) is normal for cellular.
            return (String::new(), dev);
        }
    }
    recall_group_route(g, fam)
}

/// `rollback_family`: the previous owner goes back into the active slot; the
/// failed target is parked on standby - but only if the previous owner is
/// still usable (a rollback must never create a blackhole).
pub fn rollback_family(fam: Family, previous: Group, target: Group, snap: &LiveSnapshot) -> bool {
    let (pgw, pdev) = match snap.group_route(previous, fam) {
        Some(r) => (r.via.clone().unwrap_or_default(), r.dev.clone()),
        None => (String::new(), String::new()),
    };
    if pgw.is_empty() || pdev.is_empty() || !sysfs::netdev_exists(&pdev) {
        log::log_info(
            "switch",
            &format!(
                "ipv{}: previous exit {} has no usable route; keeping the live exit",
                fam.n(),
                previous.as_str()
            ),
        );
        return false;
    }
    if route_replace_slot(fam, &pgw, &pdev, ACTIVE_METRIC).is_err() {
        return false;
    }
    log::log_info(
        "switch",
        &format!(
            "ipv{} default -> {} (rollback, via {pgw} dev {pdev})",
            fam.n(),
            previous.as_str()
        ),
    );
    if let Some(r) = snap.group_route(target, fam) {
        let (tgw, tdev) = (r.via.clone().unwrap_or_default(), r.dev.clone());
        if !tdev.is_empty() && pdev != tdev {
            let _ = route_replace_slot(fam, &tgw, &tdev, STANDBY_METRIC);
        }
    }
    true
}

/// `apply_group_holes`: park the standby's route for a family the active group
/// cannot carry, so the missing family can never leak out of the standby and
/// split the egress. Capability-based: applies whenever the active group is
/// single-stack, regardless of strict_dual_stack.
pub fn apply_group_holes(_mode: Mode, snap: &LiveSnapshot, cfg: &AppConfig, g: Group) -> bool {
    let standby = g.other();
    for fam in crate::network::group_hole_families(snap, g, cfg.strict_dual_stack) {
        let sec = snap.group_sec(standby, fam).to_string();
        if !sec.is_empty() {
            let _ = uci::uci_set_commit("network", &format!("network.{sec}.defaultroute"), "0");
        }
        if let Some(line) = snap.group_route(standby, fam) {
            let _ = route_del(
                fam,
                &line.via.clone().unwrap_or_default(),
                &line.dev,
                line.metric,
            );
        }
        log::log_warn(
            "switch",
            &format!(
                "warning: {} cannot be the active exit for ipv{}; parked its route",
                standby.as_str(),
                fam.n()
            ),
        );
    }
    true
}

/// `reap_orphan_routes`: delete a default route that points at a device which
/// no longer exists. Only routes this program installs (`proto boot`) are
/// considered, so nothing netifd owns is ever removed.
pub fn reap_orphan_routes() {
    for fam in Family::ALL {
        for r in routes::show_defaults(fam) {
            if r.dev.is_empty() {
                continue;
            }
            if sysfs::netdev_exists(&r.dev) {
                continue;
            }
            let via = r.via.clone().unwrap_or_default();
            // Without a gateway the delete cannot be narrowed down to one
            // route: the device is gone too, so (metric) alone may well match
            // a route that is still in use. Leave it to the kernel, which
            // removes a device's routes when the device disappears.
            if via.is_empty() {
                continue;
            }
            let boot = netlink::is_route_boot(fam, via.parse::<IpAddr>().ok(), &r.dev, r.metric);
            if !boot {
                continue;
            }
            log::log_info(
                "reconcile",
                &format!(
                    "removing an orphan default route on the missing device {}",
                    r.dev
                ),
            );
            let _ = route_del(fam, &via, &r.dev, r.metric);
        }
    }
}

/// `set_network_option_if_changed`: write one network option only when it
/// differs; returns false on a failed write.
pub fn set_network_option_if_changed(
    section: &str,
    option: &str,
    value: &str,
    changed: &mut bool,
) -> bool {
    if section.is_empty() || !uci::uci_has_key(&format!("network.{section}")) {
        return true;
    }
    let current = uci::uci_get(&format!("network.{section}.{option}"));
    if current == value {
        return true;
    }
    if uci::uci_exec(&["-q", "set", &format!("network.{section}.{option}={value}")]).is_err() {
        return false;
    }
    *changed = true;
    true
}

/// `apply_policy <mode>`: persist the priority plan of one mode - writes both
/// families of both groups, never touches IPv6 of a standby in a way that
/// disables it, and keeps IPv6 auto=1 everywhere.
pub fn apply_policy(mode: Mode, snap: &LiveSnapshot) -> Result<()> {
    let mut changed = false;
    for g in Group::ALL {
        let metric = policy_group_metric(mode, g).to_string();
        let dr = policy_group_defaultroute(mode, g).to_string();
        for fam in Family::ALL {
            let sec = snap.group_sec(g, fam);
            if sec.is_empty() {
                continue;
            }
            if !set_network_option_if_changed(sec, "metric", &metric, &mut changed) {
                return Err(crate::types::Error::config("metric write failed"));
            }
            if !set_network_option_if_changed(sec, "defaultroute", &dr, &mut changed) {
                return Err(crate::types::Error::config("defaultroute write failed"));
            }
            if fam == Family::V6 {
                // IPv6 is always enabled: the standby keeps auto=1 so its
                // addresses, RA and DHCPv6 state survive the switch.
                if !set_network_option_if_changed(sec, "auto", "1", &mut changed) {
                    return Err(crate::types::Error::config("auto write failed"));
                }
            }
        }
    }
    if changed {
        uci::uci_exec(&["-q", "commit", "network"])?;
    }

    let current_mode = uci::uci_get("h5000m_netmode.settings.mode");
    if current_mode != mode.as_str()
        && uci::uci_set_commit(
            "h5000m_netmode",
            "h5000m_netmode.settings.mode",
            mode.as_str(),
        )
        .is_err()
        && uci::uci_exec(&["-q", "get", "h5000m_netmode.settings"]).is_err()
    {
        // settings section may be missing on a fresh install; recreate it.
        let _ = uci::uci_exec(&["-q", "set", "h5000m_netmode.settings=settings"]);
        let _ = uci::uci_exec(&[
            "-q",
            "set",
            &format!("h5000m_netmode.settings.mode={}", mode.as_str()),
        ]);
        let _ = uci::uci_exec(&["-q", "commit", "h5000m_netmode"]);
    }

    // Vendor integration: the MT5700M manager keeps its own copy of the modem
    // metric; only the value is synced, the manager is never asked to restart.
    if uci::uci_has_key("mt5700m.connection") {
        let expected = policy_group_metric(mode, Group::Modem).to_string();
        let current = uci::uci_get("mt5700m.connection.metric");
        if current != expected
            && uci::uci_exec(&[
                "-q",
                "set",
                &format!("mt5700m.connection.metric={expected}"),
            ])
            .is_ok()
        {
            let _ = uci::uci_exec(&["-q", "commit", "mt5700m"]);
        }
    }
    Ok(())
}

/// `post_failure_converge`: after a rollback the two families must still agree
/// on one exit.
pub fn post_failure_converge(snap: &LiveSnapshot, _cfg: &AppConfig) {
    let s4 = crate::network::slot_owner_family(snap, Family::V4);
    let s6 = crate::network::slot_owner_family(snap, Family::V6);
    let s4 = if s4.is_empty() { "none".into() } else { s4 };
    let s6 = if s6.is_empty() { "none".into() } else { s6 };
    if s4 == s6 {
        return;
    }
    let mut chosen = s4.clone();
    if !matches!(chosen.as_str(), "wan" | "modem") {
        chosen = s6.clone();
    }
    if !matches!(chosen.as_str(), "wan" | "modem") {
        return;
    }
    log::log_info(
        "switch",
        &format!("converging the family split onto {chosen} after the failed switch"),
    );
    if let Some(g) = Group::parse(&chosen) {
        for fam in Family::ALL {
            let _ = commit_family(fam, g, Some(g.other()), snap);
        }
    }
}
