# M10 framework: task queue, buffer pool, dma-fence survey (W5 subset)

Status: the queue and the pool are implemented and unit-tested on the host;
board verification is scripted but not yet run (this worktree must not
touch t6). The fence entry below is a survey, not an implementation.
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
  taker retires the slot), and to a bounded best-effort event channel.

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
- `import` (external block → cache) and `dup_fd` (raw fd sharing, private
  copy) are the import/export corners of W5-4; the DPB-release-timing
  question is handled by the same refcount discipline — see the module
  doc-comment.
- Backend is generic (`PoolBuffer`): the host test suite runs on a counting
  fake (allocation counts are the reuse witness), the real `DmaBuf` backend
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
  proven on jpegd (decoders never had the rkvenc restriction; the async
  demo runs 6 jobs / 3 workers today). rkvenc keeps its one-live-session
  rule (HANDOFF §6.13, `setup_ext_line_bufs` writes process-shared row
  buffers). Until the line buffers become per-session, rkvenc jobs either
  get a 1-worker queue or share their session through
  `task_queue::SessionGate` — the gate test proves workers cannot interleave
  into the session section. When W5-5 lands it is a HAL-side change
  (per-session line buffers); queue and pool need nothing.
- **CCU (W5-6)**: driver-side multi-session batching is `MPP_CMD_SET_SESSION_FD`
  (`osal/driver/mpp_server.c` batches per-session task lists into one
  batch ioctl). This belongs to the service layer below the queue; the
  queue's only future obligation is that one job maps to one session,
  which holds by construction.
- **Secure (W5-7)**: `MPP_FLAGS_SECURE_MODE` (0x10000) is exposed as a
  constant; no code sets it. The test board has no TA, so per decision D3
  the implementation posture is "expose the bit, refuse secure work,
  record as unverified".

## 5. ROADMAP draft entries (English, for the ledger owner)

Draft — only the first has host-side evidence; tick nothing until the
board script has been run.

```
- [ ] W5 task queue (M10): async submit with worker threads, MppPollType
      wait domain, cancellation (queued removes, running flags),
      completion callback + waiter + bounded event trace. Scheduler
      state machine unit-tested (FIFO service, out-of-order completion,
      slot reuse, id monotonicity, capacity pinning). 18 unit tests.
      Board: scripts/board_framework_async_vs_sync.sh pending.
- [ ] W5 buffer pool (M10): refcounted dma-heap group with a
      Queued/Cached/Live{refs} state machine; double release and
      use-after-release reported instead of C's assert/UB; limit policy
      = mpp_buffer_group_limit_config (fail, no eviction); optional
      Block/Ms acquire on the MppPollType domain; import/dup_fd. 9 host
      tests (fake backend) + 2 board tests (DmaBuf) pending.
- [ ] W5 dma-fence (M10): survey only — the MPP tree and this board's
      service protocol expose no fence ioctl (see docs/framework-m10.md
      §3); poll_hw_finish_try ships as the host-visible poll surface,
      kernel errno contract unverified.
```

## 6. Commands for the orchestrator (board)

```bash
# 1. build + async==sync byte check + pool tallies (the new suite)
./scripts/board_framework_async_vs_sync.sh
# 2. the two ignored integration tests (jpegd + dma-heap):
ssh -n t6 'export PATH=$HOME/.cargo/bin:$PATH; cd ~/rk-mpp-rs && \
  cargo test --test framework_async_hw --test framework_pool_hw -- --ignored --test-threads=1'
# 3. regression gate - sync path must be unmoved:
ONLY=gop ./scripts/board_h264_qp_vs_mpp.sh
./scripts/board_h264_cbr_vs_mpp.sh
```
