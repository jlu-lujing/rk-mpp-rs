//! Raw client for the Rockchip `mpp_service` kernel interface.
//!
//! The protocol is a single ioctl (`MPP_IOC_CFG_V1`) that takes a `MppReqV1`
//! describing a command, a size and a user pointer. Definitions are ported from
//! rockchip-linux/mpp: `osal/inc/mpp_service.h`, `osal/inc/mpp_dev_defs.h`.

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

pub const MPP_FLAGS_MULTI_MSG: u32 = 0x1;
pub const MPP_FLAGS_LAST_MSG: u32 = 0x2;
pub const MPP_FLAGS_REG_OFFSET_ALONE: u32 = 0x10;
pub const MPP_CMD_SET_REG_READ: u32 = 0x201;
pub const MPP_CMD_POLL_HW_FINISH: u32 = 0x300;
pub const MPP_CMD_RESET_SESSION: u32 = 0x400;
pub const MPP_CMD_TRANS_FD_TO_IOVA: u32 = 0x401;
pub const MPP_CMD_RELEASE_FD: u32 = 0x402;

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
        svc.call(MPP_CMD_INIT_CLIENT_TYPE, &client_type as *const u32 as u64, 4)?;
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
        #[repr(C)]
        struct SliceInfo {
            val: u32,
            last: u32,
        }
        #[repr(C)]
        struct PollCfg {
            poll_type: i32,
            poll_ret: i32,
            count_max: i32,
            count_ret: i32,
            slice_info: [SliceInfo; 1],
        }
        let mut cfg = PollCfg {
            poll_type: 0,
            poll_ret: 0,
            count_max: 1,
            count_ret: 0,
            slice_info: [SliceInfo { val: 0, last: 1 }],
        };
        let mut req = MppReqV1 {
            cmd: MPP_CMD_POLL_HW_FINISH,
            flag: MPP_FLAGS_LAST_MSG | MPP_FLAGS_REG_OFFSET_ALONE,
            size: std::mem::size_of::<PollCfg>() as u32,
            offset: 0,
            data_ptr: &mut cfg as *mut PollCfg as u64,
        };
        let r = unsafe { libc::ioctl(self.fd, MPP_IOC_CFG_V1, &mut req as *mut MppReqV1) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
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
        self.call(
            MPP_CMD_QUERY_HW_ID,
            &mut id as *mut u32 as u64,
            4,
        )?;
        Ok(id)
    }
}

impl Drop for MppService {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}
