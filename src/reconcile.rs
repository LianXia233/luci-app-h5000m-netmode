//! The reconciler: the single decision point the watchdog and the hotplug path
//! share. One pass decides whether the exit needs to move, why, and to where;
//! the switch transaction itself is delegated to `switch::switch_run` (kind
//! `align`) so a user request and an automatic repair can never race.

use crate::config::uci;
use crate::config::AppConfig;
use crate::network::{self, LiveSnapshot};
use crate::state;
use crate::switch::transaction::{self, commit_family};
use crate::system::clock::unix_ts;
use crate::system::log;
use crate::types::{Family, Group, Result, SmState};

/// `switch_cooldown`: minimum seconds between automatic alignments.
pub fn switch_cooldown(cfg: &AppConfig) -> u32 {
    cfg.switch_cooldown
}

/// `align_cooldown_ok`: a forced align (UI one-click, `H5000M_FORCE_ALIGN=1`)
/// bypasses the rate limit - a split is a fault and waiting would be wrong.
/// The cooldown is measured from the last ATTEMPT (not the last success): a
/// failing align used to retry on every watchdog tick without ever consuming
/// the cooldown, re-probing a dead target every `watch_interval` seconds.
pub fn align_cooldown_ok(cfg: &AppConfig, force: bool) -> bool {
    if force {
        return true;
    }
    let now = unix_ts();
    let last = crate::types::num_or::<u64>(&state::state_get("last_align_attempt_ts"), 0);
    now.saturating_sub(last) >= switch_cooldown(cfg) as u64
}

/// `mark_align`: persist the audit trail of a SUCCESSFUL alignment (kept for
/// the UI) and stamp the attempt clock (used by the cooldown).
pub fn mark_align(reason: &str, target: &str) {
    let _ = state::state_write(&[
        ("last_align_ts", &unix_ts().to_string()),
        ("last_align_attempt_ts", &unix_ts().to_string()),
        ("last_align_reason", reason),
        ("last_align_target", target),
    ]);
}

/// `mark_align_attempt`: stamp the attempt clock at the START of an align so
/// failed attempts also consume the cooldown (see `align_cooldown_ok`).
fn mark_align_attempt() {
    let _ = state::state_write(&[("last_align_attempt_ts", &unix_ts().to_string())]);
}

/// `confirm_candidate`: consecutive observations must agree before a failback
/// promotes the primary (a flapping primary must not cause a switch loop).
pub fn confirm_candidate(cfg: &AppConfig, group: Group) -> bool {
    let cand = state::state_get("align_candidate");
    let hits = crate::types::num_or::<u32>(&state::state_get("align_candidate_hits"), 0);
    let hits = if cand == group.as_str() { hits + 1 } else { 1 };
    let _ = state::state_write(&[
        ("align_candidate", group.as_str()),
        ("align_candidate_hits", &hits.to_string()),
    ]);
    hits >= cfg.align_confirm
}

/// `clear_candidate`.
pub fn clear_candidate() {
    let hits = crate::types::num_or::<u32>(&state::state_get("align_candidate_hits"), 0);
    if hits > 0 {
        let _ = state::state_write(&[("align_candidate_hits", "0")]);
    }
}

/// `remember_routes`: snapshot the current next hops so an isolated standby can
/// be warmed later. One read + one write: the previous version did up to four
/// separate read-rewrite cycles of the whole state file on every reconcile
/// tick even when nothing had changed.
pub fn remember_routes(snap: &LiveSnapshot) {
    let current = state::kv_read(&state::state_file());
    let mut stale: Vec<(String, String)> = Vec::new();
    for g in Group::ALL {
        for fam in Family::ALL {
            let Some(r) = snap.group_route(g, fam) else {
                continue;
            };
            let (gw, dev) = crate::network::routes::route_gw_dev(r);
            if gw.is_empty() || dev.is_empty() {
                continue;
            }
            let key = format!("rt_{}{}", g.as_str(), fam.n());
            let val = format!("{gw} {dev}");
            if current.get(&key).map(|v| v.as_str()) != Some(val.as_str()) {
                stale.push((key, val));
            }
        }
    }
    if stale.is_empty() {
        return;
    }
    let pairs: Vec<(&str, &str)> = stale
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let _ = state::state_write(&pairs);
}

/// Seconds to let netifd re-evaluate an interface after a parked default
/// route is re-opened. Bounded: an alignment must never grow into a switch.
const ALIGN_RELOAD_SETTLE_S: u32 = 2;

/// `align_to <group> <reason>`: probe, commit both families, verify; on any
/// failure restore the previous owner.
pub fn align_to(
    cfg: &AppConfig,
    group: Group,
    reason: &str,
    snap: &LiveSnapshot,
    force: bool,
) -> Result<()> {
    let previous = group.other();
    if !align_cooldown_ok(cfg, force) {
        log::log_info(
            "reconcile",
            &format!("alignment to {} deferred (cooldown)", group.as_str()),
        );
        return Err(crate::types::Error::switch("cooldown"));
    }
    mark_align_attempt();

    // Put the target back on its configured policy before judging it. A
    // previous rescue parks the standby by writing `defaultroute=0`, and an
    // interface whose default route is parked can never become ready - so an
    // alignment back to WAN could previously only succeed through a manual
    // switch (which is exactly what `prepare_group` does for `switch_run`).
    crate::switch::prepare_group(group, cfg.mode, snap);

    // `prepare_group` only ifups a section that is down or has no device. A
    // family that already holds an address but has no default route is the
    // signature of a parked option, and netifd must re-evaluate that section
    // for the re-opened `defaultroute=1` to reach the FIB.
    let mut reloaded = false;
    for fam in Family::ALL {
        if !network::group_family_required(snap, group, fam, cfg.strict_dual_stack) {
            continue;
        }
        let sec = snap.group_sec(group, fam).to_string();
        if sec.is_empty() || snap.group_route(group, fam).is_some() {
            continue;
        }
        if network::group_family_has_address(snap, group, fam) {
            log::log_info(
                "reconcile",
                &format!("bringing {sec} up so its default route is re-installed"),
            );
            let _ = crate::system::command::ifup_section(&sec);
            reloaded = true;
        }
    }
    if reloaded {
        std::thread::sleep(std::time::Duration::from_secs(ALIGN_RELOAD_SETTLE_S as u64));
    }

    // Judge the target on a fresh view: the policy write above and the ifup
    // both postdate the caller's snapshot.
    let fresh = network::read_live_state();
    if let Err(e) = transaction::verify_group_online(&fresh, cfg, group) {
        log::log_info(
            "reconcile",
            &format!("alignment to {} aborted: {}", group.as_str(), e),
        );
        return Err(crate::types::Error::switch(e));
    }

    // Task bookkeeping identical to a user switch (LuCI reads these fields).
    let _ = state::state_write(&[
        ("state", SmState::SWITCHING),
        ("kind", "align"),
        ("target", group.as_str()),
        ("target_mode", cfg.mode.as_str()),
        ("result", "none"),
        ("pid", &std::process::id().to_string()),
        ("started", &unix_ts().to_string()),
    ]);
    log::log_info(
        "reconcile",
        &format!("align -> {} ({reason})", group.as_str()),
    );

    let mut ok = true;
    for fam in Family::ALL {
        if !network::group_family_required(&fresh, group, fam, cfg.strict_dual_stack) {
            continue;
        }
        if commit_family(fam, group, Some(previous), &fresh).is_err() {
            if fam == Family::V6 && !cfg.strict_dual_stack {
                log::log_warn(
                    "reconcile",
                    &format!(
                        "warning: IPv6 commit onto {} failed; continuing single-stack (strict_dual_stack=0)",
                        group.as_str()
                    ),
                );
                continue;
            }
            ok = false;
            break;
        }
    }

    // Park the standby's route for every family the active group cannot
    // carry. Without this the align/rescue path leaves the standby's IPv6
    // default in place and the families split across uplinks - the very
    // thing the switch state machine's apply_group_holes prevents. Only a
    // target with a committed family may park anything: a vacuous target
    // (`ok == false`, no section) must never remove the working exit.
    if ok && !transaction::apply_group_holes(cfg.mode, &fresh, cfg, group) {
        ok = false;
    }

    let _ = state::state_write(&[
        ("state", SmState::VERIFY_TARGET),
        ("message", &format!("verifying {}", group.as_str())),
    ]);
    // verify_commit keeps an IPv4-only commit alive when IPv6 verification
    // fails under strict_dual_stack=0 (IPv6 is an enhancement, not a gate).
    // The view must be taken AFTER the commit and the hole parking: judging a
    // pre-surgery snapshot reported the just-deleted standby route as a leak.
    let post = network::read_live_state();
    if ok && transaction::verify_commit(&post, group, cfg).is_ok() {
        mark_align(reason, group.as_str());
        let _ = state::state_write(&[
            ("state", SmState::COMMITTED),
            ("result", "ok"),
            ("reason", ""),
            (
                "message",
                &format!("aligned to {} ({reason})", group.as_str()),
            ),
            ("pid", ""),
        ]);
        clear_candidate();
        crate::health::refresh_health(&post, cfg, false);
        return Ok(());
    }

    let _ = state::state_write(&[
        ("state", SmState::ROLLBACK),
        (
            "message",
            &format!("restoring {} after a failed alignment", previous.as_str()),
        ),
    ]);
    for fam in Family::ALL {
        let _ = commit_family(fam, previous, Some(group), &fresh);
    }
    transaction::post_failure_converge(&network::read_live_state(), cfg);
    let _ = state::state_write(&[
        ("state", SmState::FAILED),
        ("result", "failed"),
        ("reason", "align_verify_failed"),
        (
            "message",
            &format!(
                "alignment to {} failed; restored {}",
                group.as_str(),
                previous.as_str()
            ),
        ),
        ("pid", ""),
    ]);
    crate::health::refresh_health(&network::read_live_state(), cfg, false);
    Err(crate::types::Error::switch("align_verify_failed"))
}

/// `write_ipv6_owner`: the audit field the LuCI page reads to flag a split.
pub fn write_ipv6_owner(value: &str) {
    let value = match value {
        "wan" | "modem" | "split" | "none" => value,
        _ => "none",
    };
    let previous = uci::uci_get("h5000m_netmode.settings.ipv6_owner");
    if previous == value {
        return;
    }
    if uci::uci_set_commit(
        "h5000m_netmode",
        "h5000m_netmode.settings.ipv6_owner",
        value,
    )
    .is_err()
    {
        return;
    }
    log::log_info(
        "reconcile",
        &format!(
            "ipv6 exit {}->{}",
            if previous.is_empty() {
                "none"
            } else {
                &previous
            },
            value
        ),
    );
}

/// Restore the policy primary's own `defaultroute` when it was left parked.
///
/// `apply_group_holes` writes `defaultroute=0` into UCI for the standby's
/// families. If the "active" group was one that did not exist yet, that
/// parked the wired WAN itself; the option survives reboots and netifd then
/// never installs the WAN default route again. The interface still gets a
/// DHCP lease, but every readiness predicate (address + route) stays false,
/// health reports `unknown` forever and no rescue can ever move back to it
/// (issue #4).
///
/// Only the policy primary's explicit `defaultroute=0` is reopened: the
/// standby's holes are deliberate and must stay closed, so this can never
/// fight `apply_group_holes`. An absent option already means 1 (netifd's
/// default), so a stock config is not rewritten.
fn repair_primary_policy(cfg: &AppConfig, snap: &LiveSnapshot) -> bool {
    let primary = cfg.mode.primary_group();
    let mut lines: Vec<String> = Vec::new();
    let mut sections: Vec<String> = Vec::new();
    let mut reopened: Vec<(Family, String)> = Vec::new();
    for fam in Family::ALL {
        let sec = snap.group_sec(primary, fam);
        if sec.is_empty() || !uci::uci_has_key(&format!("network.{sec}")) {
            continue;
        }
        let current = uci::uci_get(&format!("network.{sec}.defaultroute"));
        if crate::config::bool_val(&current, true) {
            continue;
        }
        lines.push(format!("set network.{sec}.defaultroute=1"));
        sections.push(sec.to_string());
        reopened.push((fam, sec.to_string()));
    }
    if lines.is_empty() {
        return false;
    }
    if let Err(e) = transaction::apply_uci_lines(&lines, "network") {
        log::log_warn(
            "reconcile",
            &format!("failed to restore the parked primary default route: {e}"),
        );
        return false;
    }
    // `uci commit` only rewrites the file; netifd re-reads `defaultroute` on
    // the next interface evaluation. A family that already holds an address
    // but has no default route would otherwise never become ready - and the
    // watchdog would never reach the align path that could ifup it. Ask
    // netifd explicitly, once, for exactly the reopened sections.
    let mut reloaded = false;
    for (fam, sec) in &reopened {
        if network::group_family_has_address(snap, primary, *fam)
            && snap.group_route(primary, *fam).is_none()
        {
            log::log_info(
                "reconcile",
                &format!("bringing {sec} up so its default route is re-installed"),
            );
            let _ = crate::system::command::ifup_section(sec);
            reloaded = true;
        }
    }
    if reloaded {
        std::thread::sleep(std::time::Duration::from_secs(ALIGN_RELOAD_SETTLE_S as u64));
    }
    log::log_warn(
        "reconcile",
        &format!(
            "restored the parked default route of {} ({})",
            primary.as_str(),
            sections.join(",")
        ),
    );
    true
}

/// `reconcile_align`: the decision half of the reconciler (see the shell
/// version for the exact branch semantics).
#[allow(unused_assignments)]
pub fn reconcile_align(cfg: &AppConfig, snap: &LiveSnapshot, force: bool) -> Result<()> {
    let s4 = network::slot_owner_family(snap, Family::V4);
    let s6 = network::slot_owner_family(snap, Family::V6);
    let s4 = if s4.is_empty() {
        "none".to_string()
    } else {
        s4
    };
    let s6 = if s6.is_empty() {
        "none".to_string()
    } else {
        s6
    };
    let primary = cfg.mode.primary_group();
    let backup = primary.other();
    let mut owner = String::new();

    if s4 == s6 {
        let active = s4;
        owner = active.clone();
        match active.as_str() {
            "wan" | "modem" => {
                if let Some(g) = Group::parse(&active) {
                    let standby = g.other();
                    if crate::health::group_degraded(snap, cfg, g)
                        && network::group_complete_online(snap, standby, cfg.strict_dual_stack)
                    {
                        log::log_info(
                            "reconcile",
                            &format!(
                                "exit {active} degraded: failing over to {}",
                                standby.as_str()
                            ),
                        );
                        if align_to(cfg, standby, "exit_degraded", snap, false).is_ok() {
                            owner = standby.as_str().to_string();
                        }
                    } else if active != primary.as_str()
                        && network::group_complete_online(snap, primary, cfg.strict_dual_stack)
                    {
                        if (force || confirm_candidate(cfg, primary))
                            && align_to(cfg, primary, "failback", snap, force).is_ok()
                        {
                            owner = primary.as_str().to_string();
                        }
                    } else {
                        clear_candidate();
                    }
                }
            }
            _ => {
                // No usable default route: rescue onto whichever group is
                // complete, preferring the policy primary.
                if network::group_complete_online(snap, primary, cfg.strict_dual_stack) {
                    if align_to(cfg, primary, "rescue", snap, force).is_ok() {
                        owner = primary.as_str().to_string();
                    }
                } else if network::group_complete_online(snap, backup, cfg.strict_dual_stack)
                    && align_to(cfg, backup, "rescue", snap, force).is_ok()
                {
                    owner = backup.as_str().to_string();
                } else {
                    // Both groups are structurally incomplete (boot: no
                    // address/route yet) or unproven. Doing nothing is the
                    // correct action - the next interface event or watchdog
                    // tick re-evaluates, and a premature rescue must never
                    // park an exit that is merely late.
                    log::log_info(
                        "reconcile",
                        "no exit is complete yet; waiting for an interface event or the next tick",
                    );
                }
            }
        }
        write_ipv6_owner(&owner);
        return Ok(());
    }

    // Only one family has an exit: nothing to repair - tearing the other one
    // down would be worse than the single-family gap.
    if s4 == "none" || s6 == "none" {
        owner = if s4 != "none" { s4.clone() } else { s6.clone() };
        log::log_info(
            "reconcile",
            &format!("ipv4={s4} ipv6={s6}: only one family has a default route; leaving it alone"),
        );
        write_ipv6_owner(&owner);
        return Ok(());
    }

    owner = "split".to_string();
    let mut chosen: Option<Group> = None;
    if network::group_complete(snap, primary, cfg.strict_dual_stack) {
        chosen = Some(primary);
    } else if network::group_complete(snap, backup, cfg.strict_dual_stack) {
        chosen = Some(backup);
    }
    if let Some(chosen) = chosen {
        log::log_info(
            "reconcile",
            &format!(
                "IPv4/IPv6 split (ipv4={s4} ipv6={s6}): aligning both families to {}",
                chosen.as_str()
            ),
        );
        if align_to(cfg, chosen, "split_repair", snap, force).is_ok() {
            owner = chosen.as_str().to_string();
        }
    } else {
        log::log_info(
            "reconcile",
            &format!("IPv4/IPv6 split (ipv4={s4} ipv6={s6}) and no complete dual-stack exit is available"),
        );
    }
    write_ipv6_owner(&owner);
    Ok(())
}

/// `reconcile_body`: one serialised evaluation.
pub fn reconcile_body(cfg: &AppConfig, snap: &LiveSnapshot, force: bool) -> Result<()> {
    if crate::state::switch_in_progress() {
        log::log_info("reconcile", "skipped: switch in progress");
        return Ok(());
    }

    // A primary whose `defaultroute` was parked by an earlier (buggy or
    // partial) rescue must be reopened before any readiness decision:
    // otherwise it can never become complete again and the watchdog can never
    // rescue back to it. Re-read the snapshot when the policy actually
    // changed, so this pass judges the live state instead of the parked one.
    let reopened;
    let snap = if repair_primary_policy(cfg, snap) {
        reopened = network::read_live_state();
        &reopened
    } else {
        snap
    };

    if cfg.health_check {
        let forced = force || state::state_get("health_force") == "1";
        crate::health::refresh_health(snap, cfg, forced);
        if forced {
            let _ = state::state_write(&[("health_force", "0")]);
        }
    }

    remember_routes(snap);
    transaction::maintain_warm_routes(cfg.mode, snap, cfg);
    reconcile_align(cfg, snap, force)?;
    transaction::reap_orphan_routes();
    Ok(())
}
