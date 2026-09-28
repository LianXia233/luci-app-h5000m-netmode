//! System helpers: unified logging, the monotonic centisecond clock and the
//! bounded external-command wrapper.

pub mod clock;
pub mod command;
pub mod log;

pub use clock::{fmt_cs, now_cs, unix_ts};
pub use log::{log_error, log_info, log_warn};
