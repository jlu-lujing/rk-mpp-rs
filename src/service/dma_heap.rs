//! dma-heap allocation, the buffer type MPP and RGA need.

use std::ffi::CString;
use std::fmt;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
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

fn heap_allocation_len(len: usize) -> io::Result<usize> {
    const PAGE: usize = 4096;
    if len == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dma-heap allocation length must be non-zero",
        ));
    }
    len.div_ceil(PAGE)
        .checked_mul(PAGE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "dma-heap length overflow"))
}

impl DmaBuf {
    /// Allocate `len` bytes from a dma-heap and map them into our address space.
    pub fn alloc(heap: &str, len: usize, cached: bool) -> io::Result<Self> {
        let allocation_len = heap_allocation_len(len)?;
        let path = CString::new(format!("/dev/dma_heap/{}", heap)).unwrap();
        // SAFETY: `path` is a live NUL-terminated string for the duration of
        // the call, and open does not retain the caller's pointer.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut data = DmaHeapAllocationData {
            len: allocation_len as u64,
            fd: 0,
            fd_flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
            heap_flags: 0,
        };
        // SAFETY: `fd` is the open dma-heap descriptor and `data` is a live,
        // correctly sized `DMA_HEAP_IOCTL_ALLOC` payload for this ioctl.
        let r = unsafe { libc::ioctl(fd, DMA_HEAP_IOCTL_ALLOC, &mut data as *mut _) };
        // SAFETY: this local heap descriptor is owned here and is not reused
        // after the close, whether the allocation ioctl succeeded or failed.
        unsafe { libc::close(fd) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `data.fd` is the dma-buf fd returned by the successful heap
        // allocation. The mapping is read/write, shared and exactly the
        // page-aligned allocation length accepted by the kernel.
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
            // SAFETY: the heap returned this fd, mmap failed without retaining
            // it, and no other owner exists yet.
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
        // SAFETY: `ptr` maps `data.len` writable bytes, and `data.len` still
        // describes that mapping at this point.
        unsafe { std::ptr::write_bytes(ptr, 0, data.len as usize) };
        buf.sync_write_end()?;
        let _ = cached;
        Ok(buf)
    }

    /// Duplicate and map an existing dma-buf fd.
    ///
    /// The duplicate is owned by this value; the caller may close its original
    /// fd after this function returns.
    pub fn import_fd(fd: c_int, len: usize, cached: bool) -> io::Result<Self> {
        if fd < 0 || len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "imported dma-buf needs a non-negative fd and non-zero length",
            ));
        }
        // SAFETY: this queries a caller-owned descriptor and returns a new fd
        // on success. `F_DUPFD_CLOEXEC` gives us independent close ownership.
        let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if dup < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `dup` is a fresh descriptor returned above. The mapping is
        // shared/read-write and exactly `len` bytes as promised by the caller.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                dup,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            // SAFETY: `dup` has not escaped and mmap does not retain it on
            // failure.
            unsafe { libc::close(dup) };
            return Err(io::Error::last_os_error());
        }
        let _ = cached;
        Ok(Self {
            fd: dup,
            len,
            ptr: ptr as *mut u8,
        })
    }

    /// Duplicate this buffer's fd for another driver or process.
    pub fn dup_fd(&self) -> io::Result<OwnedFd> {
        duplicate_fd(self.fd)
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Self::import_fd(self.fd, self.len, false)
    }

    pub fn fd(&self) -> c_int {
        self.fd
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: `ptr` owns a live mapping of `len` bytes, and `&mut self`
        // guarantees this is the only Rust view with write access.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `ptr` owns a live mapping of `len` bytes. Shared access is
        // sound because mutable access requires `&mut DmaBuf`.
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
        // SAFETY: this is a live dma-buf fd and `s` matches the
        // `struct dma_buf_sync` payload expected by DMA_BUF_IOCTL_SYNC.
        let r = unsafe { libc::ioctl(self.fd, DMA_BUF_IOCTL_SYNC, &mut s as *mut Sync) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// Duplicate a caller-owned descriptor without importing or mapping the
/// underlying allocation.
pub fn duplicate_fd(fd: c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot duplicate a negative fd",
        ));
    }
    // SAFETY: this queries a live descriptor and returns a fresh fd owned by
    // the caller on success.
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `duplicate` is fresh and ownership transfers exactly once.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

impl Drop for DmaBuf {
    fn drop(&mut self) {
        // SAFETY: both the mapping and fd were created together by `alloc`,
        // are owned solely by this DmaBuf, and are each released exactly once.
        // munmap may report a diagnostic for an external unmap, but the fd is
        // still valid to close immediately afterward.
        unsafe {
            libc::munmap(self.ptr as *mut c_void, self.len);
            libc::close(self.fd);
        }
    }
}

impl AsRawFd for DmaBuf {
    fn as_raw_fd(&self) -> c_int {
        self.fd
    }
}

impl fmt::Debug for DmaBuf {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DmaBuf")
            .field("fd", &self.fd)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{heap_allocation_len, DMA_BUF_IOCTL_SYNC};

    #[test]
    fn allocation_length_rejects_zero_and_overflow() {
        assert_eq!(heap_allocation_len(1).unwrap(), 4096);
        assert_eq!(heap_allocation_len(4096).unwrap(), 4096);
        assert_eq!(heap_allocation_len(4097).unwrap(), 8192);
        assert_eq!(
            heap_allocation_len(0).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            heap_allocation_len(usize::MAX).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn sync_ioctl_encodes_dma_buf_sync_size() {
        // _IOW('b', 0, struct dma_buf_sync) on a 64-bit target.
        assert_eq!(DMA_BUF_IOCTL_SYNC, 0x40086200);
    }

    #[test]
    fn import_rejects_invalid_arguments_before_dup() {
        assert_eq!(
            super::DmaBuf::import_fd(-1, 4096, false)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            super::DmaBuf::import_fd(0, 0, false).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            super::duplicate_fd(-1).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
