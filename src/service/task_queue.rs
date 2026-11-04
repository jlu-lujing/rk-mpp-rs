//! Async task queue layered over the synchronous `submit + poll` service.
//!
//! The C framework (refs/mpp `mpp/base/mpp_task.c`, `mpp/mpp.c`) separates
//! the user-facing ports from the hardware worker: the user dequeues a task
//! on a port, fills it, enqueues it, and the queue either completes it inline
//! (`MPP_TASK_SYNC`) or hands it to the worker thread (`MPP_TASK_ASYNC`, the
//! default). Completion is then picked up on the output port. This module is
//! the Rust counterpart of that separation for whole hardware jobs: a job is
//! a closure that performs the existing sync path (submit batch, poll finish,
//! read STATUS back) on the calling worker's own session, and the queue
//! provides ordering, capacity, cancellation, timeout and completion
//! callbacks around it.
//!
//! # Relationship to the C semantics
//!
//! | C concept | our counterpart | notes |
//! |---|---|---|
//! | `MppTaskQueue` input/output ports | `TaskQueue` + per-task [`TaskQueue::wait`] | we do not expose the OpenMAX port dance; one job per task instead of item-carrying slots |
//! | `MppPollType` (`BLOCK` −1, `NON_BLOCK` 0, 1..=8000 ms) | [`PollTimeout`] | same domain, same rejection of −2 and >8000 |
//! | async enqueue returns before completion | [`TaskQueue::submit`] | non-blocking submit; [`TaskQueue::submit_blocking`] for port-style waiting when full |
//! | completion surfaced on output port | callback + [`TaskQueue::wait`] + best-effort event channel | one outcome per task, retired by the first waiter |
//! | flush discarding queued work | [`TaskQueue::cancel`] on not-yet-started tasks | a running ioctl cannot be interrupted, only flagged |
//!
//! Deliberate deviations, and why:
//!
//! * A cancelled *running* task is not killed — `POLL_HW_FINISH` on a session
//!   fd cannot be safely abandoned mid-job without leaking the hardware.
//!   The job body runs to completion and the outcome is reported as
//!   `Cancelled`, matching how C's poll timeout also leaves the hardware
//!   job to finish.
//! * Queue capacity bounds *live* tasks (queued + running + finished-not-yet
//!   retired). C's task queue pre-allocates a fixed slot table with the same
//!   property: nobody dequeues, nothing new gets a slot.
//!
//! # Multi-session and multi-instance (PLAN W5-5/W5-6)
//!
//! Every `/dev/mpp_service` open is one kernel session, and one session
//! serialises its own jobs, which is what makes a worker pool per hardware
//! block the natural shape: each *job closure* owns the session it opens, so
//! `TaskQueue::new(n)` presents `n` independent sessions to the kernel. That
//! is valid for rkvenc as well: two and three live VEPU580 H.264 sessions in
//! one process were measured at 64x64, 320x240 and 640x480 and produce one
//! distinct stream each, equal to the serial result and to the C reference
//! (`tests/h264_encoder_hw.rs`). The earlier claim that rkvenc allows one live
//! session per process came from stale goldens that failed without any
//! concurrency at all; it is retracted. [`SessionGate`] stays available for
//! callers who *choose* to share one session (a queue with one worker per
//! block is also a reasonable sizing choice), but it is no longer a
//! correctness requirement.
//!
//! Multi-core CCU distribution (W5-6) lives below this layer: the
//! driver-side batching C performs with `MPP_CMD_SET_SESSION_FD` inside one
//! batch ioctl (`osal/driver/mpp_server.c`) is a service-protocol feature,
//! not a queue feature. Our queue hands one job to one worker; the job may
//! itself use `MppService::submit_batch`, and a per-session preamble is the
//! hook point for spreading several sessions over cores later.
//!
//! dma-fence (W5-3): the MPP tree this project mirrors (refs/mpp develop)
//! exposes no fence ioctl at all — see `docs/framework-m10.md` for the
//! survey. No interface is faked here; a host-side wait-before-run is simply
//! a job closure that waits before submitting.

use std::collections::HashMap;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Monotonic task identifier; ids are never reused.
pub type TaskId = u64;

/// `MPP_POLL_MAX` from `inc/mpp_task.h`.
pub const MPP_POLL_MAX: u32 = 8000;

/// Poll wait domain, mirroring `MppPollType` (`inc/mpp_task.h`):
/// `BLOCK` = −1, `NON_BLOCK` = 0, 1..=8000 ms, everything else invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollTimeout {
    Block,
    NonBlock,
    Ms(u32),
}

impl PollTimeout {
    /// Decode a C `MppPollType`/`MppTimeout` integer. C rejects values
    /// `<= MPP_POLL_BUTT` (−2) or `> MPP_POLL_MAX` (8000).
    pub fn from_mpp_poll_type(value: i32) -> Option<PollTimeout> {
        if value <= -2 || value > MPP_POLL_MAX as i32 {
            return None;
        }
        Some(match value {
            -1 => PollTimeout::Block,
            0 => PollTimeout::NonBlock,
            ms => PollTimeout::Ms(ms as u32),
        })
    }

    /// The same wait expressed as `std` terms: `None` means unbounded.
    pub fn duration(self) -> Option<Duration> {
        match self {
            PollTimeout::Block => None,
            PollTimeout::NonBlock => Some(Duration::ZERO),
            PollTimeout::Ms(ms) => Some(Duration::from_millis(ms as u64)),
        }
    }
}

/// Terminal result of one task, delivered exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    Done,
    Failed(String),
    Cancelled,
}

impl TaskOutcome {
    /// Wrap an io error as a failed outcome (job bodies return this).
    pub fn io_error(e: &std::io::Error) -> TaskOutcome {
        TaskOutcome::Failed(e.to_string())
    }
}

/// Result of [`TaskQueue::cancel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancel {
    /// The task was still queued and will never run.
    Removed,
    /// The job body is running; it cannot be interrupted, its outcome will
    /// be reported as [`TaskOutcome::Cancelled`] instead of what it returns.
    Flagged,
    /// No live task with this id.
    Missing,
}

/// Why a submit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    /// No live slot below the configured capacity (non-blocking submit, or
    /// the blocking variant past its timeout).
    Full,
    /// The queue is shutting down and no longer accepts work.
    ShutDown,
}

/// Best-effort trace event; a dropped event is not lost information because
/// callbacks and [`TaskQueue::wait`] are the authoritative delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEvent {
    pub id: TaskId,
    pub outcome: TaskOutcome,
    pub queued_depth: usize,
    pub running: usize,
}

// ---------------------------------------------------------------------------
// Pure state machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Queued,
    Running,
    Finished,
}

#[derive(Debug)]
struct Slot {
    id: TaskId,
    seq: u64,
    state: SlotState,
    cancel_requested: bool,
    outcome: Option<TaskOutcome>,
}

/// Thread-free task bookkeeping. All ordering and slot-reuse rules the
/// threaded runtime must obey are decided here so they can be unit tested
/// without hardware or timing.
///
/// Rules (all tested):
/// * execution order is submission order (`seq` FIFO among `Queued`);
/// * completion order is independent of submission order;
/// * a `Finished` slot is only reclaimed by [`Scheduler::take`], so a late
///   waiter always finds its outcome; reclaiming frees the slot for a new
///   id — ids themselves are never reused;
/// * cancel moves `Queued` tasks straight to `Finished(Cancelled)` (their
///   waiter still gets an outcome) and only flags `Running` ones;
/// * capacity counts every live slot (queued + running + finished).
#[derive(Debug)]
pub struct Scheduler {
    capacity: usize,
    slots: Vec<Option<Slot>>,
    next_id: TaskId,
    next_seq: u64,
}

impl Scheduler {
    pub fn new(capacity: usize) -> Scheduler {
        Scheduler {
            capacity,
            slots: (0..capacity).map(|_| None).collect(),
            next_id: 1,
            next_seq: 0,
        }
    }

    /// Take the first free slot for a new task, or `None` when full.
    pub fn admit(&mut self) -> Option<TaskId> {
        let slot = self.slots.iter().position(|s| s.is_none())?;
        let id = self.next_id;
        self.next_id += 1;
        let seq = self.next_seq;
        self.next_seq += 1;
        self.slots[slot] = Some(Slot {
            id,
            seq,
            state: SlotState::Queued,
            cancel_requested: false,
            outcome: None,
        });
        Some(id)
    }

    /// Oldest `Queued` task, moving it to `Running`.
    pub fn next(&mut self) -> Option<TaskId> {
        let slot = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.as_ref().is_some_and(|sl| sl.state == SlotState::Queued))
            .min_by_key(|(_, s)| s.as_ref().map(|sl| sl.seq).unwrap_or(u64::MAX))?
            .0;
        let slot = self.slots[slot].as_mut()?;
        slot.state = SlotState::Running;
        Some(slot.id)
    }

    pub fn cancel(&mut self, id: TaskId) -> Cancel {
        let Some(idx) = self.slot_of(id) else {
            return Cancel::Missing;
        };
        let slot = self.slots[idx].as_mut().unwrap();
        match slot.state {
            SlotState::Queued => {
                slot.state = SlotState::Finished;
                slot.outcome = Some(TaskOutcome::Cancelled);
                Cancel::Removed
            }
            SlotState::Running => {
                slot.cancel_requested = true;
                Cancel::Flagged
            }
            _ => Cancel::Missing,
        }
    }

    /// Move a `Running` task to `Finished`. A cancel flag set while the job
    /// body was running overrides the produced outcome to `Cancelled`, so
    /// both delivery paths see the same single outcome.
    pub fn finish(&mut self, id: TaskId, outcome: TaskOutcome) {
        let Some(idx) = self.slot_of(id) else {
            return;
        };
        let slot = self.slots[idx].as_mut().unwrap();
        if slot.state != SlotState::Running {
            return;
        }
        slot.state = SlotState::Finished;
        slot.outcome = Some(if slot.cancel_requested {
            TaskOutcome::Cancelled
        } else {
            outcome
        });
    }

    /// Reclaim a finished task's slot, returning its outcome.
    pub fn take(&mut self, id: TaskId) -> Option<TaskOutcome> {
        let idx = self.slot_of(id)?;
        if self.slots[idx].as_ref()?.state != SlotState::Finished {
            return None;
        }
        let outcome = self.slots[idx].as_mut()?.outcome.take();
        self.slots[idx] = None;
        outcome
    }

    /// Peek a finished outcome without reclaiming it.
    pub fn peek(&self, id: TaskId) -> Option<&TaskOutcome> {
        let idx = self.slot_of(id)?;
        let slot = self.slots[idx].as_ref()?;
        if slot.state != SlotState::Finished {
            return None;
        }
        slot.outcome.as_ref()
    }

    /// Is the id known at all (any live state)?
    pub fn is_live(&self, id: TaskId) -> bool {
        self.slot_of(id).is_some()
    }

    fn slot_of(&self, id: TaskId) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|sl| sl.id == id))
    }

    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub fn queued_depth(&self) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.state == SlotState::Queued)
            .count()
    }

    pub fn running_count(&self) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.state == SlotState::Running)
            .count()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Move every `Finished` slot out, returning the ids it reclaimed.
    /// Used only by queue shutdown so unwaited outcomes do not pin capacity
    /// forever.
    fn drain_finished(&mut self) -> Vec<TaskId> {
        let ids: Vec<TaskId> = self
            .slots
            .iter()
            .flatten()
            .filter(|s| s.state == SlotState::Finished)
            .map(|s| s.id)
            .collect();
        for id in &ids {
            let idx = self.slot_of(*id).unwrap();
            self.slots[idx] = None;
        }
        ids
    }
}

// ---------------------------------------------------------------------------
// Threaded runtime
// ---------------------------------------------------------------------------

type Job = Box<dyn FnOnce() -> TaskOutcome + Send>;
type Callback = Box<dyn FnOnce(TaskId, TaskOutcome) + Send>;

struct Entry {
    /// Taken by the worker before the body runs; the entry itself lives on
    /// so the callback survives until the outcome is delivered.
    job: Option<Job>,
    callback: Option<Callback>,
}

struct Runtime {
    scheduler: Scheduler,
    pending: HashMap<TaskId, Entry>,
    shut_down: bool,
}

struct Shared {
    core: Mutex<Runtime>,
    /// Worker wakeups: work available or shutdown.
    work_cv: Condvar,
    /// `submit_blocking` waiters: capacity freed.
    space_cv: Condvar,
    /// Outcome waiters: any slot reached `Finished`.
    done_cv: Condvar,
    events: Mutex<Option<SyncSender<TaskEvent>>>,
}

/// A pool of worker threads executing jobs in submission order; each job
/// runs the existing synchronous submit+poll path inside its closure.
pub struct TaskQueue {
    shared: Arc<Shared>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl TaskQueue {
    /// Spawn `workers` threads; at most `4 * workers` live tasks.
    pub fn new(workers: usize) -> TaskQueue {
        TaskQueue::with_capacity(workers, 4 * workers.max(1))
    }

    pub fn with_capacity(workers: usize, capacity: usize) -> TaskQueue {
        Self::build(workers, capacity, None)
    }

    /// Spawn workers and attach a bounded event channel for tracing (W5-9).
    /// Events are best effort: when the channel is full the event is
    /// dropped; the callback/wait paths stay authoritative.
    pub fn with_events(workers: usize) -> (TaskQueue, Receiver<TaskEvent>) {
        TaskQueue::with_capacity_and_events(workers, 4 * workers.max(1))
    }

    pub fn with_capacity_and_events(
        workers: usize,
        capacity: usize,
    ) -> (TaskQueue, Receiver<TaskEvent>) {
        let (tx, rx) = sync_channel::<TaskEvent>(capacity);
        (Self::build(workers, capacity, Some(tx)), rx)
    }

    fn build(workers: usize, capacity: usize, events: Option<SyncSender<TaskEvent>>) -> TaskQueue {
        assert!(capacity > 0 && workers > 0);
        let shared = Arc::new(Shared {
            core: Mutex::new(Runtime {
                scheduler: Scheduler::new(capacity),
                pending: HashMap::new(),
                shut_down: false,
            }),
            work_cv: Condvar::new(),
            space_cv: Condvar::new(),
            done_cv: Condvar::new(),
            events: Mutex::new(events),
        });
        let mut handles = Vec::new();
        for _ in 0..workers {
            let shared = Arc::clone(&shared);
            handles.push(std::thread::spawn(move || worker_loop(shared)));
        }
        TaskQueue {
            shared,
            workers: Mutex::new(handles),
        }
    }

    /// Queue a job without waiting for capacity. `callback` runs on a worker
    /// thread with the task's outcome; [`TaskQueue::wait`] also delivers the
    /// outcome to any waiter (the slot lives until the first of the two
    /// retakes it — a callback does *not* retire it, so the waiter after a
    /// callback still gets the same outcome).
    pub fn submit(
        &self,
        job: impl FnOnce() -> TaskOutcome + Send + 'static,
        callback: impl FnOnce(TaskId, TaskOutcome) + Send + 'static,
    ) -> Result<TaskId, SubmitError> {
        let mut core = self.shared.core.lock().unwrap();
        if core.shut_down {
            return Err(SubmitError::ShutDown);
        }
        let id = core.scheduler.admit().ok_or(SubmitError::Full)?;
        core.pending.insert(
            id,
            Entry {
                job: Some(Box::new(job)),
                callback: Some(Box::new(callback)),
            },
        );
        self.shared.work_cv.notify_one();
        Ok(id)
    }

    /// Like [`submit`](Self::submit) but waits for a free slot, mirroring
    /// the C input-port behaviour of blocking in `poll` when the queue is
    /// full.
    pub fn submit_blocking(
        &self,
        job: impl FnOnce() -> TaskOutcome + Send + 'static,
        callback: impl FnOnce(TaskId, TaskOutcome) + Send + 'static,
        timeout: PollTimeout,
    ) -> Result<TaskId, SubmitError> {
        let budget = timeout.duration();
        let start = Instant::now();
        let mut core = self.shared.core.lock().unwrap();
        loop {
            if core.shut_down {
                return Err(SubmitError::ShutDown);
            }
            if let Some(id) = core.scheduler.admit() {
                core.pending.insert(
                    id,
                    Entry {
                        job: Some(Box::new(job)),
                        callback: Some(Box::new(callback)),
                    },
                );
                self.shared.work_cv.notify_one();
                return Ok(id);
            }
            match budget {
                None => core = self.shared.space_cv.wait(core).unwrap(),
                Some(b) => {
                    let Some(remaining) = b.checked_sub(start.elapsed()) else {
                        return Err(SubmitError::Full);
                    };
                    let (c, wait) = self.shared.space_cv.wait_timeout(core, remaining).unwrap();
                    core = c;
                    if wait.timed_out() {
                        return Err(SubmitError::Full);
                    }
                }
            }
        }
    }

    /// Try to stop a task. See [`Cancel`].
    pub fn cancel(&self, id: TaskId) -> Cancel {
        let (result, delivered) = {
            let mut core = self.shared.core.lock().unwrap();
            let result = core.scheduler.cancel(id);
            let delivered = if result == Cancel::Removed {
                let entry = core.pending.remove(&id);
                let outcome = core.scheduler.peek(id).cloned();
                (entry.and_then(|e| e.callback), outcome)
            } else {
                (None, None)
            };
            self.shared.done_cv.notify_all();
            self.shared.space_cv.notify_one();
            (result, delivered)
        };
        if result == Cancel::Removed {
            if let (Some(cb), Some(outcome)) = delivered {
                cb(id, outcome.clone());
                let (depth, running) = self.depths();
                publish_event(&self.shared, id, outcome, depth, running);
            }
        }
        result
    }

    /// Wait for a task's outcome, retiring it. `None` means timed out or the
    /// outcome was already retired by another waiter (the task keeps its
    /// slot until a later wait succeeds or the queue shuts down, exactly
    /// like an unread output port).
    pub fn wait(&self, id: TaskId, timeout: PollTimeout) -> Option<TaskOutcome> {
        let budget = timeout.duration();
        let start = Instant::now();
        let mut core = self.shared.core.lock().unwrap();
        loop {
            if let Some(outcome) = core.scheduler.take(id) {
                self.shared.space_cv.notify_one();
                return Some(outcome);
            }
            if !core.scheduler.is_live(id) {
                return None;
            }
            match budget {
                None => core = self.shared.done_cv.wait(core).unwrap(),
                Some(b) => {
                    let Some(remaining) = b.checked_sub(start.elapsed()) else {
                        return None;
                    };
                    let (c, wait) = self.shared.done_cv.wait_timeout(core, remaining).unwrap();
                    core = c;
                    if wait.timed_out() && core.scheduler.peek(id).is_none() {
                        return None;
                    }
                }
            }
        }
    }

    /// Non-destructive check, for stats and tests.
    pub fn outcome_of(&self, id: TaskId) -> Option<TaskOutcome> {
        self.shared.core.lock().unwrap().scheduler.peek(id).cloned()
    }

    /// (queued, running, live, capacity)
    pub fn stats(&self) -> (usize, usize, usize, usize) {
        let core = self.shared.core.lock().unwrap();
        (
            core.scheduler.queued_depth(),
            core.scheduler.running_count(),
            core.scheduler.live_count(),
            core.scheduler.capacity(),
        )
    }

    fn depths(&self) -> (usize, usize) {
        let core = self.shared.core.lock().unwrap();
        (
            core.scheduler.queued_depth(),
            core.scheduler.running_count(),
        )
    }

    /// Stop accepting work and join all workers. Queued jobs still run
    /// (draining shutdown: submitted hardware jobs should not be dropped
    /// mid-clip); anything racing in afterwards is refused with
    /// [`SubmitError::ShutDown`]. Outcomes of drained jobs are delivered to
    /// their callbacks; slots nobody waited on are reclaimed here.
    pub fn shutdown(&self) {
        {
            let mut core = self.shared.core.lock().unwrap();
            core.shut_down = true;
        }
        self.shared.work_cv.notify_all();
        self.shared.space_cv.notify_all();
        let mut handles = self.workers.lock().unwrap();
        for handle in handles.drain(..) {
            let _ = handle.join();
        }
        let mut core = self.shared.core.lock().unwrap();
        for id in core.scheduler.drain_finished() {
            core.pending.remove(&id);
        }
    }
}

impl Drop for TaskQueue {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop(shared: Arc<Shared>) {
    loop {
        let id = {
            let mut core = shared.core.lock().unwrap();
            loop {
                if let Some(id) = core.scheduler.next() {
                    break Some(id);
                }
                if core.shut_down {
                    break None;
                }
                let (c, _wait) = shared
                    .work_cv
                    .wait_timeout(core, Duration::from_millis(50))
                    .unwrap();
                core = c;
            }
        };
        let Some(id) = id else { break };
        // Between `next()` (Queued->Running) and grabbing the payload a
        // cancel cannot strike (cancel only removes Queued slots), so the
        // payload is always present here.
        let job = shared
            .core
            .lock()
            .unwrap()
            .pending
            .get_mut(&id)
            .and_then(|entry| entry.job.take());
        let outcome = match job {
            Some(job) => job(),
            None => continue,
        };
        let (callback, delivered) = {
            let mut core = shared.core.lock().unwrap();
            core.scheduler.finish(id, outcome);
            let outcome = core.scheduler.peek(id).cloned();
            (core.pending.remove(&id).and_then(|e| e.callback), outcome)
        };
        if let (Some(cb), Some(outcome)) = (callback, delivered.clone()) {
            cb(id, outcome);
        }
        let (depth, running) = {
            let core = shared.core.lock().unwrap();
            (
                core.scheduler.queued_depth(),
                core.scheduler.running_count(),
            )
        };
        shared.done_cv.notify_all();
        shared.space_cv.notify_one();
        if let Some(outcome) = delivered {
            publish_event(&shared, id, outcome, depth, running);
        }
    }
}

fn publish_event(shared: &Shared, id: TaskId, outcome: TaskOutcome, depth: usize, running: usize) {
    let mut events = shared.events.lock().unwrap();
    if let Some(tx) = events.as_mut() {
        let event = TaskEvent {
            id,
            outcome,
            queued_depth: depth,
            running,
        };
        // Best-effort trace, documented on TaskEvent.
        let _ = tx.try_send(event);
    }
}

/// Serialise a *single* hardware session across several queue workers. Give
/// every job closure the same gate when the jobs must share one kernel
/// session; the queue may still run `n` workers, only the session section is
/// serialised. Independent sessions per worker do not need a gate.
pub struct SessionGate<T> {
    inner: Mutex<T>,
}

impl<T> SessionGate<T> {
    pub fn new(value: T) -> SessionGate<T> {
        SessionGate {
            inner: Mutex::new(value),
        }
    }

    pub fn with<R: Send>(&self, run: impl FnOnce(&mut T) -> R + Send) -> R {
        let mut guard = self.inner.lock().unwrap();
        run(&mut guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn done() -> TaskOutcome {
        TaskOutcome::Done
    }

    fn noop_job() -> impl FnOnce() -> TaskOutcome + Send {
        || done()
    }

    fn count_done(counter: Arc<AtomicUsize>) -> impl FnOnce(TaskId, TaskOutcome) + Send {
        move |_id, outcome| {
            if outcome == TaskOutcome::Done {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    // ----- pure scheduler ------------------------------------------------

    #[test]
    fn execution_order_is_submission_order() {
        let mut s = Scheduler::new(8);
        let ids: Vec<TaskId> = (0..5).map(|_| s.admit().unwrap()).collect();
        let served: Vec<TaskId> = (0..5).map(|_| s.next().unwrap()).collect();
        assert_eq!(ids, served);
    }

    #[test]
    fn ids_are_never_reused_even_after_slots_recycle() {
        let mut s = Scheduler::new(2);
        let a = s.admit().unwrap();
        let b = s.admit().unwrap();
        assert_eq!(s.admit(), None, "capacity full");
        assert_eq!(s.next(), Some(a));
        s.finish(a, done());
        assert_eq!(s.take(a), Some(done()));
        let c = s.admit().unwrap();
        assert!(c > b, "new id {c} must exceed retired id {b}");
    }

    #[test]
    fn out_of_order_completion_retires_independently() {
        // completion order 3,1,2 against submission order 1,2,3: each waiter
        // takes only its own outcome, and the third slot is reusable while
        // the first two still hold theirs.
        let mut s = Scheduler::new(3);
        let a = s.admit().unwrap();
        let b = s.admit().unwrap();
        let c = s.admit().unwrap();
        for id in [a, b, c] {
            assert_eq!(s.next(), Some(id));
        }
        s.finish(c, done());
        s.finish(a, TaskOutcome::Failed("a".into()));
        assert_eq!(s.take(c), Some(done()));
        assert_eq!(s.live_count(), 2);
        // a freed slot admits a new task although a is still unretired
        let d = s.admit().unwrap();
        assert!(d > c);
        assert_eq!(s.admit(), None, "back to full");
        s.finish(b, done());
        assert_eq!(s.take(a), Some(TaskOutcome::Failed("a".into())));
        assert_eq!(s.take(b), Some(done()));
        assert_eq!(s.live_count(), 1, "d keeps its queued slot");
    }

    #[test]
    fn cancel_queued_never_runs_and_yields_cancelled_outcome() {
        let mut s = Scheduler::new(4);
        let a = s.admit().unwrap();
        let b = s.admit().unwrap();
        assert_eq!(s.cancel(b), Cancel::Removed);
        assert_eq!(s.next(), Some(a), "cancelled task must not be served");
        assert_eq!(s.next(), None);
        assert_eq!(s.peek(b), Some(&TaskOutcome::Cancelled));
        assert_eq!(s.take(b), Some(TaskOutcome::Cancelled));
        assert_eq!(s.cancel(b), Cancel::Missing);
    }

    #[test]
    fn cancel_running_overrides_outcome_once() {
        let mut s = Scheduler::new(4);
        let a = s.admit().unwrap();
        assert_eq!(s.next(), Some(a));
        assert_eq!(s.cancel(a), Cancel::Flagged);
        s.finish(a, done());
        assert_eq!(
            s.take(a),
            Some(TaskOutcome::Cancelled),
            "a flagged cancel must yield exactly one Cancelled outcome even when the body succeeded"
        );
    }

    #[test]
    fn finished_slots_pin_capacity_until_taken() {
        let mut s = Scheduler::new(2);
        let a = s.admit().unwrap();
        let b = s.admit().unwrap();
        assert_eq!(s.next(), Some(a));
        s.finish(a, done());
        assert_eq!(s.admit(), None, "an unretired Finished slot pins capacity");
        s.take(a);
        assert!(s.admit().is_some());
        assert!(s.is_live(b));
    }

    #[test]
    fn drain_finished_only_reclaims_finished_slots() {
        let mut s = Scheduler::new(4);
        let a = s.admit().unwrap();
        let b = s.admit().unwrap();
        assert_eq!(s.next(), Some(a));
        s.finish(a, done());
        let drained = s.drain_finished();
        assert_eq!(drained, vec![a]);
        assert!(s.is_live(b), "a queued task is never drained away");
    }

    #[test]
    fn poll_timeout_domain_matches_mpp_poll_type() {
        assert_eq!(
            PollTimeout::from_mpp_poll_type(-1),
            Some(PollTimeout::Block)
        );
        assert_eq!(
            PollTimeout::from_mpp_poll_type(0),
            Some(PollTimeout::NonBlock)
        );
        assert_eq!(PollTimeout::from_mpp_poll_type(1), Some(PollTimeout::Ms(1)));
        assert_eq!(
            PollTimeout::from_mpp_poll_type(8000),
            Some(PollTimeout::Ms(8000))
        );
        assert_eq!(PollTimeout::from_mpp_poll_type(-2), None, "MPP_POLL_BUTT");
        assert_eq!(PollTimeout::from_mpp_poll_type(8001), None);
        assert_eq!(PollTimeout::Block.duration(), None);
        assert_eq!(
            PollTimeout::NonBlock.duration(),
            Some(Duration::from_secs(0))
        );
    }

    // ----- threaded runtime ----------------------------------------------

    #[test]
    fn fifty_jobs_all_complete_by_both_paths() {
        // Capacity 64: finished outcomes pin live slots until `wait` retires
        // them (a deliberate C-output-port property), so a submit-all-then
        // -wait-all pattern needs the slots.
        let queue = TaskQueue::with_capacity(4, 64);
        let ok = Arc::new(AtomicUsize::new(0));
        let mut ids = Vec::new();
        for _ in 0..50 {
            ids.push(
                queue
                    .submit(noop_job(), count_done(Arc::clone(&ok)))
                    .unwrap(),
            );
        }
        for id in ids {
            assert_eq!(queue.wait(id, PollTimeout::Block), Some(done()));
        }
        queue.shutdown();
        assert_eq!(ok.load(Ordering::SeqCst), 50);
        assert_eq!(queue.stats(), (0, 0, 0, 64));
    }

    #[test]
    fn completions_may_arrive_out_of_submission_order() {
        // Long first job, short second: the short task finishes while the
        // long one is still running. Only start order is enforced.
        let queue = TaskQueue::new(2);
        let order = Arc::new(Mutex::new(Vec::new()));
        let o1 = Arc::clone(&order);
        let long = queue
            .submit(
                || {
                    std::thread::sleep(Duration::from_millis(150));
                    done()
                },
                move |_id, _o| o1.lock().unwrap().push(1),
            )
            .unwrap();
        let o2 = Arc::clone(&order);
        let short = queue
            .submit(noop_job(), move |_id, _o| o2.lock().unwrap().push(2))
            .unwrap();
        assert_eq!(queue.wait(short, PollTimeout::Block), Some(done()));
        assert_eq!(
            order.lock().unwrap().as_slice(),
            &[2],
            "short finished first"
        );
        assert_eq!(queue.wait(long, PollTimeout::Block), Some(done()));
        assert_eq!(order.lock().unwrap().as_slice(), &[2, 1]);
    }

    #[test]
    fn wait_timeout_lets_the_task_finish_later() {
        let queue = TaskQueue::new(1);
        let id = queue
            .submit(
                || {
                    std::thread::sleep(Duration::from_millis(120));
                    done()
                },
                |_, _| {},
            )
            .unwrap();
        assert_eq!(queue.wait(id, PollTimeout::Ms(5)), None, "must time out");
        assert_eq!(queue.wait(id, PollTimeout::Block), Some(done()));
    }

    #[test]
    fn queued_task_cancelled_before_it_runs() {
        let queue = TaskQueue::new(1);
        let blocker = queue
            .submit(
                || {
                    std::thread::sleep(Duration::from_millis(150));
                    done()
                },
                |_, _| {},
            )
            .unwrap();
        // Single worker busy: the second task cannot have started.
        let doomed = queue
            .submit(|| panic!("a cancelled job must never execute"), |_, _| {})
            .unwrap();
        assert_eq!(queue.cancel(doomed), Cancel::Removed);
        assert_eq!(
            queue.wait(doomed, PollTimeout::Block),
            Some(TaskOutcome::Cancelled)
        );
        assert_eq!(queue.wait(blocker, PollTimeout::Block), Some(done()));
    }

    #[test]
    fn running_task_cancel_reports_cancelled_after_body_finishes() {
        let queue = TaskQueue::new(1);
        let id = queue
            .submit(
                || {
                    std::thread::sleep(Duration::from_millis(100));
                    done()
                },
                |_, _| {},
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(queue.cancel(id), Cancel::Flagged);
        assert_eq!(
            queue.wait(id, PollTimeout::Block),
            Some(TaskOutcome::Cancelled)
        );
    }

    #[test]
    fn submit_reports_full_and_blocking_waits_for_room() {
        let queue = TaskQueue::with_capacity(1, 2);
        let blocker = queue
            .submit(
                || {
                    std::thread::sleep(Duration::from_millis(150));
                    done()
                },
                |_, _| {},
            )
            .unwrap();
        let _second = queue.submit(noop_job(), |_, _| {}).unwrap();
        // 2 live slots used (running + queued), capacity 2.
        assert_eq!(queue.submit(noop_job(), |_, _| {}), Err(SubmitError::Full));
        // A blocking submit with a short budget gives up while full...
        assert_eq!(
            queue.submit_blocking(noop_job(), |_, _| {}, PollTimeout::Ms(10)),
            Err(SubmitError::Full)
        );
        // ...and lands once the blocker retires its slot.
        assert_eq!(queue.wait(blocker, PollTimeout::Block), Some(done()));
        let third = queue
            .submit_blocking(noop_job(), |_, _| {}, PollTimeout::Block)
            .expect("room freed by the retired blocker");
        assert_eq!(queue.wait(third, PollTimeout::Block), Some(done()));
    }

    #[test]
    fn shutdown_drains_submitted_work_and_refuses_new() {
        let queue = TaskQueue::with_capacity(2, 16);
        let done_count = Arc::new(AtomicUsize::new(0));
        let mut ids = Vec::new();
        for _ in 0..10 {
            ids.push(
                queue
                    .submit(noop_job(), count_done(Arc::clone(&done_count)))
                    .unwrap(),
            );
        }
        queue.shutdown();
        assert_eq!(
            done_count.load(Ordering::SeqCst),
            10,
            "shutdown must drain, not drop"
        );
        assert_eq!(
            queue.submit(noop_job(), |_, _| {}),
            Err(SubmitError::ShutDown)
        );
    }

    #[test]
    fn session_gate_serialises_a_shared_handle() {
        // Many workers sharing one live session on purpose.
        // Capacity covers all 24 pinned outcomes until waited (by design).
        let queue = TaskQueue::with_capacity(4, 32);
        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(SessionGate::new(0u32));
        let mut ids = Vec::new();
        for _ in 0..24 {
            let (gate, concurrent, max_seen) = (
                Arc::clone(&gate),
                Arc::clone(&concurrent),
                Arc::clone(&max_seen),
            );
            ids.push(
                queue
                    .submit(
                        move || {
                            gate.with(|_session| {
                                let n = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                                max_seen.fetch_max(n, Ordering::SeqCst);
                                std::thread::sleep(Duration::from_millis(2));
                                concurrent.fetch_sub(1, Ordering::SeqCst);
                            });
                            done()
                        },
                        |_, _| {},
                    )
                    .unwrap(),
            );
        }
        for id in ids {
            assert_eq!(queue.wait(id, PollTimeout::Block), Some(done()));
        }
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "the session must never be shared"
        );
    }

    #[test]
    fn event_channel_sees_every_outcome() {
        let (queue, rx) = TaskQueue::with_events(2);
        let mut ids = Vec::new();
        for i in 0..8u8 {
            ids.push(
                queue
                    .submit(move || TaskOutcome::Failed(format!("e{i}")), |_, _| {})
                    .unwrap(),
            );
        }
        let mut seen = 0;
        while seen < 8 {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(event) => {
                    assert!(matches!(event.outcome, TaskOutcome::Failed(_)));
                    seen += 1;
                }
                Err(e) => panic!("event stream ended early: {e}"),
            }
        }
    }

    #[test]
    fn outcome_io_error_is_delivered_verbatim() {
        let queue = TaskQueue::new(1);
        let id = queue
            .submit(
                || TaskOutcome::io_error(&std::io::Error::from_raw_os_error(22)),
                |_, _| {},
            )
            .unwrap();
        match queue.wait(id, PollTimeout::Block) {
            Some(TaskOutcome::Failed(msg)) => {
                assert!(msg.contains("Invalid input") || msg.contains("22"), "{msg}")
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
