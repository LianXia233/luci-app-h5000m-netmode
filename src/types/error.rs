//! Unified error type. The legacy shell backend reports failures as short
//! reason strings (`ipv4_not_ready`, `lock_busy`, ...) that the LuCI page
//! renders verbatim; the variants here carry those same strings so RPC
//! compatibility holds while the code stays `Result`-typed.

use std::fmt;

#[derive(Debug, Clone)]
pub enum Error {
    Config(String),
    Network(String),
    Probe(String),
    Route(String),
    Switch(String),
    Rpc(String),
    System(String),
    Io(String),
}

impl Error {
    pub fn config(m: impl Into<String>) -> Error {
        Error::Config(m.into())
    }
    pub fn network(m: impl Into<String>) -> Error {
        Error::Network(m.into())
    }
    pub fn probe(m: impl Into<String>) -> Error {
        Error::Probe(m.into())
    }
    pub fn route(m: impl Into<String>) -> Error {
        Error::Route(m.into())
    }
    pub fn switch(m: impl Into<String>) -> Error {
        Error::Switch(m.into())
    }
    pub fn rpc(m: impl Into<String>) -> Error {
        Error::Rpc(m.into())
    }
    pub fn system(m: impl Into<String>) -> Error {
        Error::System(m.into())
    }
    pub fn io(m: impl Into<String>) -> Error {
        Error::Io(m.into())
    }

    /// A short stable tag usable as a `reason=` value in the state file.
    pub fn tag(&self) -> &'static str {
        match self {
            Error::Config(_) => "config_error",
            Error::Network(_) => "network_error",
            Error::Probe(_) => "probe_error",
            Error::Route(_) => "route_error",
            Error::Switch(_) => "switch_error",
            Error::Rpc(_) => "rpc_error",
            Error::System(_) => "system_error",
            Error::Io(_) => "io_error",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(m)
            | Error::Network(m)
            | Error::Probe(m)
            | Error::Route(m)
            | Error::Switch(m)
            | Error::Rpc(m)
            | Error::System(m)
            | Error::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Convenience: convert a `std::io::Error` into our `Io` variant.
pub fn io_err(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

/// Parse helpers shared across modules.
pub fn parse_u32(s: &str, default: u32) -> u32 {
    match s.trim().parse::<u32>() {
        Ok(v) => v,
        Err(_) => default,
    }
}

/// `number_or`: numeric coercion with a fallback.
pub fn num_or<T: std::str::FromStr + Copy>(s: &str, default: T) -> T {
    s.trim().parse::<T>().unwrap_or(default)
}
