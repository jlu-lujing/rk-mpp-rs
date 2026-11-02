//! Raw client for the Rockchip `mpp_service` kernel interface.
//!
//! The protocol is a single ioctl (`MPP_IOC_CFG_V1`) that takes a `MppReqV1`
//! describing a command, a size and a user pointer. Definitions are ported from
//! rockchip-linux/mpp: `osal/inc/mpp_service.h`, `osal/inc/mpp_dev_defs.h`.

pub mod buffer_pool;
pub mod dma_heap;
pub mod iep_session;
pub mod task_queue;

use std::ffi::CString;
use std::io;
use std::os::raw::{c_int, c_ulong};

#[allow(dead_code)]
pub const _UNUSED: () = ();

pub const MPP_SERVICE_PATH: &str = "/dev/mpp_service";

const MPP_IOC_MAGIC: u8 = b'v';
const MPP_IOC_NR_CFG_V1: u32 = 1;

// _IOW(magic, nr, u32)
const fn iow(magic: u8, nr: u32, size: u32) -> c_ulong {
    ((1u32 << 30) | (size << 16) | ((magic as u32) << 8) | nr) as c_ulong
}

pub const MPP_IOC_CFG_V1: c_ulong = iow(MPP_IOC_MAGIC, MPP_IOC_NR_CFG_V1, 4);

/// Commands (MppServiceCmdType).
pub const MPP_CMD_PROBE_HW_SUPPORT: u32 = 0;
pub const MPP_CMD_QUERY_HW_ID: u32 = 1;
pub const MPP_CMD_QUERY_CMD_SUPPORT: u32 = 2;
pub const MPP_CMD_INIT_CLIENT_TYPE: u32 = 0x100;
pub const MPP_CMD_SET_REG_WRITE: u32 = 0x200;

const MPP_CMD_QUERY_BASE: u32 = 0;
const MPP_CMD_INIT_BASE: u32 = 0x100;
const MPP_CMD_SEND_BASE: u32 = 0x200;
const MPP_CMD_POLL_BASE: u32 = 0x300;
const MPP_CMD_CONTROL_BASE: u32 = 0x400;

pub const MPP_CMD_INIT_DRIVER_DATA: u32 = 0x101;
pub const MPP_CMD_INIT_TRANS_TABLE: u32 = 0x102;
pub const MPP_CMD_SET_REG_READ: u32 = 0x201;
pub const MPP_CMD_SET_REG_ADDR_OFFSET: u32 = 0x202;
pub const MPP_CMD_SET_RCB_INFO: u32 = 0x203;
pub const MPP_CMD_SET_SESSION_FD: u32 = 0x204;
pub const MPP_CMD_POLL_HW_IRQ: u32 = 0x301;
pub const MPP_CMD_RESET_SESSION: u32 = 0x400;
pub const MPP_CMD_TRANS_FD_TO_IOVA: u32 = 0x401;
pub const MPP_CMD_RELEASE_FD: u32 = 0x402;
pub const MPP_CMD_SEND_CODEC_INFO: u32 = 0x403;
pub const MPP_CMD_SET_ERR_REF_HACK: u32 = 0x404;

pub const MPP_FLAGS_MULTI_MSG: u32 = 0x1;
pub const MPP_FLAGS_LAST_MSG: u32 = 0x2;
pub const MPP_FLAGS_REG_OFFSET_ALONE: u32 = 0x10;
/// `MPP_FLAGS_POLL_NON_BLOCK`: let `POLL_HW_FINISH` return `EAGAIN` instead
/// of waiting for the interrupt. Used by `MppService::poll_hw_finish_try`.
pub const MPP_FLAGS_POLL_NON_BLOCK: u32 = 0x20;
/// `MPP_FLAGS_SECURE_MODE`, carried on requests for a secure (TA) session.
/// Capability survey in `docs/framework-m10.md`: we expose the bit and refuse
/// secure work, the RK3588 test board has no TA path to verify against.
pub const MPP_FLAGS_SECURE_MODE: u32 = 0x1_0000;
pub const MPP_CMD_POLL_HW_FINISH: u32 = 0x300;

/// Client types (MppClientType).
pub const VPU_CLIENT_JPEG_DEC: u32 = 13;
pub const VPU_CLIENT_RKVDEC: u32 = 9;

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct MppReqV1 {
    pub cmd: u32,
    pub flag: u32,
    pub size: u32,
    pub offset: u32,
    pub data_ptr: u64,
}

const _: () = assert!(std::mem::size_of::<MppReqV1>() == 24);

pub struct MppService {
    fd: c_int,
    client_type: u32,
}

impl MppService {
    /// Open the device and announce which hardware block we are going to drive.
    pub fn open(client_type: u32) -> io::Result<Self> {
        let path = CString::new(MPP_SERVICE_PATH).unwrap();
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let svc = MppService { fd, client_type };
        svc.call(
            MPP_CMD_INIT_CLIENT_TYPE,
            &client_type as *const u32 as u64,
            4,
        )?;
        Ok(svc)
    }

    pub fn client_type(&self) -> u32 {
        self.client_type
    }

    /// One ioctl round trip.
    pub fn call(&self, cmd: u32, data: u64, size: u32) -> io::Result<i32> {
        let mut req = MppReqV1 {
            cmd,
            flag: 0,
            size,
            offset: 0,
            data_ptr: data,
        };
        let r = unsafe { libc::ioctl(self.fd, MPP_IOC_CFG_V1, &mut req as *mut MppReqV1) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r)
        }
    }

    /// Send the register file for one job and wait for the hardware.
    pub fn submit_regs(&self, regs: &[u32]) -> io::Result<()> {
        let mut req = MppReqV1 {
            cmd: MPP_CMD_SET_REG_WRITE,
            flag: MPP_FLAGS_LAST_MSG,
            size: (regs.len() * 4) as u32,
            offset: 0,
            data_ptr: regs.as_ptr() as u64,
        };
        let r = unsafe { libc::ioctl(self.fd, MPP_IOC_CFG_V1, &mut req as *mut MppReqV1) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Poll until the job finishes.
    pub fn poll_hw_finish(&self) -> io::Result<()> {
        // Linux mpp_service uses the no-payload form of POLL_HW_FINISH when
        // HW_IRQ is unavailable.  The driver writes all queued SET_REG_READ
        // destinations before this ioctl returns.
        let mut req = MppReqV1 {
            cmd: MPP_CMD_POLL_HW_FINISH,
            flag: MPP_FLAGS_LAST_MSG | MPP_FLAGS_REG_OFFSET_ALONE,
            size: 0,
            offset: 0,
            data_ptr: 0,
        };
        let r = unsafe { libc::ioctl(self.fd, MPP_IOC_CFG_V1, &mut req as *mut MppReqV1) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Non-blocking variant of `poll_hw_finish`: carries
    /// `MPP_FLAGS_POLL_NON_BLOCK` so the driver answers `EAGAIN` when the
    /// job is still running. `Ok(true)` means finished, `Ok(false)` means
    /// not yet; other errors propagate. The exact errno contract on this
    /// kernel is verified by `scripts/board_framework_async_vs_sync.sh`,
    /// which only uses it as an extra check around the blocking poll.
    pub fn poll_hw_finish_try(&self) -> io::Result<bool> {
        let mut req = MppReqV1 {
            cmd: MPP_CMD_POLL_HW_FINISH,
            flag: MPP_FLAGS_LAST_MSG | MPP_FLAGS_REG_OFFSET_ALONE | MPP_FLAGS_POLL_NON_BLOCK,
            size: 0,
            offset: 0,
            data_ptr: 0,
        };
        let r = unsafe { libc::ioctl(self.fd, MPP_IOC_CFG_V1, &mut req as *mut MppReqV1) };
        if r < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN) {
                return Ok(false);
            }
            return Err(err);
        }
        Ok(true)
    }

    /// Hardware support bitmask (bit N == VPU_CLIENT_* N is present).
    pub fn probe_hw_support(&self) -> io::Result<u32> {
        let mut mask = 0u32;
        self.call(MPP_CMD_PROBE_HW_SUPPORT, &mut mask as *mut u32 as u64, 4)?;
        Ok(mask)
    }

    /// Hardware id of our client block, as reported by the driver.
    pub fn hw_id(&self) -> io::Result<u32> {
        let mut id = 0u32;
        self.call(MPP_CMD_QUERY_HW_ID, &mut id as *mut u32 as u64, 4)?;
        Ok(id)
    }
}

impl Drop for MppService {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

// ---------------------------------------------------------------------------
// Full command surface
// ---------------------------------------------------------------------------

/// Client types (MppClientType); bit N of the hardware-support mask means the
/// block is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Client {
    Vdpu1 = 0,
    Vdpu2 = 1,
    Vdpu1Pp = 2,
    Vdpu2Pp = 3,
    Av1Dec = 4,
    HevcDec = 8,
    RkvDec = 9,
    AvsPlusDec = 12,
    JpegDec = 13,
    RkvEnc = 16,
    Vepu1 = 17,
    Vepu2 = 18,
    Vepu2Jpeg = 19,
    JpegEnc = 20,
    Vepu22 = 24,
    /// `IEP_CLIENT_TYPE`: the image enhance processor, deinterlace and YUV scale.
    Iep = 28,
    /// `VDPP_CLIENT_TYPE`: the standalone video post-processor.
    Vdpp = 29,
}

impl Client {
    /// Every block the official MPP can drive.
    pub fn all() -> &'static [(Client, &'static str)] {
        &[
            (Client::Vdpu1, "vdpu1 (H.264/MPEG4 decode)"),
            (Client::Vdpu2, "vdpu2 (H.264/MPEG4 decode)"),
            (Client::Vdpu1Pp, "vdpu1 post-processor"),
            (Client::Vdpu2Pp, "vdpu2 post-processor"),
            (Client::Av1Dec, "AV1 decode"),
            (Client::HevcDec, "HEVC decode"),
            (Client::RkvDec, "rkvdec (H.264/H.265/VP9/AVS2)"),
            (Client::AvsPlusDec, "AVS+ decode"),
            (Client::JpegDec, "JPEG decode"),
            (Client::RkvEnc, "rkv encoder (H.264/H.265)"),
            (Client::Vepu1, "vepu1 encoder"),
            (Client::Vepu2, "vepu2 encoder"),
            (Client::Vepu2Jpeg, "vepu JPEG encoder"),
            (Client::JpegEnc, "JPEG encoder"),
            (Client::Vepu22, "vepu22 encoder"),
            (Client::Iep, "IEP (deinterlace, YUV scale)"),
            (Client::Vdpp, "VDPP (scale, rotate, CSC, sharpen)"),
        ]
    }
}

/// (register index, byte offset) for the address-offset command.
#[repr(C)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegOffsetInfo {
    pub reg_idx: u32,
    pub offset: u32,
}

/// External register-command-buffer descriptor.
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct RcbInfo {
    pub reg_idx: u32,
    pub size: u32,
}

/// `MPP_CMD_SEND_CODEC_INFO` entry.
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct DevInfoCfg {
    pub type_: u32,
    pub flag: u32,
    pub data: u64,
}

const _: () = assert!(std::mem::size_of::<DevInfoCfg>() == 16);

#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
pub struct SliceInfo {
    pub val: u32,
    pub last: u32,
}

/// A chain of requests delivered in one ioctl (`MPP_FLAGS_MULTI_MSG`).
#[derive(Default)]
pub struct Batch {
    reqs: Vec<MppReqV1>,
}

impl Batch {
    pub fn new() -> Self {
        Batch {
            reqs: Vec::with_capacity(8),
        }
    }

    pub fn clear(&mut self) {
        self.reqs.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.reqs.is_empty()
    }

    pub fn push(&mut self, cmd: u32, flag: u32, size: u32, offset: u32, data: u64) {
        self.reqs.push(MppReqV1 {
            cmd,
            flag,
            size,
            offset,
            data_ptr: data,
        });
    }

    /// The buffers referenced here must outlive `MppService::submit_batch`.
    pub fn reg_write(&mut self, regs: &[u32], offset: u32) -> &mut Self {
        self.push(
            MPP_CMD_SET_REG_WRITE,
            0,
            (regs.len() * 4) as u32,
            offset,
            regs.as_ptr() as u64,
        );
        self
    }

    pub fn reg_read(&mut self, regs: &mut [u32], offset: u32) -> &mut Self {
        self.push(
            MPP_CMD_SET_REG_READ,
            0,
            (regs.len() * 4) as u32,
            offset,
            regs.as_mut_ptr() as u64,
        );
        self
    }

    pub fn reg_offsets(&mut self, offs: &[RegOffsetInfo]) -> &mut Self {
        self.push(
            MPP_CMD_SET_REG_ADDR_OFFSET,
            MPP_FLAGS_REG_OFFSET_ALONE,
            (offs.len() * std::mem::size_of::<RegOffsetInfo>()) as u32,
            0,
            offs.as_ptr() as u64,
        );
        self
    }

    pub fn rcb_info(&mut self, rcb: &[RcbInfo]) -> &mut Self {
        self.push(
            MPP_CMD_SET_RCB_INFO,
            0,
            (rcb.len() * std::mem::size_of::<RcbInfo>()) as u32,
            0,
            rcb.as_ptr() as u64,
        );
        self
    }

    pub fn codec_info(&mut self, info: &[DevInfoCfg]) -> &mut Self {
        self.push(
            MPP_CMD_SEND_CODEC_INFO,
            MPP_FLAGS_LAST_MSG,
            (info.len() * std::mem::size_of::<DevInfoCfg>()) as u32,
            0,
            info.as_ptr() as u64,
        );
        self
    }
}

impl MppService {
    /// One ioctl carrying the whole request chain, with the last-message flags.
    /// Encode/decode add `MPP_FLAGS_REG_OFFSET_ALONE`; the IEP2 job does not.
    pub fn submit_batch_plain(&self, batch: &mut Batch) -> io::Result<()> {
        if batch.reqs.is_empty() {
            return Ok(());
        }
        let n = batch.reqs.len();
        for (i, r) in batch.reqs.iter_mut().enumerate() {
            if i + 1 < n {
                r.flag |= MPP_FLAGS_MULTI_MSG;
            } else {
                r.flag |= MPP_FLAGS_LAST_MSG;
            }
        }
        let r = unsafe {
            libc::ioctl(
                self.fd,
                MPP_IOC_CFG_V1,
                batch.reqs.as_mut_ptr() as *mut MppReqV1,
            )
        };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// One ioctl carrying the whole request chain.
    pub fn submit_batch(&self, batch: &mut Batch) -> io::Result<()> {
        if batch.reqs.is_empty() {
            return Ok(());
        }
        let n = batch.reqs.len();
        for (i, r) in batch.reqs.iter_mut().enumerate() {
            if i + 1 < n {
                r.flag |= MPP_FLAGS_MULTI_MSG;
            } else {
                r.flag |= MPP_FLAGS_LAST_MSG | MPP_FLAGS_REG_OFFSET_ALONE;
            }
        }
        let r = unsafe {
            libc::ioctl(
                self.fd,
                MPP_IOC_CFG_V1,
                batch.reqs.as_mut_ptr() as *mut MppReqV1,
            )
        };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Is this command implemented by the running kernel?
    pub fn cmd_supported(&self, cmd: u32) -> bool {
        let mut caps = [0u32; 5];
        for (cap, base) in caps.iter_mut().zip([
            MPP_CMD_QUERY_BASE,
            MPP_CMD_INIT_BASE,
            MPP_CMD_SEND_BASE,
            MPP_CMD_POLL_BASE,
            MPP_CMD_CONTROL_BASE,
        ]) {
            *cap = base;
            if self
                .call(MPP_CMD_QUERY_CMD_SUPPORT, cap as *mut u32 as u64, 4)
                .is_err()
            {
                return false;
            }
        }
        command_supported(cmd, &caps)
    }

    pub fn reset_session(&self) -> io::Result<()> {
        self.call(MPP_CMD_RESET_SESSION, 0, 0).map(|_| ())
    }

    /// dma-buf fd -> IOVA, for callers that patch addresses themselves.
    pub fn fd_to_iova(&self, fd: c_int) -> io::Result<u32> {
        let mut v = fd as u32;
        self.call(MPP_CMD_TRANS_FD_TO_IOVA, &mut v as *mut u32 as u64, 4)?;
        Ok(v)
    }

    pub fn release_fd(&self, fd: c_int) -> io::Result<()> {
        let mut v = fd as u32;
        self.call(MPP_CMD_RELEASE_FD, &mut v as *mut u32 as u64, 4)
            .map(|_| ())
    }

    /// Non-blocking poll with a slice table.
    pub fn poll_irq(&self, slices: &mut [SliceInfo]) -> io::Result<()> {
        #[repr(C)]
        struct Cfg {
            poll_type: i32,
            poll_ret: i32,
            count_max: i32,
            count_ret: i32,
        }
        let mut c = Cfg {
            poll_type: 0,
            poll_ret: 0,
            count_max: slices.len() as i32,
            count_ret: 0,
        };
        let mut buf = Vec::with_capacity(std::mem::size_of::<Cfg>() + slices.len() * 8);
        unsafe {
            std::ptr::copy_nonoverlapping(
                &c as *const Cfg as *const u8,
                buf.as_mut_ptr(),
                std::mem::size_of::<Cfg>(),
            );
            std::ptr::copy_nonoverlapping(
                slices.as_ptr() as *const u8,
                buf.as_mut_ptr().add(std::mem::size_of::<Cfg>()),
                slices.len() * 8,
            );
        }
        let mut req = MppReqV1 {
            cmd: MPP_CMD_POLL_HW_IRQ,
            flag: MPP_FLAGS_LAST_MSG | MPP_FLAGS_REG_OFFSET_ALONE,
            size: (std::mem::size_of::<Cfg>() + slices.len() * 8) as u32,
            offset: 0,
            data_ptr: buf.as_mut_ptr() as u64,
        };
        let r = unsafe { libc::ioctl(self.fd, MPP_IOC_CFG_V1, &mut req as *mut MppReqV1) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        let _ = &mut c;
        Ok(())
    }
}

fn command_supported(cmd: u32, caps: &[u32; 5]) -> bool {
    cmd < caps[0]
        || (MPP_CMD_INIT_BASE..caps[1]).contains(&cmd)
        || (MPP_CMD_SEND_BASE..caps[2]).contains(&cmd)
        || (MPP_CMD_POLL_BASE..caps[3]).contains(&cmd)
        || (MPP_CMD_CONTROL_BASE..caps[4]).contains(&cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_capabilities_exclude_unimplemented_rcb_info() {
        let caps = [0x3, 0x103, 0x203, 0x302, 0x404];

        assert!(command_supported(MPP_CMD_SET_REG_ADDR_OFFSET, &caps));
        assert!(!command_supported(MPP_CMD_SET_RCB_INFO, &caps));
        assert!(command_supported(MPP_CMD_POLL_HW_FINISH, &caps));
    }
}
