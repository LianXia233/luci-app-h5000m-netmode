//! Binary entry: `/usr/sbin/h5000m-netmode`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rc = h5000m_netmode::rpc::dispatch(&args);
    ExitCode::from(rc.clamp(0, 255) as u8)
}
