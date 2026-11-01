use rk_mpp_rs::service::buffer_pool::BufferPool;
use rk_mpp_rs::service::task_queue::{PollTimeout, TaskOutcome, TaskQueue};

/// The pool tests in the library run against a fake allocator (a host has
/// no dma-heap); these two exercise the real DmaBuf backend.
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
#[ignore = "requires /dev/dma_heap/system"]
fn pool_feeds_queue_workers_under_concurrency() {
    let pool = BufferPool::new(8);
    let queue = TaskQueue::new(4);
    let mut ids = Vec::new();
    for _ in 0..32 {
        let pool = pool.clone();
        ids.push(
            queue
                .submit(
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
                )
                .unwrap(),
        );
    }
    for id in ids {
        assert_eq!(
            queue.wait(id, PollTimeout::Ms(8000)),
            Some(TaskOutcome::Done)
        );
    }
    assert!(pool.stats().allocations <= 8, "pool honoured its cap");
}
