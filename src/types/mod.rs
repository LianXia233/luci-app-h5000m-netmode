//! Shared domain types for the H5000M netmode backend.
//!
//! Everything here is deliberately `Copy`-friendly plain data: the hot paths
//! (health rounds, switch transactions, status output) run many times per
//! second on router-class CPUs and must not allocate.

pub use crate::types::error::{num_or, parse_u32, Error, Result};
pub mod error;

/// The two exit groups. Kept as a plain enum with a lossless `str` mapping so
/// status output and UCI config agree byte-for-byte with the legacy shell
/// backend (the LuCI frontend parses `group_primary`/`group_active` etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Wan,
    Modem,
}

impl Group {
    pub const ALL: [Group; 2] = [Group::Wan, Group::Modem];

    pub fn as_str(self) -> &'static str {
        match self {
            Group::Wan => "wan",
            Group::Modem => "modem",
        }
    }

    pub fn parse(s: &str) -> Option<Group> {
        match s {
            "wan" => Some(Group::Wan),
            "modem" => Some(Group::Modem),
            _ => None,
        }
    }

    /// `other_group`: the standby partner of a group.
    pub fn other(self) -> Group {
        match self {
            Group::Wan => Group::Modem,
            Group::Modem => Group::Wan,
        }
    }

    /// `group_label`: the short label used in logs.
    pub fn label(self) -> &'static str {
        match self {
            Group::Wan => "wan",
            Group::Modem => "5g",
        }
    }
}

impl std::fmt::Display for Group {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Address family. `4`/`6` parse to keep call sites identical to the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    pub const ALL: [Family; 2] = [Family::V4, Family::V6];

    pub fn n(self) -> u8 {
        match self {
            Family::V4 => 4,
            Family::V6 => 6,
        }
    }

    /// Default kernel metric when a route line carries none: IPv4 defaults to
    /// 0, IPv6 to 1024 (the kernel's own default for RA routes).
    pub fn default_metric(self) -> u32 {
        match self {
            Family::V4 => 0,
            Family::V6 => 1024,
        }
    }

    pub fn from_n(n: u8) -> Option<Family> {
        match n {
            4 => Some(Family::V4),
            6 => Some(Family::V6),
            _ => None,
        }
    }

    /// Probe family label used by the health cache (`wan_fail`, `wan6_fail`).
    pub fn suffix(self) -> &'static str {
        match self {
            Family::V4 => "",
            Family::V6 => "6",
        }
    }
}

impl std::fmt::Display for Family {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.n())
    }
}

/// A reachability verdict. `Unknown` is not a failure: a family with no
/// address/route cannot be probed and must not accumulate a failure streak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Up,
    Down,
    Unknown,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Up => "1",
            Verdict::Down => "0",
            Verdict::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Verdict {
        match s {
            "1" => Verdict::Up,
            "0" => Verdict::Down,
            _ => Verdict::Unknown,
        }
    }

    pub fn is_up(self) -> bool {
        matches!(self, Verdict::Up)
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Exit policy modes, normalized exactly like the shell `normalize_mode()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    WanFirst,
    ModemFirst,
    WanOnly,
    ModemOnly,
}

impl Mode {
    pub const ALL: [Mode; 4] = [
        Mode::WanFirst,
        Mode::ModemFirst,
        Mode::WanOnly,
        Mode::ModemOnly,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::WanFirst => "wan_first",
            Mode::ModemFirst => "modem_first",
            Mode::WanOnly => "wan_only",
            Mode::ModemOnly => "modem_only",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "wan_first" => Some(Mode::WanFirst),
            "modem_first" => Some(Mode::ModemFirst),
            "wan_only" => Some(Mode::WanOnly),
            "modem_only" => Some(Mode::ModemOnly),
            _ => None,
        }
    }

    /// `mode_primary_group`: the group that owns the active metric slot.
    pub fn primary_group(self) -> Group {
        match self {
            Mode::WanFirst | Mode::WanOnly => Group::Wan,
            Mode::ModemFirst | Mode::ModemOnly => Group::Modem,
        }
    }

    /// `mode_is_only`: the standby group is isolated (no default route at all).
    pub fn is_only(self) -> bool {
        matches!(self, Mode::WanOnly | Mode::ModemOnly)
    }
}

/// Normalize a mode string the way `normalize_mode` does (wan_first defaults).
pub fn normalize_mode(s: &str) -> Mode {
    Mode::parse(s).unwrap_or(Mode::WanFirst)
}

/// State machine states, spelled exactly like the shell constants so the state
/// file stays byte-compatible (the status program and the LuCI page parse them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmState {
    Idle,
    Preparing,
    WaitIpv4,
    WaitIpv6,
    VerifyIpv4,
    VerifyIpv6,
    Switching,
    VerifyTarget,
    Committed,
    Rollback,
    Failed,
}

impl SmState {
    pub const IDLE: &'static str = "IDLE";
    pub const PREPARING: &'static str = "PREPARING_TARGET";
    pub const WAIT4: &'static str = "WAIT_IPV4";
    pub const WAIT6: &'static str = "WAIT_IPV6";
    pub const VERIFY4: &'static str = "VERIFY_IPV4";
    pub const VERIFY6: &'static str = "VERIFY_IPV6";
    pub const SWITCHING: &'static str = "SWITCHING";
    pub const VERIFY_TARGET: &'static str = "VERIFY_TARGET";
    pub const COMMITTED: &'static str = "COMMITTED";
    pub const ROLLBACK: &'static str = "ROLLBACK";
    pub const FAILED: &'static str = "FAILED";

    pub fn as_str(self) -> &'static str {
        match self {
            SmState::Idle => SmState::IDLE,
            SmState::Preparing => SmState::PREPARING,
            SmState::WaitIpv4 => SmState::WAIT4,
            SmState::WaitIpv6 => SmState::WAIT6,
            SmState::VerifyIpv4 => SmState::VERIFY4,
            SmState::VerifyIpv6 => SmState::VERIFY6,
            SmState::Switching => SmState::SWITCHING,
            SmState::VerifyTarget => SmState::VERIFY_TARGET,
            SmState::Committed => SmState::COMMITTED,
            SmState::Rollback => SmState::ROLLBACK,
            SmState::Failed => SmState::FAILED,
        }
    }

    /// `switch_in_progress` predicate: only the terminal states are idle.
    pub fn is_terminal(self) -> bool {
        matches!(self, SmState::Idle | SmState::Committed | SmState::Failed)
    }

    /// Parse a state-file state name (unknown names become Idle).
    pub fn parse(s: &str) -> SmState {
        match s {
            SmState::PREPARING => SmState::Preparing,
            SmState::WAIT4 => SmState::WaitIpv4,
            SmState::WAIT6 => SmState::WaitIpv6,
            SmState::VERIFY4 => SmState::VerifyIpv4,
            SmState::VERIFY6 => SmState::VerifyIpv6,
            SmState::SWITCHING => SmState::Switching,
            SmState::VERIFY_TARGET => SmState::VerifyTarget,
            SmState::COMMITTED => SmState::Committed,
            SmState::ROLLBACK => SmState::Rollback,
            SmState::FAILED => SmState::Failed,
            _ => SmState::Idle,
        }
    }
}

/// `switch_busy` semantics: a state is busy when non-terminal *and* the worker
/// pid that wrote it is still alive.
pub fn switch_busy(state: SmState, pid_alive: bool) -> bool {
    !state.is_terminal() && pid_alive
}
