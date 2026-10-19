//! Real-kernel route-surgery tests inside a throwaway network namespace.
//!
//! These complement `tests/backend_logic.rs`: the fixtures there redirect
//! every kernel view, so no route write ever reaches a real kernel; HERE the
//! netlink writes are real. The suite must be started inside a private
//! network namespace or it refuses to run:
//!
//! ```sh
//! H5_NETNS_TEST=1 unshare -n target/aarch64-unknown-linux-musl/debug/deps/backend_netns-*
//! ```
//!
//! The gate is deliberate: this binary also builds on machines where the
//! tests would otherwise run as root in the ROOT namespace and touch real
//! routing tables. When the gate is absent the tests print a skip notice and
//! pass without asserting anything (the device run is the authoritative
//! execution; see tests/device-run.sh).
//!
//! Topology (two dummy links in the namespace):
//!   h5wan   10.13.35.40/24  + 2409:8a00::40/64   (the wired-WAN exit)
//!   h5modem 10.13.35.41/24  + 2409:8a00::41/64   (the 5G exit)
//! The two subnets deliberately OVERLAP: the shared probe-target list then
//! contains one address that is local on each dummy, so `SO_BINDTODEVICE`
//! probes succeed on both exits without any external connectivity (the
//! kernel answers echoes addressed to its own interfaces).

mod common;

use common::{file_lock, Scenario};
use std::process::Command;

fn netns_enabled() -> bool {
    std::env::var("H5_NETNS_TEST").ok().as_deref() == Some("1")
}

fn sh(args: &[&str]) -> bool {
    Command::new(args[0])
        .args(&args[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ip_out(args: &[&str]) -> String {
    let out = Command::new("ip")
        .args(args)
        .output()
        .expect("ip must be present");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn setup_topology() -> bool {
    // Idempotent: the links survive across the serially-run tests of this
    // file (they share one namespace); only the routing state is reset.
    let links_exist =
        sh(&["ip", "link", "show", "h5wan"]) && sh(&["ip", "link", "show", "h5modem"]);
    if !links_exist {
        if !ip(&["link", "add", "h5wan", "type", "dummy"]) {
            return false;
        }
        if !ip(&["link", "add", "h5modem", "type", "dummy"]) {
            return false;
        }
        if !ip(&["addr", "add", "10.13.35.40/24", "dev", "h5wan"]) {
            return false;
        }
        if !ip(&["addr", "add", "10.13.35.41/24", "dev", "h5modem"]) {
            return false;
        }
        if !ip(&["addr", "add", "2409:8a00::40/64", "dev", "h5wan"]) {
            return false;
        }
        if !ip(&["addr", "add", "2409:8a00::41/64", "dev", "h5modem"]) {
            return false;
        }
        if !ip(&["link", "set", "h5wan", "up"]) {
            return false;
        }
        if !ip(&["link", "set", "h5modem", "up"]) {
            return false;
        }
    }
    // Reset the tables so each test bootstraps its own route layout.
    ip(&["route", "flush", "table", "main"]);
    ip(&["-6", "route", "flush", "table", "main"]);
    true
}

fn ip(args: &[&str]) -> bool {
    let mut v: Vec<&str> = vec!["ip"];
    v.extend_from_slice(args);
    sh(&v)
}

/// Scenario that uses the REAL kernel views of the namespace: only the UCI
/// config, state, lock and the daemon mocks are redirected.
fn netns_scenario(name: &str) -> Scenario {
    let mut s = Scenario::new(name);
    s.real_kernel = true;
    s.apply_env();
    s
}

fn netns_network(s: &Scenario) {
    s.write_network(
        "config interface 'loopback'\n\toption device 'lo'\n\n\
         config interface 'wan'\n\toption device 'h5wan'\n\toption proto 'static'\n\toption ipaddr '10.13.35.40'\n\toption netmask '255.255.255.0'\n\toption gateway '10.13.35.1'\n\toption metric '50'\n\n\
         config interface 'wan6'\n\toption device 'h5wan'\n\toption proto 'dhcpv6'\n\toption metric '50'\n\toption auto '1'\n\n\
         config interface 'modem0'\n\toption device 'h5modem'\n\toption proto 'static'\n\toption ipaddr '10.13.35.41'\n\toption netmask '255.255.255.0'\n\toption gateway '10.13.35.1'\n\toption metric '20'\n\n\
         config interface 'modem0_6'\n\toption device '@modem0'\n\toption proto 'dhcpv6'\n\toption metric '20'\n\toption auto '1'\n",
    );
    s.ubus_section("wan", true, false, "h5wan");
    s.ubus_section("wan6", true, false, "h5wan");
    s.ubus_section("modem0", true, false, "h5modem");
    s.ubus_section("modem0_6", true, false, "h5modem");
}

fn netns_settings(s: &Scenario, overrides: &[(&str, &str)]) {
    let mut all: Vec<(&str, &str)> = vec![
        ("probe_targets", "10.13.35.40 10.13.35.41"),
        ("probe_targets6", "2409:8a00::40 2409:8a00::41"),
        ("switch_wait_ipv4", "3"),
        ("switch_wait_ipv6", "3"),
    ];
    all.extend(overrides.iter().copied());
    s.settings(&all);
}

fn default4() -> String {
    ip_out(&["-4", "route", "show", "default"])
}

fn default6() -> String {
    ip_out(&["-6", "route", "show", "default"])
}

fn active4_is(dev: &str, metric: u32) -> bool {
    default4()
        .lines()
        .find(|l| !l.contains("from"))
        .map(|l| l.contains(dev) && l.contains(&format!("metric {metric}")))
        .unwrap_or(false)
}

#[test]
fn real_switch_commits_and_demotes_standby() {
    if !netns_enabled() {
        eprintln!("SKIP: H5_NETNS_TEST is not set (run inside `unshare -n`)");
        return;
    }
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    assert!(setup_topology(), "failed to create the dummy topology");
    let s = netns_scenario("n1");
    netns_network(&s);
    netns_settings(&s, &[("mode", "wan_first")]);
    // Bootstrap: the wired WAN carries the active slot.
    assert!(ip(&[
        "route",
        "add",
        "default",
        "via",
        "10.13.35.1",
        "dev",
        "h5wan",
        "metric",
        "10"
    ]));
    assert!(ip(&[
        "route",
        "add",
        "default",
        "via",
        "10.13.35.1",
        "dev",
        "h5modem",
        "metric",
        "50"
    ]));
    assert!(ip(&[
        "-6",
        "route",
        "add",
        "default",
        "via",
        "2409:8a00::41",
        "dev",
        "h5modem",
        "metric",
        "50"
    ]));

    // Switch to the modem: probes succeed via the local addresses.
    let started = std::time::Instant::now();
    let rc = s.run(&["set", "modem_first", "--wait"]);
    assert_eq!(rc, 0, "the switch must commit");
    assert_eq!(s.state("state"), "COMMITTED");
    assert_eq!(s.state("applied_mode"), "modem_first");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(12),
        "a warm switch must stay well inside its budget (took {:?})",
        started.elapsed()
    );

    // The FIB: modem active at 10, wan demoted to the standby slot.
    assert!(
        active4_is("h5modem", 10),
        "default4 after the switch: {}",
        default4()
    );
    assert!(
        default4().contains("h5wan") && default4().contains("metric 50"),
        "the wired exit must stay reachable as the warm standby: {}",
        default4()
    );
    // The plan round-tripped through the uci mock into the config file.
    let network = std::fs::read_to_string(s.path("config/network")).unwrap();
    assert!(
        network.contains("option metric '10'"),
        "the modem's own section must carry the active metric"
    );
    assert!(
        s.uci_ops()
            .iter()
            .any(|l| l.contains("network.modem0.metric=10")),
        "uci ops: {:?}",
        s.uci_ops()
    );
    s.cleanup();
}

#[test]
fn unreachable_target_rolls_back_without_blackholing() {
    if !netns_enabled() {
        eprintln!("SKIP: H5_NETNS_TEST is not set (run inside `unshare -n`)");
        return;
    }
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    assert!(setup_topology(), "failed to create the dummy topology");
    let s = netns_scenario("n2");
    netns_network(&s);
    // Probes can never succeed: an address nobody answers.
    netns_settings(
        &s,
        &[
            ("mode", "wan_first"),
            ("probe_targets", "10.99.99.99"),
            ("probe_targets6", "fd99::99"),
        ],
    );
    assert!(ip(&[
        "route",
        "add",
        "default",
        "via",
        "10.13.35.1",
        "dev",
        "h5wan",
        "metric",
        "10"
    ]));
    assert!(ip(&[
        "route",
        "add",
        "default",
        "via",
        "10.13.35.1",
        "dev",
        "h5modem",
        "metric",
        "50"
    ]));

    let rc = s.run(&["set", "modem_first", "--wait"]);
    assert_eq!(rc, 1, "the switch must fail");
    assert_eq!(s.state("state"), "FAILED");
    assert_eq!(s.state("reason"), "ipv4_unreachable");

    // The active exit was never disturbed: no blackhole, no dual outage.
    assert!(
        active4_is("h5wan", 10),
        "the wired exit must still own the active slot: {}",
        default4()
    );
    let v6 = default6();
    assert!(
        !v6.lines().any(|l| l.contains("metric 10")),
        "no half-committed IPv6 promotion may survive: {}",
        v6
    );
    s.cleanup();
}

#[test]
fn stable_reconcile_touches_no_route() {
    if !netns_enabled() {
        eprintln!("SKIP: H5_NETNS_TEST is not set (run inside `unshare -n`)");
        return;
    }
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    assert!(setup_topology(), "failed to create the dummy topology");
    let s = netns_scenario("n3");
    netns_network(&s);
    netns_settings(&s, &[("mode", "wan_first"), ("ipv6_owner", "wan")]);
    assert!(ip(&[
        "route",
        "add",
        "default",
        "via",
        "10.13.35.1",
        "dev",
        "h5wan",
        "metric",
        "10"
    ]));
    assert!(ip(&[
        "route",
        "add",
        "default",
        "via",
        "10.13.35.1",
        "dev",
        "h5modem",
        "metric",
        "50"
    ]));
    assert!(ip(&[
        "-6",
        "route",
        "add",
        "default",
        "via",
        "2409:8a00::40",
        "dev",
        "h5wan",
        "metric",
        "10"
    ]));

    let rc = s.run(&["set", "wan_first", "--wait"]);
    assert_eq!(rc, 0);

    let before4 = default4();
    let before6 = default6();
    let ops_before = s.uci_ops().len();
    let rc = s.run(&["reconcile"]);
    assert_eq!(rc, 0);
    let rc = s.run(&["reconcile"]);
    assert_eq!(rc, 0);

    assert_eq!(default4(), before4, "no route rewrite in a stable state");
    assert_eq!(default6(), before6, "no route rewrite in a stable state");
    assert_eq!(
        s.uci_ops().len(),
        ops_before,
        "no uci write in a stable state"
    );
    s.cleanup();
}

#[test]
fn concurrent_switch_and_hotplug_keep_one_writer() {
    if !netns_enabled() {
        eprintln!("SKIP: H5_NETNS_TEST is not set (run inside `unshare -n`)");
        return;
    }
    let _g = file_lock().lock().unwrap_or_else(|e| e.into_inner());
    assert!(setup_topology(), "failed to create the dummy topology");
    let s = netns_scenario("n4");
    netns_network(&s);
    netns_settings(&s, &[("mode", "modem_first"), ("hotplug_debounce", "1")]);
    assert!(ip(&[
        "route",
        "add",
        "default",
        "via",
        "10.13.35.1",
        "dev",
        "h5modem",
        "metric",
        "10"
    ]));
    assert!(ip(&[
        "route",
        "add",
        "default",
        "via",
        "10.13.35.1",
        "dev",
        "h5wan",
        "metric",
        "50"
    ]));

    s.apply_env();

    // A switch to the wired exit runs in the foreground thread while a
    // hotplug event arrives mid-flight: the notify worker must wait for the
    // writer lock, never start a second writer, and still run afterwards.
    let notify = std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let args = vec![
            "notify".to_string(),
            "modem0".to_string(),
            "ifup".to_string(),
        ];
        h5000m_netmode::rpc::dispatch(&args)
    });
    let rc = s.run(&["set", "wan_first", "--wait"]);
    let notify_rc = notify.join().unwrap();

    assert_eq!(rc, 0, "the switch must commit");
    assert_eq!(notify_rc, 0);
    assert_eq!(s.state("state"), "COMMITTED");
    assert_eq!(s.state("applied_mode"), "wan_first");
    assert!(
        active4_is("h5wan", 10),
        "the wired exit owns the active slot: {}",
        default4()
    );
    assert!(!s.path("lock").exists(), "the writer lock must be released");
    assert!(!s.marker().exists(), "the debounce marker must be released");
    assert_eq!(
        s.state("hotplug_runs"),
        "1",
        "the event must have run exactly once after waiting for the switch"
    );
    s.cleanup();
}
