# M10 framework: task queue, buffer pool, dma-fence survey (W5 subset)

Status: the queue, timing/metrics, pool, hardware recovery and cross-session
CCU submission are implemented and verified on host and t6. The fence entry
below remains a protocol survey, not an implementation.
ROADMAP draft entries live at the bottom; the ledger itself is updated by
whoever lands the board evidence.

## 1. Async task queue (`src/service/task_queue.rs`)

Layered *next to* the sync path — no existing call site changed, the sync
`MppService::submit_regs / submit_batch / poll_hw_finish` surface is
untouched and stays the reference the board script compares against.

Two halves:

- **`Scheduler`** — pure state machine, no threads. Slots transition
  `admit → Queued → next() → Running → finish() → Finished → take()`.
  Decisions that the threaded runtime inherits and that are unit-tested
  here directly: service order is submission order (`seq` FIFO); completion
  order is independent (retire task 3 first, slots 1/2 keep their outcomes);
  a `Finished` slot pins capacity until someone `take()`s it (the C output
  port behaves the same: nobody dequeues, nobody gets a new slot); ids are
  monotonic so a recycled slot can never alias an old waiter; `cancel`
  removes `Queued` (straight to `Finished(Cancelled)` so the waiter still
  gets an outcome) and only *flags* `Running`.
- **`TaskQueue`** — the runtime: N worker threads, `std::thread` +
  `Mutex` + `Condvar` only (no new dependencies). A job is `FnOnce() ->
  TaskOutcome + Send`, i.e. the closure runs the whole sync path (its own
  `MppService` session included) on the worker. Outcome is delivered
  exactly once to a callback, to any `wait(id, PollTimeout)` waiter (first
  taker retires the slot), and to a bounded best-effort event channel. Every
  event carries queue-wait, execution and turnaround timing. `TaskQueue::metrics`
  exposes cumulative submitted/done/failed/cancelled counts, aggregate timing
  and queued/running high-water marks (W5-9).

`PollTimeout` mirrors `MppPollType` exactly (`BLOCK` −1, `NON_BLOCK` 0,
1..=8000 ms, −2 and >8000 rejected) and is reused by the pool's timed
acquire. Cancellation semantics: a running job is never killed (abandoning
`POLL_HW_FINISH` mid-job leaks the session hardware); the body completes
and the outcome is reported `Cancelled`, which is also C's truth — its poll
timeout does not stop the hardware either.

Deviation from C worth stating: C's task queue is a port protocol (the
user shuttles `MppTask` handles between input/output ports). We expose a
submit/wait surface instead; the port dance carries no extra guarantees
once there is exactly one job per task, and the item-carrying tasks belong
to W7's C-API compatibility layer if it ever happens.

## 2. Buffer pool (`src/service/buffer_pool.rs`)

Refcounted dma-heap group mirroring `mpp/base/mpp_buffer.c`:

- `acquire` = `mpp_buffer_get`: reuses the smallest cached block ≥ size,
  else allocates through the backend, `Err(Full)` at the limit (C's limit
  → NULL). `acquire_timeout` adds Block/Ms waiting (our host-side queuing
  option; C has none).
- `try_clone`/`release` = `ref`/`unref`; last release caches the block
  instead of freeing. Enforced, not trusted: double release and
  use-after-release return `PoolError::Released` (C: debug-assert or UB);
  a partially released block stays invisible to new acquires (C trusts the
  caller); monotonic ids mean a recycled slot never aliases an old handle.
- `acquire_lease` / `BufferLease::try_clone` are the RAII forms of
  get/ref. The last lease to drop performs `unref`, including error paths,
  so a DPB owner or output consumer cannot silently leak its final reference.
  The explicit `BufferHandle` surface remains for C-style code and diagnostics.
  The async JPEG demo now uses leases for its table, stream and output blocks;
  every `?` path releases them automatically. VP8's DPB slots own leases
  directly. H.264's NV12/colmv, HEVC's video/colmv, AV1's
  tile-output/display and VP9's video/colmv pairs all use frame leases.
  H.264 drops every old frame lease on IDR or geometry change, clears only
  idle cache, then acquires a fresh surface, preserving the reference-less-IDR
  requirement. HEVC releases only surfaces outside the DPB; AV1 releases
  non-reference pictures before reuse; VP9 additionally protects the hardware
  previous/last colmv fds.
- Pooled JPEG jobs declare `MppService` after all buffer leases, so reverse
  drop order closes the kernel client before a shared dma-buf can return to
  the pool, including every early-error path. The t6 driver retains DMA import
  state until client close; releasing the leases first allowed another worker
  to reuse those fds while the old service still owned the import. A prior
  boot ended in a CPU6 soft lockup at `mpp_dma_find_buffer_fd()`. After this
  ordering fix, 500 consecutive async demos (3000 jobs, three workers) completed
  without a lockup or MPP reset under the 80% OPP cap.
- `import` (external block → cache) and `dup_fd` (raw fd sharing, private
  copy) are the import/export corners of W5-4; the DPB-release-timing
  question is handled by the same refcount discipline — see the module
  doc-comment.
- Backend is generic (`PoolBuffer`): the host test suite runs on a counting
  fake (allocation counts and lease-drop reuse are the witness), the real `DmaBuf` backend
  is exercised by the ignored `tests/framework_pool_hw.rs` on the board.
- `DmaBuf` gained `unsafe impl Send` (no thread affinity on a dma-buf
  borrow checker), deliberately not `Sync` — this is what lets pooled
  blocks travel to queue workers.

## 3. dma-fence survey (W5-3) — nothing to port, honestly

Searched the whole of `refs/mpp` (develop, the tree this project mirrors):

- `grep -rni fence` over every `.c/.h` returns exactly one file,
  `mpp/codec/dec/h264/h264d_global.h`, where "refence" is a typo for
  "reference". There is **no dma-fence support anywhere in this MPP
  generation**: no `MPP_CMD_*` fence command in `osal/inc/mpp_service.h`
  (the command table stops at `MPP_CMD_SET_ERR_REF_HACK` 0x404), no fence
  fields in `MppReqV1`, nothing in `mpp_task.*`, the task queue API
  headers, the buffer layer, or `kmpp`.
- `osal/inc/vcodec_service.h` (the newer batching service header the tree
  carries for the multi-instance server) also has no fence fields.
- The only completion signal this protocol offers userspace is (a) the
  blocking `POLL_HW_FINISH`, (b) `POLL_HW_IRQ` with its slice table and
  poll-type field, and (c) the `MPP_FLAGS_POLL_NON_BLOCK` (0x20) flag on
  (a). We expose (c) as `MppService::poll_hw_finish_try` mapping `EAGAIN`
  to `Ok(false)`, **unverified**: the kernel's behaviour for a
  non-blocking poll on an already-idle session (immediate `EAGAIN` vs
  `0` vs blocking anyway) is not documented in this tree, and a driver
  that ignores the flag would block forever — so the board script does not
  call it yet; probe it by hand first.

Decision for the PLAN checklist: W5-3 stays unticked. A dma-fence in/out
means the kernel accepting an in-fence fd per session and exporting an
out-fence per job; neither ioctl exists on this board's protocol, so this
cannot be implemented, only re-surveyed when the kernel tree moves. The
shape it would take here is visible: an in-fence = a job-closure pre-stage
that waits on the fence fd before its submit (host-side, `poll()` on the
dma-fence fd — no ioctl needed), and an out-fence would be handed from the
worker after `poll_hw_finish` — both land as wrappers around the queue, no
state machine change.

## 4. Multi-instance, CCU and secure — design position

- **Multi-instance (W5-5)**: the queue is instance-agnostic; each job owns
  its session fd, so `TaskQueue::new(n)` is already n concurrent sessions —
  proven on jpegd (the async demo runs 6 jobs / 3 workers today) and on
  rkvenc. The earlier one-session-per-process claim came from stale H.264
  goldens that also failed when run serially and is retracted. H.264's
  `ext_line_buf` and H.265's tile line buffers are members of their encoder
  sessions, not process globals. On t6, two and three live VEPU580 H.264
  sessions at 64x64, 320x240 and 640x480 produced one distinct stream each,
  equal to the serial result and the C reference. `task_queue::SessionGate`
  remains available when callers deliberately share one session, but it is
  no longer a correctness requirement.
- **CCU (W5-6)**: `src/service/batch.rs` is the Rust counterpart of
  `mpp_server.c`. It opens one server descriptor, inserts
  `MPP_CMD_SET_SESSION_FD` before each task, keeps internal boundaries free of
  `LAST_MSG`, and marks only the final task as the end of the ioctl chain.
  The 6.1 kernel's `mpp_collect_msgs()` triggers every external session, but
  does not populate each `mpp_bat_msg.ret`; completion therefore falls back to
  the real session's `POLL_HW_FINISH`. `CcuTopology` records the vdec/venc/JPEG
  device-tree nodes, and callers fall back to sequential jobs when neither the
  CCU nor `SET_SESSION_FD` is available. On t6, two independent VEPU2 JPEG
  sessions submitted together match their standalone streams byte for byte.
- **Secure (W5-7)**: `MPP_FLAGS_SECURE_MODE` (0x10000) is exposed as a
  constant for protocol completeness. Every batch submit rejects a request
  carrying that bit with `ErrorKind::Unsupported` before issuing an ioctl.
  The test board has no TA and the kernel exposes no capability query, so per
  decision D3 the secure data path remains unimplemented and unverified.

## 5. Hardware recovery (W5-8)

`POLL_HW_FINISH` timeouts and abnormal STATUS now enter a shared recovery
path. A direct `MPP_CMD_RESET_SESSION` is deliberately *not* considered
sufficient: t6 accepted 16 idle resets and reported the client alive, but the
next H.264 job submitted to the same fd wedged the driver and required a board
reboot. Opening a replacement fd before closing the reset fd was also unsafe:
the next decode blocked the entire driver. Normal recovery therefore never
issues `RESET_SESSION`; it closes the failed descriptor first, opens a fresh
`MppService`, health-checks `QUERY_HW_ID`, preserves counters and drops the
placeholder. DMA buffers stay allocated; the next job re-registers them. Every
service ioctl first retries `EINTR`; an interrupted syscall is not a hardware
failure and must not tear down a healthy session.

The normal hardware gate covers repeated close/reopen without reset and H.264
decode output after reconnection. The old 32-cycle open/reset/query/drop probe
is now an explicitly destructive example,
`examples/reset_session_probe.rs`; it is not part of `cargo test -- --ignored`
and must not be followed by a decode job without a board reboot first. This
prevents a diagnostic from poisoning a later hardware test in the same run.

The shared legacy VDPU2 path (VP8, MPEG-2, MPEG-4 and H.263) now treats
`IRQ_ERROR | IRQ_BUFFER` as a session-recovery boundary as well. It still
returns the original interrupt word so the damaged picture is marked, but the
next picture opens on a healthy session. The VP8 damage fixture decodes a clean
stream through the same decoder afterward and compares every visible plane;
that assertion is board-only and runs with the recovery suite.

## 6. ROADMAP draft entries (English, for the ledger owner)

Draft — mirrored here for context; the authoritative ledger is `ROADMAP.md`.

```
- [x] W5 task queue (M10): async submit with worker threads, MppPollType
      wait domain, cancellation (queued removes, running flags),
      completion callback + waiter + bounded event trace. Scheduler
      state machine unit-tested (FIFO service, out-of-order completion,
      slot reuse, id monotonicity, capacity pinning). 18 unit tests.
      Board: scripts/board_framework_async_vs_sync.sh passes on t6.
- [x] W5 buffer pool (M10): refcounted dma-heap group with a
      Queued/Cached/Live{refs} state machine; double release and
      use-after-release reported instead of C's assert/UB; limit policy
      = mpp_buffer_group_limit_config (fail, no eviction); optional
      Block/Ms acquire on the MppPollType domain; import/dup_fd; RAII
      BufferLease cloning/drop. 10 host tests (fake backend) + 3 board
      tests (DmaBuf fd→IOVA/dup and concurrent queue use) pass.
- [ ] W5 dma-fence (M10): survey only — the MPP tree and this board's
      service protocol expose no fence ioctl (see docs/framework-m10.md
      §3); poll_hw_finish_try ships as the host-visible poll surface,
      kernel errno contract unverified.
- [x] W5 CCU batching (M10): device-tree CCU detection, one
      `SET_SESSION_FD` submission for up to eight sessions, and
      per-session poll fallback on the 6.1 batch-result limitation.
      Two independent VEPU2 JPEG sessions match standalone output on t6.
```

## 7. Commands for the orchestrator (board)

```bash
# 1. build + async==sync byte check + pool tallies (the new suite)
./scripts/board_framework_async_vs_sync.sh
# 2. the four ignored integration tests (jpegd + three dma-heap/IOVA):
ssh -n t6 'export PATH=$HOME/.cargo/bin:$PATH; cd ~/rk-mpp-rs && \
  cargo test --test framework_async_hw --test framework_pool_hw -- --ignored --test-threads=1'
# 3. regression gate - sync path must be unmoved:
ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh
./scripts/board_h264_cbr_vs_mpp.sh
# 4. W5-8 recovery (replaces a failed session and then decodes a clean stream):
./scripts/board_framework_recovery.sh
# 5. W5-5 multi-instance encoder matrix:
ssh -n t6 'export PATH=$HOME/.cargo/bin:$PATH; cd ~/rk-mpp-rs && \
  cargo test --test h264_encoder_hw \
  concurrent_live_sessions_match_the_serial_reference \
  -- --ignored --nocapture --test-threads=1'
# 6. W5-6 cross-session CCU batch:
./scripts/board_framework_ccu.sh
```
