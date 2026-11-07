//! Destructive RK3588 reset diagnostic.
//!
//! On t6, `MPP_CMD_RESET_SESSION` can return success and leave the driver in a
//! state that wedges the next decode job. This probe therefore deliberately
//! lives outside the integration-test binary: running all ignored hardware
//! tests must never execute it before another decoder test.
//!
//! Run this only when a board reboot and command-channel access are available.
//! Do not submit a decode job after this process exits.

use rk_mpp_rs::service::{Client, MppService};
use std::io;

fn fd_count() -> io::Result<usize> {
    Ok(std::fs::read_dir("/proc/self/fd")?.count())
}

fn main() -> io::Result<()> {
    eprintln!("WARNING: destructive RESET_SESSION probe; reboot t6 before decoding again");

    let before = fd_count()?;
    for iteration in 0..32 {
        let service = MppService::open(Client::RkvDec as u32)?;
        if service.hw_id()? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("iteration {iteration}: reset client has no hardware id"),
            ));
        }
        service.reset_session()?;
        if service.hw_id()? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("iteration {iteration}: reset client lost its hardware id"),
            ));
        }
        if service.recovery_stats().session_resets != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("iteration {iteration}: reset counter was not incremented"),
            ));
        }
    }

    let after = fd_count()?;
    if after != before {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("fd leak: before={before} after={after}"),
        ));
    }
    eprintln!("32 reset/query/drop cycles left {after} fds open; reboot before decoding");
    Ok(())
}
