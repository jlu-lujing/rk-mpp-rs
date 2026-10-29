//! dma-heap allocation, the buffer type MPP and RGA need.

use std::ffi::CString;
use std::io;
use std::os::raw::{c_int, c_ulong, c_void};

const DMA_HEAP_IOC_MAGIC: u8 = b'H';
const DMA_HEAP_IOCTL_ALLOC_NR: u32 = 0;

const fn iowr(magic: u8, nr: u32, size: u32) -> c_ulong {
    ((3u32 << 30) | (size << 16) | ((magic as u32) << 8) | nr) as c_ulong
}

#[repr(C)]
#[derive(Default)]
struct DmaHeapAllocationData {
    len: u64,
    fd: u32,
    fd_flags: u32,
    heap_flags: u64,
}

const DMA_HEAP_IOCTL_ALLOC: c_ulong = iowr(
    DMA_HEAP_IOC_MAGIC,
    DMA_HEAP_IOCTL_ALLOC_NR,
    std::mem::size_of::<DmaHeapAllocationData>() as u32,
);

pub struct DmaBuf {
    fd: c_int,
    len: usize,
    ptr: *mut u8,
}

impl DmaBuf {
    /// Allocate `len` bytes from a dma-heap and map them into our address space.
    pub fn alloc(heap: &str, len: usize, cached: bool) -> io::Result<Self> {
        let path = CString::new(format!("/dev/dma_heap/{}", heap)).unwrap();
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let page = 4096usize;
        let mut data = DmaHeapAllocationData {
            len: ((len + page - 1) / page * page) as u64,
            fd: 0,
            fd_flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
            heap_flags: 0,
        };
        let r = unsafe { libc::ioctl(fd, DMA_HEAP_IOCTL_ALLOC, &mut data as *mut _) };
        unsafe { libc::close(fd) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                data.len as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                data.fd as c_int,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            unsafe { libc::close(data.fd as c_int) };
            return Err(io::Error::last_os_error());
        }
        let _ = cached;
        Ok(DmaBuf {
            fd: data.fd as c_int,
            len: data.len as usize,
            ptr: ptr as *mut u8,
        })
    }

    pub fn fd(&self) -> c_int {
        self.fd
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for DmaBuf {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut c_void, self.len);
            libc::close(self.fd);
        }
    }
}
