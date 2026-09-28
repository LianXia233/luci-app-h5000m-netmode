//! H5000M NetMode – Rust backend.
//!
//! A single static ELF replaces the entire shell backend of the
//! `luci-app-h5000m-netmode` OpenWrt package. The LuCI frontend is untouched:
//! every command, argument, stdout line and exit code below is byte-compatible
//! with the legacy `/usr/sbin/h5000m-netmode` shell script and the
//! `/usr/sbin/h5000m-netmode-status` cache program.
//!
//! Module map (each one a single responsibility):
//! * `types` – domain enums/verdicts and the unified error type;
//! * `config` – UCI parsing, defaults, validation (`uci.rs` = read/write);
//! * `network` – kernel state: /proc+/sys reads, /proc route oracles,
//!   netlink route surgery;
//! * `probe` – bound ICMP/TCP probes and multi-attempt verdicts;
//! * `health` – health monitor: verdict persistence, streaks, throttle,
//!   optional layered score;
//! * `switch` – the switch transaction state machine, budget, route
//!   surgery and rollback;
//! * `monitor` – the scheduler glue (watch loop + hotplug debounce) lives in
//!   `rpc` (process-dispatch model, see below);
//! * `reconcile` – the single decision point (failover/failback/split repair);
//! * `state` – key=value state files and the writer lock;
//! * `system` – clock, logging, bounded external commands;
//! * `status` – the read-only status snapshot for `h5000m-netmode-status`;
//! * `rpc` – the CLI dispatcher that *is* the RPC surface.
//!
//! Process model: LuCI calls `fs.exec`, i.e. short-lived processes. `set` and
//! `align` therefore spawn a detached worker (`switch-worker`/`align-worker`)
//! and return immediately; the worker serialises on the state-file lock, writes
//! its progress into the state file, and LuCI polls `status`.

pub mod config;
pub mod health;
pub mod network;
pub mod probe;
pub mod reconcile;
pub mod rpc;
pub mod state;
pub mod status;
pub mod switch;
pub mod system;
pub mod types;
