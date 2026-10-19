//! Shared fixture scaffolding for the deterministic backend test suites.
//!
//! One `Scenario` is a throwaway directory that stands in for the whole
//! OpenWrt state the backend reads:
//!   * `config/`     – the `network` and `h5000m_netmode` UCI files (read
//!     in-process, written through the mini-uci mock binary);
//!   * `sysfs/`      – `/sys/class/net` (device existence, carrier, type;
//!     deliberately WITHOUT `ifindex` so accidental netlink writes fail
//!     instead of touching a real interface);
//!   * `proc/`       – `/proc/net/route` and `/proc/net/ipv6_route` in the
//!     kernel's own column formats;
//!   * `addrs4`      – the SIOCGIFCONF replacement seam (`dev ip` lines);
//!   * `ubus/`       – per-section `network.interface.<sec> status` JSON;
//!   * `state`, `health`, `lock/` – the runtime state files;
//!   * `mockbin/`    – copies of the `h5000m-test-mocks` binary named `uci`,
//!     `ubus` and `ifup`.
//!
//! Every test runs with the backend's environment seams pointed at the
//! scenario (see `Scenario::run`); tests within one file are serialised by a
//! global mutex because the seams are process-global.

#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub fn file_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub struct Scenario {
    pub dir: PathBuf,
    /// When true the backend uses the REAL kernel views (`/sys/class/net`,
    /// `/proc/net/*`, SIOCGIFCONF) instead of the scenario fixtures: only
    /// UCI config, state, lock and the daemon mocks stay redirected. Used
    /// by `tests/backend_netns.rs`, which runs inside a private network
    /// namespace so the netlink writes reach a throwaway kernel instead of
    /// the host's routing tables.
    pub real_kernel: bool,
}

fn mock_exe() -> PathBuf {
    // `CARGO_BIN_EXE_*` bakes in the BUILD HOST's absolute path, so a device
    // run (the test binary copied to the router) cannot use it. `H5M_MOCK_EXE`
    // overrides it at run time; the mock binary just has to be shipped
    // alongside the test binary.
    if let Ok(p) = std::env::var("H5M_MOCK_EXE") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_BIN_EXE_h5000m-test-mocks"))
}

impl Scenario {
    pub fn new(name: &str) -> Scenario {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "h5nm-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        for sub in ["config", "sysfs", "proc", "ubus", "mockbin", "lock"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let s = Scenario {
            dir,
            real_kernel: false,
        };
        s.install_mocks();
        s
    }

    /// Copy the mock binary under its three role names.
    fn install_mocks(&self) {
        for role in ["uci", "ubus", "ifup"] {
            let dst = self
                .dir
                .join("mockbin")
                .join(format!("{role}{}", EXE_SUFFIX));
            std::fs::copy(mock_exe(), &dst).unwrap();
            make_executable(&dst);
        }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    // -- UCI fixtures ------------------------------------------------------

    pub fn write_network(&self, text: &str) {
        std::fs::write(self.path("config/network"), text).unwrap();
    }

    pub fn write_h5(&self, text: &str) {
        std::fs::write(self.path("config/h5000m_netmode"), text).unwrap();
    }

    /// The standard settings block with per-test overrides.
    pub fn settings(&self, overrides: &[(&str, &str)]) {
        let mut text = String::from("config settings 'settings'\n");
        let set = |text: &mut String, k: &str, v: &str| {
            text.push_str(&format!("\toption {k} '{v}'\n"));
        };
        set(&mut text, "mode", "wan_first");
        set(&mut text, "watcher", "0");
        set(&mut text, "health_check", "0");
        set(&mut text, "strict_dual_stack", "0");
        set(&mut text, "switch_wait_ipv4", "2");
        set(&mut text, "switch_wait_ipv6", "2");
        set(&mut text, "switch_budget", "10");
        set(&mut text, "switch_settle", "0");
        set(&mut text, "switch_settle_warm", "0");
        set(&mut text, "probe_attempts", "1");
        set(&mut text, "probe_ok", "1");
        set(&mut text, "probe_timeout", "1");
        set(&mut text, "switch_cooldown", "20");
        set(&mut text, "hotplug_debounce", "0");
        set(&mut text, "reconcile_wait", "1");
        set(&mut text, "switch_lock_wait", "1");
        for (k, v) in overrides {
            // Replace an existing line.
            let pattern = format!("\toption {k} '");
            let mut next = String::new();
            for l in text.lines() {
                if !l.starts_with(&pattern) {
                    next.push_str(l);
                    next.push('\n');
                }
            }
            next.push_str(&format!("\toption {k} '{v}'\n"));
            text = next;
        }
        self.write_h5(&text);
    }

    /// The standard dual-uplink network config: wan/wan6 on `wan0`,
    /// modem0/modem0_6 (device `@modem0`) as the modem group.
    pub fn standard_network(&self) {
        self.write_network(
            "config interface 'loopback'\n\toption device 'lo'\n\n\
             config interface 'lan'\n\toption device 'br-lan'\n\n\
             config interface 'wan'\n\toption device 'wan0'\n\toption proto 'dhcp'\n\toption metric '50'\n\n\
             config interface 'wan6'\n\toption device 'wan0'\n\toption proto 'dhcpv6'\n\toption metric '50'\n\toption auto '1'\n\n\
             config interface 'modem0'\n\toption device 'modem0'\n\toption proto 'static'\n\toption metric '20'\n\n\
             config interface 'modem0_6'\n\toption device '@modem0'\n\toption proto 'dhcpv6'\n\toption metric '20'\n\toption auto '1'\n",
        );
    }

    // -- sysfs fixtures ----------------------------------------------------

    /// A physical netdev (type 1) with a carrier; no ifindex on purpose.
    pub fn netdev(&self, name: &str, carrier: u8) {
        let d = self.path(&format!("sysfs/{name}"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("type"), "1\n").unwrap();
        std::fs::write(d.join("carrier"), format!("{carrier}\n")).unwrap();
    }

    /// A TUN/TAP netdev (ARPHRD_NONE = 65534).
    pub fn tunnel(&self, name: &str) {
        let d = self.path(&format!("sysfs/{name}"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("type"), "65534\n").unwrap();
    }

    // -- /proc/net fixtures ------------------------------------------------

    /// Write `/proc/net/route`-format lines: (iface, gw, metric).
    /// The kernel prints the gateway as little-endian hex, so the fixture
    /// must reverse the address bytes (192.168.88.1 -> "0158A8C0").
    pub fn routes4(&self, routes: &[(&str, &str, u32)]) {
        let mut text = String::from(
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n",
        );
        for (dev, gw, metric) in routes {
            let gwhex = if gw.is_empty() {
                "00000000".to_string()
            } else {
                let o = gw.parse::<std::net::Ipv4Addr>().unwrap().octets();
                format!("{:02X}{:02X}{:02X}{:02X}", o[3], o[2], o[1], o[0])
            };
            text.push_str(&format!(
                "{dev}\t00000000\t{gwhex}\t0003\t0\t0\t{metric}\t00000000\t0\t0\t0\n"
            ));
        }
        std::fs::write(self.path("proc/route"), text).unwrap();
    }

    /// Write `/proc/net/ipv6_route`-format lines: (dev, gw, metric, src).
    pub fn routes6(&self, routes: &[(&str, &str, u32, &str)]) {
        let mut text = String::new();
        for (dev, gw, metric, src) in routes {
            let gwhex = if gw.is_empty() {
                "00000000000000000000000000000000".to_string()
            } else {
                let ip: std::net::Ipv6Addr = gw.parse().unwrap();
                ip.octets().iter().map(|b| format!("{b:02X}")).collect()
            };
            let srchex = if src.is_empty() {
                "00000000000000000000000000000000".to_string()
            } else {
                let ip: std::net::Ipv6Addr = src.parse().unwrap();
                ip.octets().iter().map(|b| format!("{b:02X}")).collect()
            };
            text.push_str(&format!(
                "00000000000000000000000000000000 00 {srchex} 00 {gwhex} {metric:08X} 00000001 00000000 00000000 {dev}\n"
            ));
        }
        std::fs::write(self.path("proc/ipv6_route"), text).unwrap();
    }

    // -- address / ubus fixtures ------------------------------------------

    /// `<dev> <ip>` lines for the SIOCGIFCONF seam.
    pub fn addrs4(&self, pairs: &[(&str, &str)]) {
        let text: String = pairs.iter().map(|(d, a)| format!("{d} {a}\n")).collect();
        std::fs::write(self.path("addrs4"), text).unwrap();
    }

    /// `/proc/net/if_inet6` lines: (dev, addr, link_local).
    pub fn addrs6(&self, triples: &[(&str, &str, bool)]) {
        let mut text = String::new();
        for (i, (dev, addr, link_local)) in triples.iter().enumerate() {
            let ifindex = (i + 1) as u32;
            let ip: std::net::Ipv6Addr = addr.parse().unwrap();
            let hex: String = ip.octets().iter().map(|b| format!("{b:02x}")).collect();
            let scope = if *link_local { "20" } else { "00" };
            text.push_str(&format!("{hex} {ifindex:02x} 40 {scope} 00 {dev}\n"));
        }
        std::fs::write(self.path("proc/if_inet6"), text).unwrap();
    }

    pub fn ubus_section(&self, sec: &str, up: bool, pending: bool, l3: &str) {
        let json = format!(
            "{{\n\t\"up\": {},\n\t\"pending\": {},\n\t\"available\": true,\n\t\"l3_device\": \"{}\",\n\t\"device\": \"{}\",\n\t\"proto\": \"static\"\n}}\n",
            up, pending, l3, l3
        );
        std::fs::write(self.path(&format!("ubus/{sec}.json")), json).unwrap();
    }

    pub fn ubus_drop(&self, sec: &str) {
        let _ = std::fs::remove_file(self.path(&format!("ubus/{sec}.json")));
    }

    // -- state ---------------------------------------------------------------

    pub fn state_write(&self, pairs: &[(&str, &str)]) {
        for (k, v) in pairs {
            let text = std::fs::read_to_string(self.path("state")).unwrap_or_default();
            let mut next = String::new();
            for l in text.lines() {
                if !l.starts_with(&format!("{k}=")) {
                    next.push_str(l);
                    next.push('\n');
                }
            }
            next.push_str(&format!("{k}={v}\n"));
            std::fs::write(self.path("state"), next).unwrap();
        }
    }

    pub fn state(&self, key: &str) -> String {
        let text = std::fs::read_to_string(self.path("state")).unwrap_or_default();
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .unwrap_or("")
            .to_string()
    }

    pub fn state_text(&self) -> String {
        std::fs::read_to_string(self.path("state")).unwrap_or_default()
    }

    pub fn uci_ops(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("uci-ops.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.to_string())
            .collect()
    }

    pub fn ifup_ops(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("ifup.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.to_string())
            .collect()
    }

    // -- running the backend -------------------------------------------------

    /// Apply the scenario environment (process-global; tests are serialised).
    ///
    /// With `real_kernel` set the three kernel-view seams
    /// (`H5000M_SYSFS_NET`/`H5000M_PROC_NET`/`H5000M_IPV4_ADDRS`) are
    /// explicitly REMOVED so a leftover value from a previous fixture-based
    /// scenario cannot leak into this one.
    pub fn apply_env(&self) {
        let dir = self.dir.to_string_lossy().to_string();
        let mut vars: Vec<(&str, String)> = vec![
            ("H5000M_CFG_FILE", format!("{dir}/config/h5000m_netmode")),
            ("H5000M_NETWORK_CFG", format!("{dir}/config/network")),
            ("H5000M_MT5700M_CFG", format!("{dir}/config/mt5700m")),
            ("H5000M_STATE_FILE", format!("{dir}/state")),
            ("H5000M_HEALTH_STATE", format!("{dir}/health")),
            ("H5000M_LOCK_DIR", format!("{dir}/lock")),
            ("H5M_MOCK_DIR", dir.clone()),
            ("H5000M_FOREGROUND", "1".to_string()),
        ];
        if self.real_kernel {
            for k in ["H5000M_SYSFS_NET", "H5000M_PROC_NET", "H5000M_IPV4_ADDRS"] {
                std::env::remove_var(k);
            }
        } else {
            vars.extend([
                ("H5000M_SYSFS_NET", format!("{dir}/sysfs")),
                ("H5000M_PROC_NET", format!("{dir}/proc")),
                ("H5000M_IPV4_ADDRS", format!("{dir}/addrs4")),
            ]);
        }
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        let uci = self
            .dir
            .join(format!("mockbin/uci{EXE_SUFFIX}"))
            .to_string_lossy()
            .to_string();
        let ubus = self
            .dir
            .join(format!("mockbin/ubus{EXE_SUFFIX}"))
            .to_string_lossy()
            .to_string();
        let ifup = self
            .dir
            .join(format!("mockbin/ifup{EXE_SUFFIX}"))
            .to_string_lossy()
            .to_string();
        std::env::set_var("H5000M_UCI_BIN", uci);
        std::env::set_var("H5000M_UBUS_BIN", ubus);
        std::env::set_var("H5000M_IFUP_BIN", ifup);
    }

    /// Run one backend dispatch under this scenario's environment.
    pub fn run(&self, args: &[&str]) -> i32 {
        self.apply_env();
        h5000m_netmode::rpc::dispatch(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    /// One read-only status render under this scenario's environment.
    pub fn status(&self) -> String {
        self.apply_env();
        let cfg = h5000m_netmode::config::AppConfig::load();
        let snap = h5000m_netmode::network::read_live_state();
        h5000m_netmode::status::render_status(&cfg, &snap)
    }

    pub fn status_field(&self, out: &str, key: &str) -> String {
        out.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .unwrap_or("")
            .to_string()
    }

    /// Marker directory used by the hotplug debounce path.
    pub fn marker(&self) -> PathBuf {
        self.path("state.debounce")
    }

    pub fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(windows)]
const EXE_SUFFIX: &str = ".exe";
#[cfg(not(windows))]
const EXE_SUFFIX: &str = "";

fn make_executable(_p: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(md) = std::fs::metadata(_p) {
            let mut perm = md.permissions();
            perm.set_mode(0o755);
            let _ = std::fs::set_permissions(_p, perm);
        }
    }
}

/// Write a /proc/uptime-shaped file with a controllable clock.
pub fn write_uptime(path: &Path, seconds: f64) {
    let mut f = std::fs::File::create(path).unwrap();
    let _ = writeln!(f, "{seconds:.2} 42.00");
}
