//! Deterministic logic tests for the h5000m-netmode backend.
//!
//! These run anywhere `cargo test` runs: every kernel / daemon view is
//! redirected through the scenario fixtures (see `tests/common/mod.rs`), so
//! no root, no network namespace and no real device are required. The
//! route-writing paths are exercised against fixtures without `ifindex`, so
//! an accidental netlink write fails loudly instead of touching a real
//! interface; the real-kernel suite lives in `tests/backend_netns.rs`.
//!
//! Concurrency rules under test (from the task contract):
//!   * gen protocol: the newest user request wins;
//!   * superseded transactions abort before the first route write;
//!   * hotplug events coalesce into one marker run and the marker is only
//!     released after the writer lock is held;
//!   * the watchdog tick defers to a busy lock instead of racing it;
//!   * a stale lock owner is reaped;
//!   * stable state: reconcile leaves state, uci and routes untouched;
//!   * a TUN egress is attributed to its physical uplink, never managed.

mod common;

use common::{file_lock, Scenario};

fn both_up_scenario(s: &Scenario) {
    s.standard_network();
    s.settings(&[]);
    s.netdev("wan0", 1);
    s.netdev("modem0", 1);
    s.addrs4(&[("wan0", "192.168.88.187"), ("modem0", "10.13.35.40")]);
    s.addrs6(&[
        ("wan0", "2409:8800::187", false),
        ("wan0", "fe80::1", true),
        ("modem0", "2409:8a00::40", false),
        ("modem0", "fe80::2", true),
    ]);
    s.routes4(&[("wan0", "192.168.88.1", 10), ("modem0", "10.13.35.1", 50)]);
    s.routes6(&[("wan0", "fe80::1", 10, ""), ("modem0", "fe80::2", 50, "")]);
    s.ubus_section("wan", true, false, "wan0");
    s.ubus_section("wan6", true, false, "wan0");
    s.ubus_section("modem0", true, false, "modem0");
    s.ubus_section("modem0_6", true, false, "modem0");
}

// ---------------------------------------------------------------------------
// 1. gen protocol: the newest request wins
// ---------------------------------------------------------------------------

#[test]
fn set_while_switch_running_is_queued_with_latest_mode() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("gen");
    both_up_scenario(&s);
    // Fabricate an in-flight switch owned by THIS process.
    s.state_write(&[
        ("state", "SWITCHING"),
        ("kind", "switch"),
        ("target", "modem"),
        ("pid", &std::process::id().to_string()),
        ("gen", "10"),
    ]);

    let rc = s.run(&["set", "modem_first"]);
    assert_eq!(rc, 3, "an in-flight switch must answer busy, not spawn");
    assert_eq!(s.state("gen"), "11", "the request bumps the generation");
    assert_eq!(
        s.state("requested_mode"),
        "modem_first",
        "the request is recorded for the running worker"
    );

    let rc = s.run(&["set", "wan_first"]);
    assert_eq!(rc, 3);
    assert_eq!(s.state("gen"), "12");
    assert_eq!(s.state("requested_mode"), "wan_first");

    // The worker loop picks up the LATEST request: simulate the running
    // worker's post-run check by invoking the worker for the first request;
    // it must observe the newer gen and land on wan_first.
    s.state_write(&[
        ("state", "COMMITTED"),
        ("pid", ""),
        ("applied_mode", "modem_first"),
    ]);
    let rc = s.run(&["switch-worker", "modem_first", "11"]);
    // Both families are ready and probes are not mocked as reachable here:
    // the switch fails its probe phase, but the failure mode is irrelevant -
    // what matters is WHICH request the worker executed last.
    assert!(rc == 0 || rc == 1);
    assert_eq!(
        s.state("target_mode"),
        "wan_first",
        "the worker must end on the newest request, not the stale one"
    );
    assert_eq!(s.state("gen"), "12");
    s.cleanup();
}

#[test]
fn superseded_transaction_aborts_before_any_route_write() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("superseded");
    both_up_scenario(&s);
    // The modem is the active exit (metric 10) and the UCI mode already says
    // modem_first, so the NEWER request lands on the already-governs path -
    // the only commit path that works without a real kernel route write.
    // The wired target has no IPv4 address, so the FIRST transaction
    // (wan_first) parks in its wait loop - which is where the supersede
    // check fires.
    s.settings(&[("mode", "modem_first"), ("probe_attempts", "0")]);
    s.addrs4(&[("modem0", "10.13.35.40")]);
    s.routes4(&[("modem0", "10.13.35.1", 10)]);
    s.routes6(&[("modem0", "fe80::2", 10, "")]);

    let dir = s.dir.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let mut text = String::new();
        for l in std::fs::read_to_string(dir.join("state"))
            .unwrap_or_default()
            .lines()
        {
            if !l.starts_with("gen=") && !l.starts_with("requested_mode=") {
                text.push_str(l);
                text.push('\n');
            }
        }
        text.push_str("gen=2\nrequested_mode=modem_first\n");
        std::fs::write(dir.join("state"), text).unwrap();
    });

    let started = std::time::Instant::now();
    let rc = s.run(&["set", "wan_first", "--wait"]);
    let elapsed = started.elapsed();
    assert_eq!(
        rc, 0,
        "a superseded run hands over to the newer request and succeeds with it"
    );
    assert_eq!(
        s.state("applied_mode"),
        "modem_first",
        "the newer request must be the one executed"
    );
    assert_eq!(s.state("gen"), "2");
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "the abort must not run the full wait window (took {elapsed:?})"
    );
    // The aborted target never touched the FIB: the fixture route table only
    // ever held the modem line (a netlink write would fail anyway - the
    // fixture devices have no ifindex - but the abort must happen before the
    // SWITCHING phase is even attempted).
    let route_text = std::fs::read_to_string(s.path("proc/route")).unwrap();
    assert!(route_text.contains("modem0"));
    assert!(
        !route_text.contains("wan0"),
        "no route was installed for wan0"
    );
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 2. hotplug event merging
// ---------------------------------------------------------------------------

#[test]
fn hotplug_events_coalesce_into_one_marker_run() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("coalesce");
    both_up_scenario(&s);

    s.run(&["notify", "wan", "ifup"]);
    // The marker must be gone after a completed run.
    assert!(
        !s.marker().exists(),
        "the debounce marker must be released after the run"
    );
    let runs_after_first = s.state("hotplug_runs");

    // A burst: while the marker exists, every event only updates the record.
    std::fs::create_dir_all(s.marker()).unwrap();
    std::fs::write(s.marker().join("pid"), "999999").unwrap(); // dead owner
    let rc = s.run(&["notify", "wan", "ifdown"]);
    assert_eq!(rc, 0);
    // Dead owner -> the stale marker is reaped and a NEW run starts.
    assert_eq!(
        s.state("hotplug_runs"),
        format!("{}", runs_after_first.parse::<u32>().unwrap() + 1),
        "a stale marker is reaped, the event still runs"
    );
    assert!(!s.marker().exists());
    s.cleanup();
}

#[test]
fn notify_records_events_even_when_coalesced() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("coalesce2");
    both_up_scenario(&s);

    // Hold the marker so the second event coalesces.
    std::fs::create_dir_all(s.marker()).unwrap();
    std::fs::write(s.marker().join("pid"), std::process::id().to_string()).unwrap();
    s.run(&["notify", "modem0", "ifdown"]);
    assert_eq!(
        s.state("hotplug_events"),
        "1",
        "the coalesced event is still counted"
    );
    assert_eq!(s.state("hotplug_last"), "modem0:ifdown");
    // hotplug_runs is only bumped by the marker OWNER; a coalesced event
    // must not start a second run.
    assert_eq!(s.state("hotplug_runs"), "");
    // Clean the live marker as the owner would.
    let _ = std::fs::remove_dir_all(s.marker());
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 3. watchdog discipline
// ---------------------------------------------------------------------------

#[test]
fn watchdog_tick_defers_to_a_busy_lock() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("watchbusy");
    both_up_scenario(&s);

    s.apply_env();
    // Split state: a successful tick WOULD align (write uci + routes).
    s.routes6(&[("modem0", "fe80::2", 10, ""), ("wan0", "fe80::1", 50, "")]);

    // Hold the writer lock like a switch worker would.
    assert!(h5000m_netmode::state::acquire_lock(0));
    let started = std::time::Instant::now();
    let cfg = h5000m_netmode::config::AppConfig::load();
    h5000m_netmode::rpc::watch_tick(&cfg);
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "a busy lock must be skipped quickly, not waited out (took {elapsed:?})"
    );
    assert!(
        s.uci_ops().is_empty(),
        "no uci write may happen while another writer holds the lock"
    );
    h5000m_netmode::state::release_lock();

    // With the lock free, the same tick converges the split (the align
    // attempt itself is recorded even when its probes fail).
    h5000m_netmode::rpc::watch_tick(&cfg);
    assert!(
        !s.state("last_align_attempt_ts").is_empty(),
        "the free tick must act on the divergent state"
    );
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 4. lock hygiene
// ---------------------------------------------------------------------------

#[test]
fn stale_lock_owner_is_reaped() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("stalelock");
    both_up_scenario(&s);
    s.apply_env();

    // A lock directory owned by a process that no longer exists.
    let lock = s.path("lock");
    std::fs::create_dir_all(&lock).unwrap();
    std::fs::write(lock.join("pid"), "4000000").unwrap();
    assert!(
        h5000m_netmode::state::acquire_lock(0),
        "a stale lock owner must be reaped, not block forever"
    );
    h5000m_netmode::state::release_lock();
    assert!(!lock.exists(), "release must remove the lock directory");
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 5. stable state: zero disturbance
// ---------------------------------------------------------------------------

#[test]
fn stable_state_reconcile_is_read_only() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("stable");
    both_up_scenario(&s);
    s.settings(&[("ipv6_owner", "wan")]);

    // A converged state: wan active on both families, ipv6_owner recorded,
    // remembered routes stored - nothing left to do.
    s.state_write(&[
        ("state", "COMMITTED"),
        ("pid", ""),
        ("applied_mode", "wan_first"),
        ("ipv6_owner", "wan"),
        ("rt_wan4", "192.168.88.1 wan0"),
        ("rt_wan6", "fe80::1 wan0"),
        ("rt_modem4", "10.13.35.1 modem0"),
        ("rt_modem6", "fe80::2 modem0"),
        (
            "last_align_ts",
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        ),
        (
            "last_align_attempt_ts",
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        ),
    ]);

    let before_state = s.state_text();
    let before_routes = std::fs::read_to_string(s.path("proc/route")).unwrap();
    let before_routes6 = std::fs::read_to_string(s.path("proc/ipv6_route")).unwrap();
    let before_network = std::fs::read_to_string(s.path("config/network")).unwrap();

    let rc = s.run(&["reconcile"]);
    assert_eq!(rc, 0);
    let rc2 = s.run(&["reconcile"]);
    assert_eq!(rc2, 0);

    assert_eq!(
        s.state_text(),
        before_state,
        "a stable reconcile must not churn the state file"
    );
    assert_eq!(
        std::fs::read_to_string(s.path("proc/route")).unwrap(),
        before_routes
    );
    assert_eq!(
        std::fs::read_to_string(s.path("proc/ipv6_route")).unwrap(),
        before_routes6
    );
    assert_eq!(
        std::fs::read_to_string(s.path("config/network")).unwrap(),
        before_network,
        "no uci write on a stable state"
    );
    assert!(s.uci_ops().is_empty(), "no uci invocations at all");
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 6. exit classification under a transparent proxy
// ---------------------------------------------------------------------------

#[test]
fn tun_egress_is_attributed_to_the_physical_uplink() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("tun");
    both_up_scenario(&s);
    s.tunnel("singtun0");
    // IPv6 default rides the TUN; IPv4 stays on the physical modem uplink.
    s.routes4(&[("modem0", "10.13.35.1", 10), ("wan0", "192.168.88.1", 50)]);
    s.routes6(&[("singtun0", "fe80::942b:33ff:fec4:3402", 512, "")]);

    let out = s.status();
    assert_eq!(s.status_field(&out, "active4"), "modem");
    assert_eq!(
        s.status_field(&out, "active6"),
        "modem",
        "the TUN inherits the IPv4 physical owner, it is not a third exit"
    );
    assert_eq!(s.status_field(&out, "split"), "0");

    // A reconcile under a TUN egress must not touch the TUN's route: it is
    // not ours to manage.
    let before = std::fs::read_to_string(s.path("proc/ipv6_route")).unwrap();
    s.run(&["reconcile"]);
    assert_eq!(
        std::fs::read_to_string(s.path("proc/ipv6_route")).unwrap(),
        before,
        "the proxy's route must be left alone"
    );
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 7. flapping interfaces: the decision follows the real state
// ---------------------------------------------------------------------------

#[test]
fn flapping_ifdown_ifup_sequence_ends_on_the_real_state() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("flap");
    both_up_scenario(&s);

    // ifdown: the wan route and address disappear, ubus reports down.
    s.routes4(&[("modem0", "10.13.35.1", 50)]);
    s.addrs4(&[("modem0", "10.13.35.40")]);
    s.ubus_section("wan", false, false, "");
    s.run(&["notify", "wan", "ifdown"]);
    let out = s.status();
    assert_eq!(
        s.status_field(&out, "active4"),
        "modem",
        "after the WAN ifdown the modem carries the exit"
    );

    // ifup again: everything returns.
    s.routes4(&[("wan0", "192.168.88.1", 10), ("modem0", "10.13.35.1", 50)]);
    s.addrs4(&[("wan0", "192.168.88.187"), ("modem0", "10.13.35.40")]);
    s.ubus_section("wan", true, false, "wan0");
    s.run(&["notify", "wan", "ifup"]);
    let out = s.status();
    assert_eq!(s.status_field(&out, "active4"), "wan");
    assert_eq!(s.status_field(&out, "split"), "0");
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 8. concurrent notify workers: one marker, one writer
// ---------------------------------------------------------------------------

#[test]
fn concurrent_notify_events_share_one_marker_run() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("concurrent");
    both_up_scenario(&s);
    s.settings(&[("hotplug_debounce", "1")]);
    s.apply_env();

    let handles: Vec<_> = ["wan", "modem0", "wan", "modem0"]
        .iter()
        .map(|sec| {
            let sec = sec.to_string();
            std::thread::spawn(move || {
                let args = vec!["notify".to_string(), sec, "ifup".to_string()];
                h5000m_netmode::rpc::dispatch(&args)
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }

    assert_eq!(
        s.state("hotplug_runs"),
        "1",
        "all events in one window must share a single marker run"
    );
    assert!(
        !s.marker().exists(),
        "the marker must be released after the run"
    );
    assert!(!s.path("lock").exists(), "the writer lock must be released");
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 9. phase attribution (the 22 s mis-diagnosis regression)
// ---------------------------------------------------------------------------

#[test]
fn wait_phase_duration_is_charged_to_the_waiting_phase() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("phases");
    both_up_scenario(&s);
    // The wired exit is active (metric 10) and the switch target is the
    // modem: IPv4 is ready immediately, IPv6 has NO address at all on the
    // modem (only the wired link-local survives), so under
    // strict_dual_stack=0 the v6 wait hits the no-address cap (3 s here) and
    // continues single-stack. probe_attempts=0 makes the VERIFY phase fail
    // instantly - no real probing on a test machine - so the run rolls back
    // right after the wait.
    s.addrs6(&[("wan0", "fe80::1", true)]);
    s.settings(&[("switch_wait_ipv6", "3"), ("probe_attempts", "0")]);
    s.routes4(&[("wan0", "192.168.88.1", 10), ("modem0", "10.13.35.1", 50)]);

    s.run(&["set", "modem_first", "--wait"]);
    let phases = s.state("phases");
    // Every phase's own duration must appear under its own name.
    assert!(phases.contains("WAIT_IPV6:"), "phases field: {phases}");
    // The label of a phase must not charge the NEXT phase's time: extract
    // WAIT_IPV6's own duration and require it to be nonzero here (the cap
    // produced a real wait) while VERIFY phases stay fast.
    let wait6 = phases
        .split_whitespace()
        .find_map(|p| p.strip_prefix("WAIT_IPV6:"))
        .unwrap_or("9.99s");
    let secs: f64 = wait6.trim_end_matches('s').parse().unwrap();
    assert!(secs > 0.0, "WAIT_IPV6 must carry its own wait time");
    let verify4 = phases
        .split_whitespace()
        .find_map(|p| p.strip_prefix("VERIFY_IPV4:"))
        .unwrap_or("9.99s");
    let v4: f64 = verify4.trim_end_matches('s').parse().unwrap();
    assert!(
        v4 < 1.0,
        "VERIFY_IPV4 must not inherit WAIT_IPV6's time (phases: {phases})"
    );
    s.cleanup();
}

// ---------------------------------------------------------------------------
// 10. cooldown consumes failed attempts too
// ---------------------------------------------------------------------------

#[test]
fn failed_alignment_consumes_the_cooldown() {
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    let s = Scenario::new("cooldown");
    both_up_scenario(&s);
    // A split with NO complete exit: the align attempt fails its probes
    // (no target devices can answer) and must still consume the cooldown.
    s.routes4(&[("wan0", "192.168.88.1", 10), ("modem0", "10.13.35.1", 50)]);
    s.routes6(&[("modem0", "fe80::2", 10, ""), ("wan0", "fe80::1", 50, "")]);
    s.settings(&[("switch_cooldown", "3600"), ("health_check", "1")]);

    s.run(&["reconcile"]);
    let attempt = s.state("last_align_attempt_ts");
    assert!(
        !attempt.is_empty(),
        "the failed align must stamp the attempt clock"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let parsed: u64 = attempt.parse().unwrap();
    assert!(now.saturating_sub(parsed) < 60, "the stamp is recent");

    // A second immediate reconcile must defer (cooldown), not re-probe.
    s.run(&["reconcile"]);
    assert_eq!(
        s.state("last_align_attempt_ts"),
        attempt,
        "the cooldown must gate the retry"
    );
    s.cleanup();
}
