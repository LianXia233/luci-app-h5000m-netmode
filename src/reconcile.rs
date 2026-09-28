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
pub fn align_cooldown_ok(cfg: &AppConfig, force: bool) -> bool {
    if force {
        return true;
    }
    let now = unix_ts();
    let last = crate::types::num_or::<u64>(&state::state_get("last_align_ts"), 0);
    now.saturating_sub(last) >= switch_cooldown(cfg) as u64
}

/// `mark_align`: persist the audit trail of an alignment.
pub fn mark_align(reason: &str, target: &str) {
    let _ = state::state_write(&[
        ("last_align_ts", &unix_ts().to_string()),
        ("last_align_reason", reason),
        ("last_align_target", target),
    ]);
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
/// be warmed later.
pub fn remember_routes(snap: &LiveSnapshot) {
    for g in Group::ALL {
        for fam in Family::ALL {
            if let Some(r) = snap.group_route(g, fam) {
                transaction::remember_group_route(g, fam, r);
            }
        }
    }
}

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
    if let Err(e) = transaction::verify_group_online(snap, cfg, group) {
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
        if !network::group_family_required(snap, group, fam, cfg.strict_dual_stack) {
            continue;
        }
        if commit_family(fam, group, Some(previous), snap).is_err() {
            ok = false;
            break;
        }
    }

    let _ = state::state_write(&[
        ("state", SmState::VERIFY_TARGET),
        ("message", &format!("verifying {}", group.as_str())),
    ]);
    if ok
        && transaction::verify_group_priority(snap, group, cfg).is_ok()
        && transaction::verify_excluded_families(snap, group, cfg).is_ok()
    {
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
        crate::health::refresh_health(snap, cfg, false);
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
        let _ = commit_family(fam, previous, Some(group), snap);
    }
    transaction::post_failure_converge(snap, cfg);
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
    crate::health::refresh_health(snap, cfg, false);
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
