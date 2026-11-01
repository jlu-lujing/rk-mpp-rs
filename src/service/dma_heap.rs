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

/// DMA_BUF_IOCTL_SYNC, `_IOW('b', 0, struct dma_buf_sync)`
const DMA_BUF_IOCTL_SYNC: c_ulong =
    (1u32 << 30 | (std::mem::size_of::<u64>() as u32) << 16 | (('b' as u32) << 8)) as c_ulong;
const DMA_BUF_SYNC_READ: u64 = 1 << 0;
const DMA_BUF_SYNC_WRITE: u64 = 1 << 1;
const DMA_BUF_SYNC_START: u64 = 0 << 2;
const DMA_BUF_SYNC_END: u64 = 1 << 2;

pub struct DmaBuf {
    fd: c_int,
    len: usize,
    ptr: *mut u8,
}

// A dma-buf fd and its mapping have no thread affinity: ownership of the
// whole buffer can move to another thread, and the `DMA_BUF_SYNC_*` ioctls
// only require the fd. Exclusive access is still enforced by the borrow
// checker (`as_mut_slice` takes `&mut self`), so the buffer is `Send` but
// deliberately *not* `Sync`. This is what lets the framework hand pooled
// buffers to task-queue worker threads.
unsafe impl Send for DmaBuf {}

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
        let buf = DmaBuf {
            fd: data.fd as c_int,
            len: data.len as usize,
            ptr: ptr as *mut u8,
        };
        // dma-heap does not promise zero-filled memory. The hardware tables
        // are fixed-size and only partially rewritten for each frame, so
        // stale contents from a previous allocation can change decode output.
        buf.sync_write_begin()?;
        unsafe { std::ptr::write_bytes(ptr, 0, data.len as usize) };
        buf.sync_write_end()?;
        let _ = cached;
        Ok(buf)
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

    /// Tell the kernel the CPU is about to read this buffer (cache invalidate).
    pub fn sync_read_begin(&self) -> io::Result<()> {
        self.sync(DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ)
    }

    /// Finish the CPU read section.
    pub fn sync_read_end(&self) -> io::Result<()> {
        self.sync(DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ)
    }

    /// Tell the kernel the CPU is about to write this buffer.
    pub fn sync_write_begin(&self) -> io::Result<()> {
        self.sync(DMA_BUF_SYNC_START | DMA_BUF_SYNC_WRITE)
    }

    /// Flush CPU writes before handing the buffer to a DMA device.
    pub fn sync_write_end(&self) -> io::Result<()> {
        self.sync(DMA_BUF_SYNC_END | DMA_BUF_SYNC_WRITE)
    }

    fn sync(&self, flags: u64) -> io::Result<()> {
        #[repr(C)]
        struct Sync {
            flags: u64,
        }
        let mut s = Sync { flags };
        let r = unsafe { libc::ioctl(self.fd, DMA_BUF_IOCTL_SYNC, &mut s as *mut Sync) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
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

#[cfg(test)]
mod tests {
    use super::DMA_BUF_IOCTL_SYNC;

    #[test]
    fn sync_ioctl_encodes_dma_buf_sync_size() {
        // _IOW('b', 0, struct dma_buf_sync) on a 64-bit target.
        assert_eq!(DMA_BUF_IOCTL_SYNC, 0x40086200);
    }
}
