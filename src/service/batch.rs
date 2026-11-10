//! Cross-session `mpp_service` batching, the Rust counterpart of C MPP's
//! `mpp_server`.
//!
//! C opens one additional `/dev/mpp_service` descriptor as the batch server.
//! Each task is prefixed by `MPP_CMD_SET_SESSION_FD`, whose `MppBatCmd.client`
//! names the real session descriptor. Sending several tasks through that server
//! lets the kernel's CCU schedule independent sessions together instead of
//! issuing one ioctl per session from userspace.

use std::ffi::CString;
use std::io;
use std::os::raw::c_int;

use super::{
    ioctl_mpp, Batch, MppBatCmd, MppReqV1, MppService, MPP_CMD_SET_SESSION_FD, MPP_FLAGS_LAST_MSG,
    MPP_FLAGS_MULTI_MSG, MPP_FLAGS_REG_OFFSET_ALONE, MPP_SERVICE_PATH,
};

/// C's default `mpp_server_batch_task` limit.
pub const MAX_BATCH_TASKS: usize = 8;

/// One already-built request batch and the session it belongs to.
pub struct BatchJob<'a> {
    pub session: &'a mut MppService,
    pub batch: &'a mut Batch,
}

/// A batch server descriptor, separate from every hardware session fd.
pub struct BatchServer {
    fd: c_int,
    client_type: u32,
}

impl BatchServer {
    /// Does this kernel expose the multi-session delimiter command?
    pub fn available(client_type: u32) -> io::Result<bool> {
        let service = MppService::open(client_type)?;
        Ok(service.cmd_supported(MPP_CMD_SET_SESSION_FD))
    }

    pub fn open(client_type: u32) -> io::Result<Self> {
        if !Self::available(client_type)? {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "kernel mpp_service does not support SET_SESSION_FD batching",
            ));
        }
        let path = CString::new(MPP_SERVICE_PATH).unwrap();
        // SAFETY: `path` is a live NUL-terminated string and the batch server
        // owns the descriptor it returns.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, client_type })
    }

    pub fn client_type(&self) -> u32 {
        self.client_type
    }

    /// Submit up to [`MAX_BATCH_TASKS`] independent session batches, then poll
    /// every real session until all tasks complete.
    ///
    /// Each `Batch` must already have its register writes, offsets and reads
    /// in the order the caller wants. The request buffers referenced by those
    /// commands must remain alive for the duration of this call.
    pub fn submit(&self, jobs: &mut [BatchJob<'_>]) -> io::Result<Vec<io::Result<()>>> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        if jobs.len() > MAX_BATCH_TASKS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "mpp_service batch has {} tasks, maximum is {MAX_BATCH_TASKS}",
                    jobs.len()
                ),
            ));
        }
        if jobs
            .iter()
            .any(|job| job.session.client_type() != self.client_type)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "all batch tasks must belong to client type {}",
                    self.client_type
                ),
            ));
        }
        if jobs.iter().any(|job| job.batch.requests().is_empty()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an empty request batch cannot be submitted",
            ));
        }

        // Allocate the complete command array before taking pointers into it.
        let mut commands: Vec<MppBatCmd> = jobs
            .iter()
            .map(|job| MppBatCmd {
                flag: 0,
                client: job.session.client_fd(),
                ret: 0,
            })
            .collect();
        let mut send = Vec::new();
        for (index, job) in jobs.iter().enumerate() {
            let command = &mut commands[index] as *mut MppBatCmd;
            send.push(MppReqV1 {
                cmd: MPP_CMD_SET_SESSION_FD,
                flag: MPP_FLAGS_MULTI_MSG,
                size: std::mem::size_of::<MppBatCmd>() as u32,
                offset: 0,
                data_ptr: command as u64,
            });
            let requests = job.batch.requests();
            for (request_index, request) in requests.iter().enumerate() {
                let mut request = *request;
                request.flag |= MPP_FLAGS_MULTI_MSG;
                request.flag &= !MPP_FLAGS_LAST_MSG;
                if request_index + 1 == requests.len() {
                    request.flag |= MPP_FLAGS_REG_OFFSET_ALONE;
                    if index + 1 == jobs.len() {
                        request.flag |= MPP_FLAGS_LAST_MSG;
                    }
                }
                send.push(request);
            }
        }

        ioctl_mpp(self.fd, send.as_mut_ptr())?;

        // The kernel's `mpp_dev_ioctl` triggers every external session before
        // waiting. t6's 6.1 MPP service does not populate `mpp_bat_msg.ret`
        // for this path, so use each real session's supported poll ioctl as the
        // completion channel. All jobs are already pending in the hardware
        // queues before the first wait.
        Ok(jobs
            .iter_mut()
            .map(|job| job.session.poll_hw_finish_recovering())
            .collect())
    }
}

impl Drop for BatchServer {
    fn drop(&mut self) {
        if self.fd >= 0 {
            // SAFETY: this object exclusively owns the descriptor returned by
            // `open`, and `fd` is never shared or cloned.
            unsafe { libc::close(self.fd) };
            self.fd = -1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_batch_limit_matches_c_mpp_server() {
        assert_eq!(MAX_BATCH_TASKS, 8);
    }
}
