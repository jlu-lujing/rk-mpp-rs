use std::os::fd::AsRawFd;

use rk_mpp_rs::hal::av1d::Frame as Av1Frame;
use rk_mpp_rs::hal::h264d::Frame;
use rk_mpp_rs::hal::hevc as hevc_hal;
use rk_mpp_rs::hal::vp9d::Frame as Vp9Frame;
use rk_mpp_rs::service::buffer_pool::BufferPool;
use rk_mpp_rs::service::task_queue::{PollTimeout, TaskOutcome, TaskQueue};
use rk_mpp_rs::service::{Client, MppService};

/// The pool tests in the library run against a fake allocator (a host has
/// no dma-heap); these tests exercise the real DmaBuf backend and every
/// codec's DPB lease pair.
#[test]
#[ignore = "requires /dev/dma_heap/system"]
fn dma_heap_pool_reuses_across_release_and_refcount() {
    let pool = BufferPool::new(3);
    let a = pool.acquire(1 << 20).unwrap();
    let fd0 = pool.fd(a).unwrap();
    assert_eq!(pool.stats().allocations, 1);
    let b = pool.try_clone(a).unwrap();
    assert_eq!(
        pool.release(a),
        Ok(rk_mpp_rs::service::buffer_pool::ReleaseEffect::StillHeld)
    );
    assert_eq!(
        pool.release(b),
        Ok(rk_mpp_rs::service::buffer_pool::ReleaseEffect::Cached)
    );
    let c = pool.acquire(1 << 20).unwrap();
    assert_eq!(pool.stats().allocations, 1, "cached dma-buf was reused");
    assert_eq!(pool.fd(c).unwrap(), fd0, "same physical block");
    // An in-flight block must never be handed out a second time.
    let d = pool.acquire(1 << 20).unwrap();
    assert_eq!(pool.stats().allocations, 2, "in-flight block not recycled");
    let _ = pool.release(c);
    let e = pool.acquire(1 << 20).unwrap();
    assert_eq!(pool.fd(e).unwrap(), fd0, "slot returned to the cache");
    assert_eq!(pool.stats().allocations, 2);
    let _ = pool.release(d);
    let _ = pool.release(e);
}

#[test]
#[ignore = "requires /dev/dma_heap/system and /dev/mpp_service"]
fn lease_fd_maps_to_a_stable_iova_and_exports_a_duplicate_fd() {
    let pool = BufferPool::new(1);
    let lease = pool.acquire_lease(1 << 20).unwrap();
    let fd = lease.fd().unwrap();
    let duplicate = lease.dup_fd().unwrap();
    assert_ne!(duplicate.as_raw_fd(), fd);

    let service = MppService::open(Client::RkvDec as u32).unwrap();
    let first = service.fd_to_iova(fd).unwrap();
    let second = service.fd_to_iova(fd).unwrap();
    assert_ne!(first, 0, "kernel returned a null IOVA");
    assert_eq!(first, second, "mapping is stable for a live dma-buf");
}

#[test]
#[ignore = "requires /dev/dma_heap/system"]
fn pool_feeds_queue_workers_under_concurrency() {
    let pool = BufferPool::new(8);
    let queue = TaskQueue::new(4);
    let mut ids = Vec::new();
    // A finished-but-untaken job pins its slot (the queue mirrors C's output
    // port nobody dequeues), so submitting all 32 up front would wedge the
    // queue after four completions. Take the first two back every two
    // submissions - the stream drains as it fills.
    let mut inflight = Vec::new();
    for _ in 0..32 {
        let pool = pool.clone();
        ids.push(
            queue
                .submit_blocking(
                    move || {
                        let size = 1 << 16;
                        let h = match pool.acquire(size) {
                            Ok(h) => h,
                            Err(_) => return TaskOutcome::Failed("pool full".into()),
                        };
                        let fd = pool.fd(h);
                        let ok =
                            fd.is_some() && pool.data(h).map(|v| v.len() >= size).unwrap_or(false);
                        let _ = pool.release(h);
                        if ok {
                            TaskOutcome::Done
                        } else {
                            TaskOutcome::Failed("pool view wrong".into())
                        }
                    },
                    |_, _| {},
                    PollTimeout::Ms(8000),
                )
                .unwrap(),
        );
        if ids.len() == 2 {
            inflight.append(&mut ids);
        }
        while inflight.len() > 2 {
            let id = inflight.remove(0);
            assert_eq!(
                queue.wait(id, PollTimeout::Ms(8000)),
                Some(TaskOutcome::Done)
            );
        }
    }
    ids.append(&mut inflight);
    for id in ids {
        assert_eq!(
            queue.wait(id, PollTimeout::Ms(8000)),
            Some(TaskOutcome::Done)
        );
    }
    assert!(pool.stats().allocations <= 8, "pool honoured its cap");
}

#[test]
#[ignore = "requires /dev/dma_heap/system"]
fn h264_dpb_frame_leases_return_to_their_pools() {
    let nv12 = BufferPool::new(1);
    let colmv = BufferPool::new(1);
    let frame = Frame::acquire(&nv12, &colmv, 320, 240, true).unwrap();
    let nv12_fd = frame.nv12_fd();
    let colmv_fd = frame.colmv_fd();
    assert_eq!(nv12.stats().in_use, 1);
    assert_eq!(colmv.stats().in_use, 1);
    drop(frame);

    assert_eq!(nv12.stats().in_use, 0);
    assert_eq!(colmv.stats().in_use, 0);
    assert_eq!(nv12.stats().cached, 1);
    assert_eq!(colmv.stats().cached, 1);
    let reused = Frame::acquire(&nv12, &colmv, 320, 240, true).unwrap();
    assert_eq!(reused.nv12_fd(), nv12_fd);
    assert_eq!(reused.colmv_fd(), colmv_fd);
    assert_eq!(nv12.stats().allocations, 1);
    assert_eq!(colmv.stats().allocations, 1);
}

#[test]
#[ignore = "requires /dev/dma_heap/system"]
fn hevc_dpb_frame_leases_return_to_their_pools() {
    let video = BufferPool::new(1);
    let colmv = BufferPool::new(1);
    let frame = hevc_hal::Frame::acquire(
        &video,
        &colmv,
        320,
        240,
        64,
        8,
        hevc_hal::ChromaFormat::Yuv420,
    )
    .unwrap();
    let video_fd = frame.video_fd();
    let colmv_fd = frame.colmv_fd();
    assert_eq!(video.stats().in_use, 1);
    assert_eq!(colmv.stats().in_use, 1);
    drop(frame);

    assert_eq!(video.stats().in_use, 0);
    assert_eq!(colmv.stats().in_use, 0);
    assert_eq!(video.stats().cached, 1);
    assert_eq!(colmv.stats().cached, 1);
    let reused = hevc_hal::Frame::acquire(
        &video,
        &colmv,
        320,
        240,
        64,
        8,
        hevc_hal::ChromaFormat::Yuv420,
    )
    .unwrap();
    assert_eq!(reused.video_fd(), video_fd);
    assert_eq!(reused.colmv_fd(), colmv_fd);
    assert_eq!(video.stats().allocations, 1);
    assert_eq!(colmv.stats().allocations, 1);
}

#[test]
#[ignore = "requires /dev/dma_heap/system, system-dma32 and system-uncached-dma32"]
fn av1_dpb_frame_leases_return_to_their_pools() {
    let tile = Av1Frame::new_tile_pool(1);
    let display = Av1Frame::new_display_pool(1);
    let frame = Av1Frame::acquire(&tile, &display, 320, 240, 320, 240, 8, 0).unwrap();
    let tile_fd = frame.tile_output_fd();
    let display_fd = frame.display_fd();
    assert_eq!(tile.stats().in_use, 1);
    assert_eq!(display.stats().in_use, 1);
    drop(frame);

    assert_eq!(tile.stats().in_use, 0);
    assert_eq!(display.stats().in_use, 0);
    assert_eq!(tile.stats().cached, 1);
    assert_eq!(display.stats().cached, 1);
    let reused = Av1Frame::acquire(&tile, &display, 320, 240, 320, 240, 8, 0).unwrap();
    assert_eq!(reused.tile_output_fd(), tile_fd);
    assert_eq!(reused.display_fd(), display_fd);
    assert_eq!(tile.stats().allocations, 1);
    assert_eq!(display.stats().allocations, 1);
}

#[test]
#[ignore = "requires /dev/dma_heap/system"]
fn vp9_dpb_frame_leases_return_to_their_pools() {
    let video = Vp9Frame::new_video_pool(1);
    let colmv = Vp9Frame::new_colmv_pool(1);
    let frame = Vp9Frame::acquire(&video, &colmv, 320, 240, 8).unwrap();
    let video_fd = frame.video_fd();
    let colmv_fd = frame.colmv_fd();
    assert_eq!(video.stats().in_use, 1);
    assert_eq!(colmv.stats().in_use, 1);
    drop(frame);

    assert_eq!(video.stats().in_use, 0);
    assert_eq!(colmv.stats().in_use, 0);
    assert_eq!(video.stats().cached, 1);
    assert_eq!(colmv.stats().cached, 1);
    let reused = Vp9Frame::acquire(&video, &colmv, 320, 240, 8).unwrap();
    assert_eq!(reused.video_fd(), video_fd);
    assert_eq!(reused.colmv_fd(), colmv_fd);
    assert_eq!(video.stats().allocations, 1);
    assert_eq!(colmv.stats().allocations, 1);
}
