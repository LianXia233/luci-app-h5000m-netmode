#![allow(static_mut_refs)]
//! The switch transaction: an explicit state machine, ported verbatim from the
//! shell backend's `switch_run()`.
//!
//!   PREPARING_TARGET -> WAIT_IPV4 -> WAIT_IPV6 -> VERIFY_IPV4 -> VERIFY_IPV6
//!   -> SWITCHING -> VERIFY_TARGET -> COMMITTED
//!                       |                     |
//!                       +------> ROLLBACK -> FAILED
//!
//! Only the last two stages touch the live routing table, and each is a single
//! netlink replace per family; the old exit is never taken down, only demoted
//! to the standby metric.

use crate::config::uci;
use crate::config::AppConfig;
use crate::network::{self, LiveSnapshot};
use crate::state;
use crate::system::clock::{fmt_cs, now_cs, unix_ts};
use crate::system::command;
use crate::system::log;
use crate::types::{Family, Group, Mode, Result, SmState};

pub mod budget;
pub mod transaction;

pub const DEFAULT_VERIFY_REASON: &str = "priority_verify_failed";

// ---------------------------------------------------------------------------
// task state (the state machine's observable half)
// ---------------------------------------------------------------------------

static mut TASK_START_CS: u64 = 0;
static mut PHASE_START_CS: u64 = 0;
static mut PHASE_LOG: String = String::new();

fn task_begin(kind: &str, target: &str, target_mode: &str) {
    let now = now_cs();
    unsafe {
        TASK_START_CS = now;
        PHASE_START_CS = now;
        PHASE_LOG = String::new();
    }
    let _ = state::state_write(&[
        ("state", SmState::PREPARING),
        ("kind", kind),
        ("target", target),
        ("target_mode", target_mode),
        ("result", "none"),
        ("reason", ""),
        ("pid", &std::process::id().to_string()),
        ("started", &unix_ts().to_string()),
        ("elapsed", "0.00s"),
        ("phases", ""),
        ("message", ""),
    ]);
}

fn task_phase(phase: SmState, message: &str) {
    let now = now_cs();
    let (phases, elapsed);
    unsafe {
        let elapsed_phase = now.saturating_sub(PHASE_START_CS);
        PHASE_LOG.push_str(&format!("{}:{} ", phase.as_str(), fmt_cs(elapsed_phase)));
        PHASE_START_CS = now;
        let total = now.saturating_sub(TASK_START_CS);
        phases = PHASE_LOG.trim().to_string();
        elapsed = fmt_cs(total);
    }
    let refs: [(&str, String); 4] = [
        ("state", phase.as_str().to_string()),
        ("message", sanitize(message)),
        ("phases", phases),
        ("elapsed", elapsed),
    ];
    let pairs: Vec<(&str, &str)> = refs.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let _ = state::state_write(&pairs);
    log::log_info("switch", &format!("[{}] {message}", phase.as_str()));
}

fn task_finish(st: SmState, result: &str, reason: &str, message: &str) {
    let now = now_cs();
    let elapsed;
    unsafe {
        elapsed = fmt_cs(now.saturating_sub(TASK_START_CS));
    }
    let phases = unsafe { PHASE_LOG.trim().to_string() };
    let _ = state::state_write(&[
        ("state", st.as_str()),
        ("result", result),
        ("reason", reason),
        ("message", message),
        ("elapsed", &elapsed),
        ("phases", &phases),
        ("pid", ""),
    ]);
    log::log_info("switch", message);
}

fn sanitize(v: &str) -> String {
    v.chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            _ => c,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// target preparation
// ---------------------------------------------------------------------------

/// `prepare_group <group> <target_mode>`: align the target's own config with
/// the plan and bring it up if it is not, without tearing anything down.
pub fn prepare_group(g: Group, mode: Mode, snap: &LiveSnapshot) {
    let mut changed = false;
    for fam in Family::ALL {
        let sec = snap.group_sec(g, fam);
        if sec.is_empty() {
            continue;
        }
        let metric = transaction::policy_group_metric(mode, g).to_string();
        transaction::set_network_option_if_changed(sec, "metric", &metric, &mut changed);
        transaction::set_network_option_if_changed(sec, "defaultroute", "1", &mut changed);
        if fam == Family::V6 {
            transaction::set_network_option_if_changed(sec, "auto", "1", &mut changed);
        }
    }
    if changed {
        let _ = uci::uci_exec(&["-q", "commit", "network"]);
    }

    for fam in Family::ALL {
        let sec = snap.group_sec(g, fam);
        if sec.is_empty() {
            continue;
        }
        if network::group_family_ready(snap, g, fam) {
            continue;
        }
        let st = snap.group_state(g, fam);
        if !st.up || st.devs.is_empty() {
            log::log_info("switch", &format!("bringing the target interface {sec} up"));
            let _ = command::ifup_section(sec);
        }
    }
    transaction::warm_group_route(mode, snap, g);
}

/// `wait_group_family`: bounded conditional wait for address + default route,
/// with one warm attempt and one bounded ifup retry. No fixed sleeps beyond
/// the 1s check interval (which is a condition poll, bounded by budget).
pub fn wait_group_family(
    g: Group,
    fam: Family,
    limit: u32,
    mode: Mode,
    snap: &mut LiveSnapshot,
) -> bool {
    let mut waited: u32 = 0;
    let mut tried_warm = false;
    let mut tried_ifup = false;
    loop {
        if network::group_family_ready(snap, g, fam) {
            return true;
        }
        if waited >= limit || !budget::budget_left() {
            return false;
        }
        if waited == 0 && !tried_warm {
            tried_warm = true;
            transaction::warm_group_route(mode, snap, g);
        }
        if waited == 2 && !tried_ifup {
            tried_ifup = true;
            let sec = snap.group_sec(g, fam);
            if !sec.is_empty() {
                log::log_info("switch", &format!("retrying ifup {sec}"));
                let _ = command::ifup_section(sec);
            }
        }
        if !budget::bounded_sleep(1) {
            return false;
        }
        waited += 1;
        // Re-read live kernel state: a warm/ifup that succeeded only becomes
        // visible in a FRESH snapshot - the caller's snapshot was taken at
        // switch start and would stay stale until the wait timed out (the
        // spurious "ipv4_not_ready" rollback on an already-healthy uplink).
        *snap = network::read_live_state();
        let _ = state::state_write(&[(
            "message",
            &format!("waiting ipv{} ({}) {waited}s/{limit}s", fam.n(), g.as_str()),
        )]);
    }
}

// ---------------------------------------------------------------------------
// the switch pipeline
// ---------------------------------------------------------------------------

/// Run one switch to `target_mode`. `kind` is "switch" (user) or "align"
/// (reconciler repair). Returns Ok(()) on COMMITTED.
#[allow(clippy::too_many_arguments)]
pub fn switch_run(
    cfg: &AppConfig,
    target_mode: Mode,
    kind: &str,
    forced_target: Option<Group>,
    snap: &mut LiveSnapshot,
) -> Result<()> {
    let target = forced_target.unwrap_or_else(|| target_mode.primary_group());
    let previous_mode = cfg.mode;
    let previous = snap.slot_owner.clone();
    let previous_group = Group::parse(&previous);

    // A mode that already governs the routing table must not move anything:
    // all five predicates are pure reads of the snapshot.
    if previous_mode == target_mode
        && previous == target.as_str()
        && network::group_complete(snap, target, cfg.strict_dual_stack)
        && transaction::verify_commit(snap, target, cfg).is_ok()
    {
        log::log_info(
            "switch",
            &format!(
                "mode {} already governs {target}; nothing to move",
                target_mode.as_str()
            ),
        );
        task_begin(kind, target.as_str(), target_mode.as_str());
        task_phase(
            SmState::Preparing,
            &format!("policy already in effect on {}", target.as_str()),
        );
        task_finish(
            SmState::Committed,
            "ok",
            "",
            &format!(
                "{kind} committed: already on {} (mode={})",
                target.as_str(),
                target_mode.as_str()
            ),
        );
        crate::health::refresh_health(snap, cfg, true);
        return Ok(());
    }

    budget::budget_start(cfg.switch_budget);
    let target_warm = network::group_complete(snap, target, cfg.strict_dual_stack);
    task_begin(kind, target.as_str(), target_mode.as_str());
    log::log_info(
        "switch",
        &format!(
            "{kind} {previous} -> {target} (mode={}, strict_dual_stack={})",
            target_mode.as_str(),
            cfg.strict_dual_stack as u8
        ),
    );

    let mut failure: Option<String> = None;

    // ---- PREPARING_TARGET ----
    task_phase(
        SmState::Preparing,
        &format!("preparing target {target} (mode={})", target_mode.as_str()),
    );
    prepare_group(target, target_mode, snap);

    // A family the target has no interface section for is simply not part of
    // the switch (group_family_required is capability-based), so there is no
    // pre-flight rejection here anymore: a single-stack target switches as a
    // single-stack exit, and the standby's route for the missing family gets
    // parked by apply_group_holes further down. A *capable* family that never
    // becomes ready fails in its own wait/probe phase below.

    // ---- WAIT_IPV4 / WAIT_IPV6 ----
    if failure.is_none() {
        task_phase(SmState::WaitIpv4, &format!("waiting IPv4 on {target}"));
        if network::group_family_required(snap, target, Family::V4, cfg.strict_dual_stack) {
            if wait_group_family(target, Family::V4, cfg.switch_wait_ipv4, target_mode, snap) {
                log::log_info(
                    "switch",
                    &format!(
                        "IPv4 ready ({})",
                        snap.group_devs(target, Family::V4).join(",")
                    ),
                );
            } else {
                failure = Some("ipv4_not_ready".into());
            }
        }
    }
    if failure.is_none() {
        task_phase(SmState::WaitIpv6, &format!("waiting IPv6 on {target}"));
        if network::group_family_required(snap, target, Family::V6, cfg.strict_dual_stack) {
            if wait_group_family(target, Family::V6, cfg.switch_wait_ipv6, target_mode, snap) {
                log::log_info(
                    "switch",
                    &format!(
                        "IPv6 ready ({})",
                        snap.group_devs(target, Family::V6).join(",")
                    ),
                );
            } else if cfg.strict_dual_stack {
                failure = Some("ipv6_not_ready".into());
            } else {
                log::log_warn(
                    "switch",
                    &format!(
                        "warning: IPv6 not ready on {target} in time; continuing single-stack (strict_dual_stack=0)"
                    ),
                );
            }
        }
    }

    // ---- VERIFY_IPV4 / VERIFY_IPV6 ----
    if failure.is_none() {
        task_phase(
            SmState::VerifyIpv4,
            &format!("probing IPv4 reachability on {target}"),
        );
        if network::group_family_required(snap, target, Family::V4, cfg.strict_dual_stack) {
            let v = crate::probe::probe_group_family(
                Family::V4,
                target,
                snap.group_devs(target, Family::V4),
                cfg.probe_attempts,
                cfg.probe_ok,
                cfg.probe_timeout,
                &cfg.probe_targets,
                &cfg.probe_targets6,
            );
            if v.is_up() {
                log::log_info("switch", "IPv4 connectivity OK");
            } else {
                failure = Some("ipv4_unreachable".into());
            }
        }
    }
    if failure.is_none() {
        task_phase(
            SmState::VerifyIpv6,
            &format!("probing IPv6 reachability on {target}"),
        );
        if network::group_family_required(snap, target, Family::V6, cfg.strict_dual_stack) {
            let v = crate::probe::probe_group_family(
                Family::V6,
                target,
                snap.group_devs(target, Family::V6),
                cfg.probe_attempts,
                cfg.probe_ok,
                cfg.probe_timeout,
                &cfg.probe_targets,
                &cfg.probe_targets6,
            );
            if v.is_up() {
                log::log_info("switch", "IPv6 connectivity OK");
            } else if cfg.strict_dual_stack {
                failure = Some("ipv6_unreachable".into());
            } else {
                log::log_warn(
                    "switch",
                    "warning: IPv6 probes failed on the target; continuing single-stack (strict_dual_stack=0)",
                );
            }
        }
    }
    if failure.is_none() && cfg.gw_required {
        if let Some(gw) = snap.group_gateway(target, Family::V4) {
            if !crate::probe::icmp::gateway_reachable(
                Family::V4,
                snap.group_devs(target, Family::V4),
                &gw,
                cfg.probe_timeout,
            ) {
                failure = Some("ipv4_gateway_unreachable".into());
            }
        }
    }

    // ---- SWITCHING ----
    let mut plan_written = false;
    let mut committed4 = false;
    let mut committed6 = false;
    if failure.is_none() {
        task_phase(
            SmState::Switching,
            &format!("switching default routes to {target}"),
        );
        if transaction::apply_policy(target_mode, snap).is_ok() {
            plan_written = true;
        } else {
            failure = Some("policy_write_failed".into());
        }
    }
    if failure.is_none() {
        if transaction::commit_family(Family::V4, target, previous_group, snap).is_ok() {
            committed4 = true;
        } else {
            failure = Some("ipv4_commit_failed".into());
        }
    }
    if failure.is_none() {
        if transaction::commit_family(Family::V6, target, previous_group, snap).is_ok() {
            committed6 = true;
        } else if cfg.strict_dual_stack {
            failure = Some("ipv6_commit_failed".into());
        } else {
            log::log_warn(
                "switch",
                "warning: IPv6 default route commit failed; continuing single-stack (strict_dual_stack=0)",
            );
        }
    }
    if failure.is_none() && !transaction::apply_group_holes(target_mode, snap, cfg, target) {
        failure = Some("hole_parking_failed".into());
    }

    // ---- VERIFY_TARGET ----
    if failure.is_none() {
        task_phase(SmState::VerifyTarget, "verifying the committed exit");
        // The snapshot was captured before commit_family rewrote the FIB, so
        // every snapshot-backed predicate below would still see the world as
        // it was pre-surgery (the same frozen-state trap S1 fixed in the WAIT
        // phase). Refresh once so the whole verify pass judges the committed
        // state, not the pre-switch one.
        *snap = network::read_live_state();
        // verify_commit: ipv6_* failures degrade to warnings under
        // strict_dual_stack=0 so a committed IPv4 exit is never rolled back.
        if let Err(r) = transaction::verify_commit(snap, target, cfg) {
            failure = Some(r);
        } else {
            // One settle window, then the same verification again; a target
            // that was already online is not delayed (switch_settle_warm).
            let settle = if target_warm {
                cfg.switch_settle_warm
            } else {
                cfg.switch_settle
            };
            if settle > 0 && budget::budget_left() {
                budget::bounded_sleep(settle);
            }
            let fresh = network::read_live_state();
            if let Err(r) = transaction::verify_commit(&fresh, target, cfg) {
                failure = Some(r);
            } else if verify_group_online_after_settle(&fresh, cfg, target).is_err() {
                failure = Some(DEFAULT_VERIFY_REASON.to_string());
            } else {
                *snap = fresh;
            }
        }
    }

    // ---- outcome ----
    if failure.is_none() {
        let elapsed = budget::elapsed_cs(unsafe { TASK_START_CS });
        let _ = state::state_write(&[("applied_mode", target_mode.as_str())]);
        task_finish(
            SmState::Committed,
            "ok",
            "",
            &format!("{kind} committed: {previous} -> {target} in {elapsed}"),
        );
        crate::health::refresh_health(snap, cfg, true);
        command::vendor_sync();
        return Ok(());
    }

    let failure = failure.unwrap();
    task_phase(
        SmState::Rollback,
        &format!("rollback to {previous} ({failure})"),
    );
    log::log_info("switch", &format!("{kind} failed: {failure}"));
    // Undo only what the switch actually changed.
    if plan_written {
        let _ = transaction::apply_policy(previous_mode, snap);
    }
    if let Some(prev) = previous_group {
        if prev != target {
            if committed4 {
                transaction::rollback_family(Family::V4, prev, target, snap);
            }
            if committed6 {
                transaction::rollback_family(Family::V6, prev, target, snap);
            }
        }
    }
    if committed4 || committed6 {
        transaction::post_failure_converge(snap, cfg);
    }
    task_finish(
        SmState::Failed,
        "failed",
        &failure,
        &format!("{kind} failed: {failure}; rolled back to {previous}"),
    );
    crate::health::refresh_health(snap, cfg, true);
    Err(crate::types::Error::switch(failure))
}

/// The post-settle online check uses the (fresh) snapshot's reachability.
fn verify_group_online_after_settle(snap: &LiveSnapshot, cfg: &AppConfig, g: Group) -> Result<()> {
    for fam in Family::ALL {
        if !network::group_family_capable(snap, g, fam) {
            continue;
        }
        let v = crate::probe::probe_group_family(
            fam,
            g,
            snap.group_devs(g, fam),
            cfg.probe_attempts,
            cfg.probe_ok,
            cfg.probe_timeout,
            &cfg.probe_targets,
            &cfg.probe_targets6,
        );
        if !v.is_up() {
            if fam == Family::V6 && !cfg.strict_dual_stack {
                log::log_warn(
                    "switch",
                    "warning: IPv6 unreachable after settle; keeping the single-stack commit (strict_dual_stack=0)",
                );
                continue;
            }
            return Err(crate::types::Error::probe(format!(
                "ipv{}_unreachable_after_settle",
                fam.n()
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// worker loop
// ---------------------------------------------------------------------------

/// The detached worker loop: run the switch; if a newer request arrived while
/// in flight, execute it too (the final state is the user's last request).
pub fn switch_worker(cfg: &AppConfig, mode: Mode, my_gen: u32) -> Result<()> {
    let mut applied = mode;
    let mut gen = my_gen;
    let mut rc = Ok(());
    for _ in 0..3 {
        let mut snap = network::read_live_state();
        rc = switch_run(cfg, applied, "switch", None, &mut snap);
        let pending = state::state_get("requested_mode");
        let applied_mode = state::state_get("applied_mode");
        let pending_gen = crate::types::num_or::<u32>(&state::state_get("gen"), 0);
        if pending_gen > gen && !pending.is_empty() && pending != applied_mode {
            if let Some(m) = Mode::parse(&pending) {
                log::log_info(
                    "switch",
                    &format!(
                        "a newer request ({pending}) arrived while switching; applying it now"
                    ),
                );
                gen = pending_gen;
                applied = m;
                continue;
            }
        }
        break;
    }
    // Re-arm the warm standby route and the audit field after the switch.
    let snap = network::read_live_state();
    let _ = crate::reconcile::reconcile_body(cfg, &snap, true);
    rc
}
