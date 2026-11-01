//! Reference-counted dma-heap buffer pool.
//!
//! The C counterpart is `mpp/base/mpp_buffer.c`: an `MppBufferGroup` keeps
//! buffers in a size-bucketed cache, hands them out through
//! `mpp_buffer_get` (bumping a reference count), `mpp_buffer_ref` /
//! `mpp_buffer_unref` adjust the count, and the *last* unref returns the
//! buffer to the cache instead of freeing it. A group limit configured with
//! `mpp_buffer_group_limit_config` makes `mpp_buffer_get` fail (return NULL)
//! rather than block once `count_used` is reached. This module ports that
//! behaviour onto [`dma_heap::DmaBuf`].
//!
//! # Mapping of the C semantics
//!
//! | C concept | our counterpart | notes |
//! |---|---|---|
//! | `MppBufferGroup` | [`BufferPool`] | `Clone`-able handle onto one shared pool, like passing the group pointer around |
//! | `mpp_buffer_get(group, &buf, size)` | [`BufferPool::acquire`] | reuses a cached buffer that is big enough, allocates otherwise, fails at the limit |
//! | `mpp_buffer_ref` | [`BufferPool::try_clone`] | `Err` once the buffer was fully released |
//! | `mpp_buffer_unref` | [`BufferPool::release`] | last release moves the slot to `Cached` (kept for reuse, like the C group cache); a second release of the same handle reports [`PoolError::Released`] instead of the C assert/UB |
//! | `mpp_buffer_group_limit_config(group, size, count)` | `limit` in the constructor | counted per slot rather than per size bucket — see deviations |
//! | `mpp_buffer_group_import` / fd sharing | [`BufferPool::import`] / [`BufferPool::dup_fd`] | import takes an existing [`dma_heap::DmaBuf`] (an imported dma-buf wrapped at the caller) and caches it |
//!
//! Deliberate deviations, and why:
//!
//! * C caches by exact aligned size (a bucket per size); we reuse the
//!   smallest cached buffer that is **large enough** for the request. Strict
//!   bucketing wastes memory for the common "same frame size every time"
//!   pattern this pool serves and offers no benefit in our single-tag use.
//! * Reaching the limit never frees a cached buffer to make room; the
//!   acquire fails (non-blocking submit, like `mpp_buffer_get` returning
//!   NULL) or waits ([`BufferPool::acquire_timeout`], our host-side
//!   queueing policy — C has none, we add the option and default to the C
//!   behaviour).
//! * A buffer handed out while another handle still references it is
//!   *never* exposed: a partial release keeps the slot `Live`, so a racing
//!   [`acquire`](Self::acquire) allocates or fails instead of sharing a
//!   reference with the previous owner. C relies on the caller; we enforce.
//!
//! # Reference lifetime and the DPB question
//!
//! W5-4 also names "DPB release timing". The decoder-side DPB in this repo
//! already recycles frames when their DPB entry goes away (M2 checklist);
//! when the DPB moves onto pooled buffers, each displayed frame holds one
//! [`BufferHandle`], every consumer (reference list, user output port) takes
//! a `try_clone`, and the frame returns to the pool exactly when the last
//! consumer drops it — the same event C models as "unref count zero after
//! output port dequeue". The state machine below is what makes that
//! discipline checkable at run time instead of debug-assert-only.
//!
//! # Multi-session use
//!
//! The pool is `Send + Sync`; several task-queue workers may acquire from
//! one pool concurrently. For rkvenc's one-session-per-process rule
//! (HANDOFF §6.13) pool sharing is still fine — buffer allocation is not the
//! serialised part, the session is; see [`super::task_queue::SessionGate`].

use std::io;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use super::dma_heap::DmaBuf;
use super::task_queue::PollTimeout;

/// A pool-managed memory block. Implemented by
/// [`DmaBuf`] and by [`PlainBuf`] (host tests / CPU-only use).
pub trait PoolBuffer: Send {
    /// Usable length in bytes.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// dma-buf fd for hardware jobs, `None` for CPU-only blocks.
    fn fd(&self) -> Option<i32> {
        None
    }

    /// Contiguous CPU view for reads; backends without contiguous memory
    /// return an empty slice rather than lie about their layout.
    fn bytes(&self) -> &[u8] {
        &[]
    }

    /// Contiguous CPU view for writes.
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut []
    }
}

impl PoolBuffer for DmaBuf {
    fn len(&self) -> usize {
        DmaBuf::len(self)
    }

    fn fd(&self) -> Option<i32> {
        Some(DmaBuf::fd(self))
    }

    fn bytes(&self) -> &[u8] {
        DmaBuf::as_slice(self)
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        DmaBuf::as_mut_slice(self)
    }
}

/// Plain heap buffer, the fake backend host tests run the pool on (no
/// `/dev/dma_heap` on a dev machine). An fd can be attached so the import
/// and export paths exercise without dma-buf.
pub struct PlainBuf {
    bytes: Vec<u8>,
    fd: Option<i32>,
}

impl PlainBuf {
    pub fn new(len: usize) -> PlainBuf {
        PlainBuf {
            bytes: vec![0u8; len],
            fd: None,
        }
    }

    pub fn with_fd(len: usize, fd: i32) -> PlainBuf {
        PlainBuf {
            bytes: vec![0u8; len],
            fd: Some(fd),
        }
    }
}

impl PoolBuffer for PlainBuf {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn fd(&self) -> Option<i32> {
        self.fd
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

/// What went wrong in a pool operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolError {
    /// Every slot is in use and the pool is at its configured limit.
    Full,
    /// A timed acquire gave up before a slot freed.
    TimedOut,
    /// The handle is not live: already fully released, or never ours. The
    /// C path here is a debug assert or plain use-after-free; we report it.
    Released,
    /// Reference count overflow on `try_clone` (effectively unreachable).
    TooManyRefs,
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoolError::Full => f.write_str("buffer pool at limit"),
            PoolError::TimedOut => f.write_str("buffer pool acquire timed out"),
            PoolError::Released => f.write_str("stale or double-released buffer handle"),
            PoolError::TooManyRefs => f.write_str("buffer reference count overflow"),
        }
    }
}

impl std::error::Error for PoolError {}

/// Effect of [`BufferPool::release`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseEffect {
    /// Other handles still reference the buffer; the slot stays live.
    StillHeld,
    /// This was the last reference; the buffer is cached for reuse.
    Cached,
}

/// A handle to one pool buffer. Handles are values, not guards: the pool
/// validates every operation against its own state, which is what allows
/// double release and use-after release to be *detected* (see
/// [`PoolError::Released`]) instead of silently accepted or panicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferHandle {
    id: u64,
}

#[derive(Debug)]
enum Occupancy<B> {
    Empty,
    /// Released by its last handle, retained for reuse (C: buffer in the
    /// group's idle list).
    Cached(B),
    /// Handed out. `refs` is the number of outstanding handles.
    Live {
        buf: B,
        refs: u32,
        id: u64,
    },
}

impl<B> Occupancy<B> {
    fn is_live(&self) -> bool {
        matches!(self, Occupancy::Live { .. })
    }
}

/// Pool statistics (W5-9 bookkeeping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Total slots ever created (never shrinks; `clear` only empties cached
    /// ones back to `Empty` without freeing capacity structure).
    pub slots: usize,
    /// Slots currently cached, not in use.
    pub cached: usize,
    /// Slots currently handed out.
    pub in_use: usize,
    /// Configured limit.
    pub limit: usize,
    /// Backend allocations performed (reuses never count).
    pub allocations: usize,
}

struct PoolCore<B> {
    limit: usize,
    slots: Vec<Occupancy<B>>,
    next_id: u64,
    allocations: usize,
    shut_down: bool,
}

// Lock-free core logic. Everything the runtime must obey about reference
// counts and cache reuse is decided here so it is unit-testable in place.
impl<B: PoolBuffer> PoolCore<B> {
    fn find_slot(&self, id: u64) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| matches!(s, Occupancy::Live { id: sid, .. } if *sid == id))
    }

    /// Smallest cached slot with `len >= need`, mirroring our documented
    /// best-fit reuse (C uses exact-size buckets).
    fn find_cached(&self, need: usize) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s {
                Occupancy::Cached(buf) if buf.len() >= need => Some((i, buf.len())),
                _ => None,
            })
            .min_by_key(|&(_, len)| len)
            .map(|(i, _)| i)
    }

    fn refs_of(&self, id: u64) -> Option<u32> {
        let idx = self.find_slot(id)?;
        match &self.slots[idx] {
            Occupancy::Live { refs, .. } => Some(*refs),
            _ => None,
        }
    }

    fn clone_ref(&mut self, id: u64) -> Result<(), PoolError> {
        let idx = self.find_slot(id).ok_or(PoolError::Released)?;
        match &mut self.slots[idx] {
            Occupancy::Live { refs, .. } => {
                *refs = (*refs).checked_add(1).ok_or(PoolError::TooManyRefs)?;
                Ok(())
            }
            _ => unreachable!(),
        }
    }

    fn release(&mut self, id: u64) -> Result<ReleaseEffect, PoolError> {
        let idx = self.find_slot(id).ok_or(PoolError::Released)?;
        match &mut self.slots[idx] {
            Occupancy::Live { refs, .. } => {
                if *refs > 1 {
                    *refs -= 1;
                    Ok(ReleaseEffect::StillHeld)
                } else {
                    // Last reference: park the buffer in the cache instead
                    // of freeing (C: mpp_buffer_unref -> group idle list).
                    // The Live slot (with its id) is replaced by Cached, so
                    // every handle to this buffer is now stale.
                    let buf = match std::mem::replace(&mut self.slots[idx], Occupancy::Empty) {
                        Occupancy::Live { buf, .. } => buf,
                        _ => unreachable!(),
                    };
                    self.slots[idx] = Occupancy::Cached(buf);
                    Ok(ReleaseEffect::Cached)
                }
            }
            _ => unreachable!(),
        }
    }
}

struct Shared<B> {
    core: Mutex<PoolCore<B>>,
    space_cv: Condvar,
    allocator: Box<dyn Fn(usize) -> io::Result<B> + Send + Sync>,
}

/// A refcounted buffer group, `Clone` throughout the program like the C
/// group pointer.
pub struct BufferPool<B = DmaBuf> {
    shared: Arc<Shared<B>>,
}

impl<B> Clone for BufferPool<B> {
    fn clone(&self) -> BufferPool<B> {
        BufferPool {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl BufferPool<DmaBuf> {
    /// Pool over the `system` dma-heap with at most `limit` buffers.
    /// Allocates lazily; nothing is opened until the first acquire.
    pub fn new(limit: usize) -> BufferPool<DmaBuf> {
        BufferPool::with_allocator(limit, |len| DmaBuf::alloc("system", len, false))
    }
}

impl<B: PoolBuffer + 'static> BufferPool<B> {
    /// Pool whose memory comes from `allocator` — [`BufferPool::new`] uses
    /// the dma-heap, host tests use [`PlainBuf`].
    pub fn with_allocator(
        limit: usize,
        allocator: impl Fn(usize) -> io::Result<B> + Send + Sync + 'static,
    ) -> BufferPool<B> {
        assert!(limit > 0);
        BufferPool {
            shared: Arc::new(Shared {
                core: Mutex::new(PoolCore {
                    limit,
                    slots: Vec::new(),
                    next_id: 1,
                    allocations: 0,
                    shut_down: false,
                }),
                space_cv: Condvar::new(),
                allocator: Box::new(allocator),
            }),
        }
    }

    /// Non-blocking acquire: reuse a cached buffer that fits or allocate a
    /// new one. [`PoolError::Full`] once every slot is live and the limit is
    /// reached — the C `mpp_buffer_get`-returns-NULL behaviour.
    pub fn acquire(&self, size: usize) -> Result<BufferHandle, PoolError> {
        self.acquire_inner(size, None, None)
    }

    /// Acquire with the queue's poll semantics ([`PollTimeout`] mirrors
    /// `MppPollType`): `Block` waits for a slot to free, `Ms` gives up with
    /// [`PoolError::TimedOut`], `NonBlock` equals [`acquire`](Self::acquire).
    /// This is our host-side queuing option; C never blocks here.
    pub fn acquire_timeout(
        &self,
        size: usize,
        timeout: PollTimeout,
    ) -> Result<BufferHandle, PoolError> {
        self.acquire_inner(size, timeout.duration(), Some(timeout))
    }

    fn acquire_inner(
        &self,
        size: usize,
        budget: Option<std::time::Duration>,
        kind: Option<PollTimeout>,
    ) -> Result<BufferHandle, PoolError> {
        let start = std::time::Instant::now();
        let mut core = self.shared.core.lock().unwrap();
        loop {
            if core.shut_down {
                return Err(PoolError::Full);
            }
            if let Some(idx) = core.find_cached(size) {
                let buf = match std::mem::replace(&mut core.slots[idx], Occupancy::Empty) {
                    Occupancy::Cached(buf) => buf,
                    _ => unreachable!(),
                };
                let id = core.next_id;
                core.next_id += 1;
                core.slots[idx] = Occupancy::Live { buf, refs: 1, id };
                return Ok(BufferHandle { id });
            }
            if core.slots.len() < core.limit
                || core
                    .slots
                    .iter()
                    .position(|s| matches!(s, Occupancy::Empty))
                    .is_some()
            {
                // No cached slot fits: create one through the backend,
                // refilling a slot emptied by `clear` before growing. The
                // lock is held across the allocation; dma-heap allocs are
                // ~100 us and the alternative (a second mutex) buys little.
                let buf = (self.shared.allocator)(size).map_err(|_| PoolError::Full)?;
                core.allocations += 1;
                let id = core.next_id;
                core.next_id += 1;
                let slot = core
                    .slots
                    .iter()
                    .position(|s| matches!(s, Occupancy::Empty))
                    .unwrap_or_else(|| {
                        core.slots.push(Occupancy::Empty);
                        core.slots.len() - 1
                    });
                core.slots[slot] = Occupancy::Live { buf, refs: 1, id };
                return Ok(BufferHandle { id });
            }
            // At the limit and nothing cached.
            match (budget, kind) {
                (Some(_), Some(PollTimeout::NonBlock)) | (None, None) => {
                    return Err(PoolError::Full)
                }
                _ => match budget {
                    None => core = self.shared.space_cv.wait(core).unwrap(),
                    Some(b) => {
                        let Some(remaining) = b.checked_sub(start.elapsed()) else {
                            return Err(PoolError::TimedOut);
                        };
                        let (c, wait) = self.shared.space_cv.wait_timeout(core, remaining).unwrap();
                        core = c;
                        if wait.timed_out() {
                            return Err(PoolError::TimedOut);
                        }
                    }
                },
            }
        }
    }

    /// Outstanding references on a live handle (`None` once released).
    pub fn refs(&self, handle: BufferHandle) -> Option<u32> {
        self.shared.core.lock().unwrap().refs_of(handle.id)
    }

    /// The `mpp_buffer_ref` of this design: a second live handle on the same
    /// buffer. [`PoolError::Released`] after the last release — including
    /// after the slot was *reused* by someone else, which can never alias
    /// because ids are monotonic and never reassigned.
    pub fn try_clone(&self, handle: BufferHandle) -> Result<BufferHandle, PoolError> {
        self.shared.core.lock().unwrap().clone_ref(handle.id)?;
        Ok(handle)
    }

    /// The `mpp_buffer_unref` of this design, validated: a double release or
    /// a release of a foreign handle is reported, never ignored.
    pub fn release(&self, handle: BufferHandle) -> Result<ReleaseEffect, PoolError> {
        let effect = self.shared.core.lock().unwrap().release(handle.id);
        if effect.is_ok() {
            self.shared.space_cv.notify_one();
        }
        effect
    }

    /// Borrow the buffer contents for a hardware result read or a CPU fill.
    /// [`PoolError::Released`] is the use-after-release refusal: once the
    /// last handle was released — even if the slot was re-acquired by a new
    /// owner — the old handle yields nothing.
    pub fn data(&self, handle: BufferHandle) -> Result<PoolRef<'_, B>, PoolError> {
        let core = self.shared.core.lock().unwrap();
        let idx = core.find_slot(handle.id).ok_or(PoolError::Released)?;
        Ok(PoolRef { core, idx })
    }

    /// Mutable view (CPU writes before a job); same refusal semantics.
    pub fn data_mut(&self, handle: BufferHandle) -> Result<PoolRefMut<'_, B>, PoolError> {
        let core = self.shared.core.lock().unwrap();
        let idx = core.find_slot(handle.id).ok_or(PoolError::Released)?;
        Ok(PoolRefMut { core, idx })
    }

    /// The dma-buf fd for submitting this buffer to the hardware
    /// (`mpp_buffer_get_fd`). `None` for CPU-only blocks.
    pub fn fd(&self, handle: BufferHandle) -> Option<i32> {
        let core = self.shared.core.lock().unwrap();
        let idx = core.find_slot(handle.id)?;
        match &core.slots[idx] {
            Occupancy::Live { buf, .. } => buf.fd(),
            _ => None,
        }
    }

    /// Share the underlying dma-buf with another process/driver: a private
    /// fd (caller-owned, closed on drop). Buffer must be live.
    #[cfg(unix)]
    pub fn dup_fd(&self, handle: BufferHandle) -> Result<std::os::fd::OwnedFd, PoolError> {
        use std::os::fd::FromRawFd;
        let raw = self.fd(handle).ok_or(PoolError::Released)?;
        let dup = unsafe { libc::dup(raw) };
        if dup < 0 {
            return Err(PoolError::Released);
        }
        Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(dup) })
    }

    /// Insert an externally created buffer as cached idle stock — the
    /// `mpp_buffer_group_import` direction. With [`DmaBuf`] this is how an
    /// imported dma-buf (wrapped by the caller at construction) joins the
    /// pool; host tests verify the logic with [`PlainBuf`].
    pub fn import(&self, buf: B) -> Result<(), PoolError> {
        let mut core = self.shared.core.lock().unwrap();
        if core.slots.len() >= core.limit {
            return Err(PoolError::Full);
        }
        core.slots.push(Occupancy::Cached(buf));
        Ok(())
    }

    /// Free every cached (idle) buffer back to the backend. Live slots are
    /// left alone; `in_use` in the report says how many.
    pub fn clear(&self) -> PoolStats {
        let mut core = self.shared.core.lock().unwrap();
        for slot in core.slots.iter_mut() {
            if matches!(slot, Occupancy::Cached(_)) {
                *slot = Occupancy::Empty;
            }
        }
        Self::stats_of(&core)
    }

    /// Refuse new acquires (drain support for pool-owning teardown paths).
    pub fn shut_down(&self) {
        self.shared.core.lock().unwrap().shut_down = true;
        self.shared.space_cv.notify_all();
    }

    pub fn stats(&self) -> PoolStats {
        let core = self.shared.core.lock().unwrap();
        Self::stats_of(&core)
    }

    fn stats_of(core: &PoolCore<B>) -> PoolStats {
        PoolStats {
            slots: core.slots.len(),
            cached: core
                .slots
                .iter()
                .filter(|s| matches!(s, Occupancy::Cached(_)))
                .count(),
            in_use: core.slots.iter().filter(|s| s.is_live()).count(),
            limit: core.limit,
            allocations: core.allocations,
        }
    }
}

/// Guarded read view of a live buffer (see [`BufferPool::data`]).
pub struct PoolRef<'a, B> {
    core: MutexGuard<'a, PoolCore<B>>,
    idx: usize,
}

impl<B: PoolBuffer> Deref for PoolRef<'_, B> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match &self.core.slots[self.idx] {
            Occupancy::Live { buf, .. } => buf.bytes(),
            _ => &[],
        }
    }
}

/// Guarded write view of a live buffer (see [`BufferPool::data_mut`]).
pub struct PoolRefMut<'a, B> {
    core: MutexGuard<'a, PoolCore<B>>,
    idx: usize,
}

impl<B: PoolBuffer> Deref for PoolRefMut<'_, B> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match &self.core.slots[self.idx] {
            Occupancy::Live { buf, .. } => buf.bytes(),
            _ => &[],
        }
    }
}

impl<B: PoolBuffer> DerefMut for PoolRefMut<'_, B> {
    fn deref_mut(&mut self) -> &mut [u8] {
        match &mut self.core.slots[self.idx] {
            Occupancy::Live { buf, .. } => buf.bytes_mut(),
            _ => &mut [],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A counting fake backend: allocation count is the reuse witness.
    fn fake_pool(limit: usize) -> (BufferPool<PlainBuf>, Arc<AtomicUsize>) {
        let allocs = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&allocs);
        let pool = BufferPool::with_allocator(limit, move |len| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(PlainBuf::new(len))
        });
        (pool, allocs)
    }

    #[test]
    fn released_buffers_are_reused_not_refreed() {
        let (pool, allocs) = fake_pool(4);
        let h = pool.acquire(1024).unwrap();
        assert_eq!(allocs.load(Ordering::SeqCst), 1);
        assert_eq!(pool.release(h), Ok(ReleaseEffect::Cached));
        let h2 = pool.acquire(1024).unwrap();
        assert_eq!(allocs.load(Ordering::SeqCst), 1, "cache hit, no re-alloc");
        let _ = pool.release(h2);
        // Larger request still fits the cached block (best-fit reuse).
        let h3 = pool.acquire(512).unwrap();
        assert_eq!(allocs.load(Ordering::SeqCst), 1);
        let _ = pool.release(h3);
        // A request that does not fit must allocate.
        let h4 = pool.acquire(2048).unwrap();
        assert_eq!(allocs.load(Ordering::SeqCst), 2);
        let _ = pool.release(h4);
    }

    #[test]
    fn refcount_gates_the_return_to_the_pool() {
        let (pool, allocs) = fake_pool(4);
        let h1 = pool.acquire(256).unwrap();
        let h2 = pool.try_clone(h1).unwrap();
        let h3 = pool.try_clone(h1).unwrap();
        assert_eq!(pool.refs(h1), Some(3));
        assert_eq!(pool.release(h1), Ok(ReleaseEffect::StillHeld));
        assert_eq!(pool.refs(h2), Some(2));
        assert_eq!(pool.release(h3), Ok(ReleaseEffect::StillHeld));
        // Still one reference: the buffer must NOT be hailable by a new
        // acquire, so the pool allocates (limit 4 > 1 live slot).
        let other = pool.acquire(256).unwrap();
        assert_eq!(
            allocs.load(Ordering::SeqCst),
            2,
            "in-flight buffer was not handed out"
        );
        assert_eq!(pool.release(h2), Ok(ReleaseEffect::Cached));
        assert_eq!(pool.refs(h2), None);
        let _ = pool.release(other);
    }

    #[test]
    fn double_release_is_reported_not_forced() {
        let (pool, _) = fake_pool(2);
        let h = pool.acquire(64).unwrap();
        assert_eq!(pool.release(h), Ok(ReleaseEffect::Cached));
        assert_eq!(pool.release(h), Err(PoolError::Released));
        assert_eq!(pool.try_clone(h), Err(PoolError::Released));
    }

    #[test]
    fn use_after_release_never_sees_the_recycled_slot() {
        let (pool, _) = fake_pool(2);
        let h = pool.acquire(64).unwrap();
        assert_eq!(pool.release(h), Ok(ReleaseEffect::Cached));
        // The slot is now recycled with a new id; the old handle must not
        // touch it, even though it is the same physical buffer.
        let fresh = pool.acquire(64).unwrap();
        assert_ne!(fresh.id, h.id, "ids are never reused");
        assert!(pool.data(h).is_err(), "use-after-release refused");
        assert!(pool.data_mut(h).is_err());
        assert!(pool.fd(h).is_none());
        assert!(pool.data(fresh).is_ok());
        let _ = pool.release(fresh);
    }

    #[test]
    fn limit_failure_policy_matches_mpp_buffer_get() {
        let (pool, _) = fake_pool(2);
        let a = pool.acquire(64).unwrap();
        let b = pool.acquire(64).unwrap();
        assert_eq!(pool.acquire(64), Err(PoolError::Full));
        // blocking acquire with a budget times out while full...
        assert_eq!(
            pool.acquire_timeout(64, PollTimeout::Ms(10)),
            Err(PoolError::TimedOut)
        );
        // ...and a blocking acquire lands once a slot frees.
        let last = a;
        assert_eq!(pool.release(last), Ok(ReleaseEffect::Cached));
        let got = pool.acquire_timeout(64, PollTimeout::Block).unwrap();
        assert!(pool.data(got).is_ok());
        let _ = pool.release(b);
        let _ = pool.release(got);
    }

    #[test]
    fn blocking_acquire_wakes_on_release_from_another_thread() {
        let (pool, _) = fake_pool(1);
        let held = pool.acquire(64).unwrap();
        let waiter = {
            let pool = pool.clone();
            std::thread::spawn(move || pool.acquire_timeout(64, PollTimeout::Block))
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(pool.release(held), Ok(ReleaseEffect::Cached));
        let got = waiter.join().unwrap().expect("waiter must be woken");
        assert!(pool.data(got).is_ok());
        let _ = pool.release(got);
    }

    #[test]
    fn import_adds_foreign_stock_export_dupes_fd() {
        // 7 is a fake fd number; dup is attempted only on unix and the
        // fake fd is invalid there, so exercise the bookkeeping with data
        // and the fd() lookup instead.
        let (pool, allocs) = fake_pool(3);
        pool.import(PlainBuf::with_fd(4096, 7)).unwrap();
        let h = pool.acquire(2048).unwrap();
        assert_eq!(allocs.load(Ordering::SeqCst), 0, "imported stock reused");
        assert_eq!(pool.fd(h), Some(7));
        assert_eq!(pool.data(h).unwrap().len(), 4096);
        // over-limit imports are refused like acquires
        pool.import(PlainBuf::new(16)).unwrap();
        pool.import(PlainBuf::new(16)).unwrap();
        assert_eq!(pool.import(PlainBuf::new(16)), Err(PoolError::Full));
        let _ = pool.release(h);
    }

    #[test]
    fn clear_only_drops_idle_buffers() {
        let (pool, allocs) = fake_pool(3);
        let live = pool.acquire(1024).unwrap();
        let idle = pool.acquire(1024).unwrap();
        assert_eq!(pool.release(idle), Ok(ReleaseEffect::Cached));
        let report = pool.clear();
        assert_eq!(
            report,
            PoolStats {
                slots: 2,
                cached: 0,
                in_use: 1,
                limit: 3,
                allocations: 2,
            }
        );
        // After a clear the stock is gone: the next acquire allocates, into
        // the slot emptied by the clear rather than a new one.
        let again = pool.acquire(1024).unwrap();
        assert_eq!(allocs.load(Ordering::SeqCst), 3);
        assert_eq!(pool.stats().slots, 2, "empty slot refilled");
        assert!(pool.data(again).unwrap().iter().all(|&b| b == 0));
        let _ = pool.release(again);
        let _ = pool.release(live);
    }

    #[test]
    fn stats_track_the_full_lifecycle() {
        let (pool, _) = fake_pool(2);
        assert_eq!(
            pool.stats(),
            PoolStats {
                slots: 0,
                cached: 0,
                in_use: 0,
                limit: 2,
                allocations: 0
            }
        );
        let a = pool.acquire(64).unwrap();
        let b = pool.try_clone(a).unwrap();
        assert_eq!(pool.stats().in_use, 1);
        let _ = pool.release(b);
        assert_eq!(pool.stats().in_use, 1);
        let _ = pool.release(a);
        assert_eq!(pool.stats().cached, 1);
        pool.shut_down();
        assert_eq!(pool.acquire(64), Err(PoolError::Full));
    }
}
