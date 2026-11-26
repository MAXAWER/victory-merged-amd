//! Host pipelines are made by workers, not by the title thread that first
//! records a draw with them. Translating a shader and the host driver's
//! compile take from milliseconds to seconds each (structurisation of the
//! biggest compute programs ~0.4 s, some driver compiles 3.4 s), and
//! the title's first world frame wants over 1,200 new pipelines: made
//! on its recording threads, that frame took more than 30 s and the
//! title's GPU watchdog ended the run (decision 0007, "Pipelines made off
//! the title's threads").
//!
//! A first use queues the pipeline and waits for it within a budget per
//! frame, so the cheap ones (menus, one-off passes such as a colour-grading
//! bake) are there at once as before; past the budget a draw or dispatch
//! whose pipeline is not ready is dropped, as an untranslated one is.
//!
//! The default budget is half a second per title frame. A pass the title
//! runs only once and whose pipeline takes longer is lost.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::Shader;
use super::host::{GraphicsState, HostPipeline};

/// What a host pipeline is made from, and the host pipeline once made.
pub(crate) struct Pending {
    shaders: Vec<Arc<Shader>>,
    pub(crate) graphics: Option<Box<GraphicsState>>,
    queue_state: AtomicU8,
    made: OnceLock<Option<HostPipeline>>,
    /// What identifies it across runs (shader containers and state), and
    /// whether a draw or dispatch has requested it yet.
    pub(crate) key: String,
    used: AtomicBool,
    /// When a worker began it ([`slow`]).
    begun: OnceLock<Instant>,
}

// SAFETY: D3D12 objects are free-threaded; the graphics state (whose input
// elements point into its own names) is only read after creation.
unsafe impl Send for Pending {}
// SAFETY: as above.
unsafe impl Sync for Pending {}

/// What identifies a title pipeline across runs (and the pre-build).
pub(crate) fn key(shaders: &[Arc<Shader>], graphics: Option<&GraphicsState>) -> String {
    let mut key = blake3::Hasher::new();
    for s in shaders {
        key.update(s.stage.as_bytes());
        key.update(s.hash.as_bytes());
    }
    key.update(graphics.map(|g| g.describe()).unwrap_or_default().as_bytes());
    key.finalize().to_hex()[..32].to_owned()
}

impl Pending {
    /// Whether it is a compute pipeline.
    fn computes(&self) -> bool {
        matches!(self.shaders.as_slice(), [only] if only.stage == "compute")
    }

    pub(crate) fn new(shaders: Vec<Arc<Shader>>, graphics: Option<Box<GraphicsState>>) -> Pending {
        Pending {
            key: key(&shaders, graphics.as_deref()),
            shaders,
            graphics,
            queue_state: AtomicU8::new(NOT_QUEUED),
            made: OnceLock::new(),
            used: AtomicBool::new(false),
            begun: OnceLock::new(),
        }
    }

    fn make(&self) {
        let _ = self.begun.set(Instant::now());
        // Not through `OnceLock::get_or_init`: a worker the driver's compiler
        // crashed under stays inside the call for good (see [`guard_start`]),
        // and anyone else asking would wait for it forever.
        if self.made.get().is_none() {
            let made = if is_bad_key(&self.key) {
                SKIPPED_BAD.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(key = self.key, shaders = ?self.shaders, "not made: the driver's compiler crashed on it in this run");
                None
            } else {
                HostPipeline::new(&self.shaders, self.graphics.as_deref())
            };
            let _ = self.made.set(made);
        }
        let (lock, done) = &DONE;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        done.notify_all();
    }
}

// The driver's compiler crashing on a worker (Belobug/VIctory-Belobug-AMD
// 81ebb7bd, "lock-free compiler VEH", GPL-3.0-or-later; adapted to this tree's
// single queue).

/// Pipelines not made for having crashed the driver's compiler in this run.
static SKIPPED_BAD: AtomicU64 = AtomicU64::new(0);

/// The keys of the pipelines the driver's compiler crashed on in this run.
/// shortcut: not kept across runs: on the RX 9070 XT the crashes are not on
/// the same pipelines in every run (Belobug), and a pipeline left out for good
/// is a pass that never draws. They are written to `compiler-crashed.txt`
/// beside the pipeline library, for a look.
static BAD_KEYS: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

fn bad_keys<R>(f: impl FnOnce(&mut std::collections::HashSet<String>) -> R) -> R {
    let mut keys = BAD_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    f(keys.get_or_insert_with(Default::default))
}

fn is_bad_key(key: &str) -> bool {
    bad_keys(|keys| keys.contains(key))
}

/// Whether the driver's compiler crashes are caught
/// (`VICTORY_COMPILER_GUARD=0`: not).
fn guard_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COMPILER_GUARD").is_none_or(|v| v != "0"))
}

/// Worker threads that can be told apart by the guard, over the run.
const MAX_SLOTS: usize = 128;
/// Per worker slot: its compile crashed (set in the handler), where.
static CRASHED: [AtomicBool; MAX_SLOTS] = [const { AtomicBool::new(false) }; MAX_SLOTS];
static FAULT_ADDR: [AtomicU64; MAX_SLOTS] = [const { AtomicU64::new(0) }; MAX_SLOTS];
static FAULT_RIP: [AtomicU64; MAX_SLOTS] = [const { AtomicU64::new(0) }; MAX_SLOTS];
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);
/// The event the handler signals (a Win32 handle).
static CRASH_EVENT: AtomicUsize = AtomicUsize::new(0);
/// What each slot's worker is making. Never locked by the handler.
static ACTIVE_JOBS: Mutex<Vec<Option<Arc<Pending>>>> = Mutex::new(Vec::new());

thread_local! {
    /// This worker's slot (`usize::MAX`: none).
    static WORKER_SLOT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    /// Whether this thread is inside the compile of a pipeline.
    static COMPILING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A worker's compile of `p`: noted for the guard while it lasts.
struct Compile {
    slot: usize,
}

impl Compile {
    fn begin(slot: usize, p: &Arc<Pending>) -> Compile {
        if slot < MAX_SLOTS && guard_on() {
            let mut jobs = ACTIVE_JOBS.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.len() < MAX_SLOTS {
                jobs.resize(MAX_SLOTS, None);
            }
            jobs[slot] = Some(p.clone());
            COMPILING.set(true);
        }
        Compile { slot }
    }
}

impl Drop for Compile {
    fn drop(&mut self) {
        if COMPILING.replace(false) && self.slot < MAX_SLOTS {
            let mut jobs = ACTIVE_JOBS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(job) = jobs.get_mut(self.slot) {
                *job = None;
            }
        }
    }
}

/// Installs the handler for the driver's compiler crashing (AMD's does on an
/// RX 9070 XT, and not on the same pipelines in every run: an access of the
/// first pages in `amdxc64.dll`) and the thread that deals with what it
/// left. The handler does the least there is: it notes where, signals, and
/// leaves the worker asleep for good. No allocation, no lock, no file: the
/// crash may have been under any of them. The watcher says the pipeline is
/// not made, gives the worker's place in the counts back and starts another
/// worker.
fn guard_start() {
    use windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler;
    use windows_sys::Win32::System::Threading::CreateEventW;
    static ONCE: std::sync::Once = std::sync::Once::new();
    if !guard_on() {
        return;
    }
    ONCE.call_once(|| {
        // SAFETY: an unnamed auto-reset event, no security attributes.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if event.is_null() {
            tracing::warn!("no event for the driver's compiler crashes: they are not caught");
            return;
        }
        CRASH_EVENT.store(event as usize, Ordering::Release);
        let spawned = std::thread::Builder::new().name("victory-pipeline-watch".into()).spawn(watch);
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "no thread for the driver's compiler crashes: they are not caught");
            return;
        }
        // SAFETY: a handler that lives as long as the process.
        unsafe {
            AddVectoredExceptionHandler(1, Some(compiler_crashed));
        }
        tracing::info!("the driver's compiler crashing on a worker is caught (VICTORY_COMPILER_GUARD=0: not)");
    });
}

/// The handler: a worker inside a compile read or wrote one of the first
/// pages. See [`guard_start`].
unsafe extern "system" fn compiler_crashed(
    info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use windows_sys::Win32::System::Threading::{SetEvent, Sleep};
    const ACCESS_VIOLATION: i32 = 0xc000_0005_u32 as i32;
    const CONTINUE_SEARCH: i32 = 0;
    // SAFETY: the system passes a live record.
    let record = unsafe { &*(*info).ExceptionRecord };
    if record.ExceptionCode != ACCESS_VIOLATION || record.NumberParameters < 2 || record.ExceptionInformation[1] >= 0x10000 {
        return CONTINUE_SEARCH;
    }
    if !COMPILING.try_with(|c| c.get()).unwrap_or(false) {
        return CONTINUE_SEARCH;
    }
    let slot = WORKER_SLOT.try_with(|s| s.get()).unwrap_or(usize::MAX);
    if slot >= MAX_SLOTS {
        return CONTINUE_SEARCH;
    }
    let _ = COMPILING.try_with(|c| c.set(false));
    FAULT_ADDR[slot].store(record.ExceptionInformation[1] as u64, Ordering::Relaxed);
    #[cfg(target_arch = "x86_64")]
    // SAFETY: the system passes a live context.
    FAULT_RIP[slot].store(unsafe { (*(*info).ContextRecord).Rip }, Ordering::Relaxed);
    CRASHED[slot].store(true, Ordering::Release);
    let event = CRASH_EVENT.load(Ordering::Acquire);
    if event != 0 {
        // SAFETY: the event lives as long as the process.
        unsafe { SetEvent(event as windows_sys::Win32::Foundation::HANDLE) };
    }
    loop {
        // SAFETY: a plain sleep.
        unsafe { Sleep(u32::MAX - 1) };
    }
}

/// The thread that deals with the workers the compiler crashed under.
fn watch() {
    use windows_sys::Win32::System::Threading::{INFINITE, WaitForSingleObject};
    loop {
        let event = CRASH_EVENT.load(Ordering::Acquire);
        // SAFETY: the event lives as long as the process.
        unsafe { WaitForSingleObject(event as windows_sys::Win32::Foundation::HANDLE, INFINITE) };
        for slot in 0..MAX_SLOTS {
            if CRASHED[slot].swap(false, Ordering::AcqRel) {
                lost(slot);
            }
        }
    }
}

/// The worker at `slot` is gone with the pipeline it was making.
fn lost(slot: usize) {
    let job = {
        let mut jobs = ACTIVE_JOBS.lock().unwrap_or_else(|e| e.into_inner());
        jobs.get_mut(slot).and_then(Option::take)
    };
    let Some(p) = job else {
        return;
    };
    tracing::error!(
        key = p.key,
        shaders = ?p.shaders,
        rip = format_args!("{:#x}", FAULT_RIP[slot].load(Ordering::Relaxed)),
        address = format_args!("{:#x}", FAULT_ADDR[slot].load(Ordering::Relaxed)),
        "the driver's compiler crashed making a pipeline: it is left out for this run, the worker is replaced"
    );
    let known = bad_keys(|keys| !keys.insert(p.key.clone()));
    if !known && let Some(dir) = super::library::folder() {
        use std::io::Write;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("compiler-crashed.txt"));
        if let Ok(mut f) = file {
            let _ = writeln!(f, "{} ended the compiler in a run: {:?}", p.key, p.shaders);
        }
    }
    let _ = p.made.set(None);
    {
        let (lock, done) = &DONE;
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
        done.notify_all();
    }
    if WAITING.fetch_sub(1, Ordering::Relaxed) == 1 {
        drained();
    }
    if !spawn_worker() {
        tracing::warn!("no worker to take the place of the one the compiler crashed under");
    }
}

/// Why a pipeline has no host pipeline (diagnostics): its shaders that do
/// not translate, else "refused" or "being made"; `None` if it has one.
#[cfg(feature = "diagnostics")]
pub(crate) fn missing(p: &Pending) -> Option<String> {
    match p.made.get() {
        Some(Some(_)) => None,
        None => Some("being made".into()),
        Some(None) => {
            let failed: Vec<String> = p
                .shaders
                .iter()
                .filter(|s| matches!(s.translated.get(), Some(None)))
                .map(|s| format!("{} {}", s.stage, s.hash))
                .collect();
            Some(if failed.is_empty() {
                format!("refused {:?}", p.shaders)
            } else {
                failed.join(" + ")
            })
        }
    }
}

/// The shaders `p` is made of.
pub(crate) fn shaders(p: &Pending) -> &[Arc<Shader>] {
    &p.shaders
}

/// The hashes of the shaders `p` is made of.
pub(crate) fn hashes(p: &Pending) -> String {
    let hashes: Vec<&str> = p.shaders.iter().map(|s| s.hash.as_str()).collect();
    hashes.join("+")
}

/// Signalled whenever a worker has made a pipeline.
static DONE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());

/// Waiting allowed per title frame, over all threads.
fn wait_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let millis = std::env::var("VICTORY_PIPELINE_WAIT_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(500);
        tracing::info!(millis, "host pipeline wait budget per frame");
        Duration::from_millis(millis)
    })
}

/// Whether the host's driver has been at `p` for longer than most take
/// (one of the title's takes 85 s): no frame waits for it.
fn slow(p: &Pending) -> bool {
    // A pass a title runs once (it unpacks a model, say) is lost if its
    // pipeline is not there; a draw comes again in the next frame.
    let most = if p.computes() { 10 } else { 3 };
    p.begun.get().is_some_and(|at| at.elapsed() > Duration::from_secs(most))
}

/// What a frame may wait while a world loads: pipelines are asked for by
/// the hundred then, and a pass the title runs once is lost if its
/// pipeline is not there. All such waits of a run together are bounded;
/// the title's GPU watchdog bounds one frame's (30 s).
const LOADING_FRAME: Duration = Duration::from_secs(4);
/// The same for compute pipelines, waited for apart: the passes that unpack
/// a world's models are dispatched once, and in a run whose pipelines were
/// all still to be loaded 130 of them were dropped, and with them walls,
/// floor, ceiling and faces (they never came back).
const LOADING_FRAME_COMPUTE: Duration = Duration::from_secs(12);

/// `VICTORY_PIPELINE_GATE_S`: the bound on all of them (0: none of them).
fn loading_left(spent: Duration) -> Duration {
    static TOTAL: OnceLock<Duration> = OnceLock::new();
    let total = *TOTAL.get_or_init(|| {
        let seconds = std::env::var("VICTORY_PIPELINE_GATE_S")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(120);
        Duration::from_secs(seconds)
    });
    total.saturating_sub(spent)
}

/// Pipelines queued and not made yet.
static WAITING: AtomicU64 = AtomicU64::new(0);

/// Pipelines queued and not made yet (for the frame line).
pub(crate) fn waiting() -> u64 {
    WAITING.load(Ordering::Relaxed)
}

/// The host pipeline if it is made (or is made within the frame's budget);
/// queues it on first use.
pub(crate) fn ready(p: &Arc<Pending>) -> Option<&HostPipeline> {
    // A still-pending pipeline was requested by the title too. Prewarm it
    // on the next launch, even if this run never got to use it.
    if !p.used.load(Ordering::Relaxed) && !p.used.swap(true, Ordering::Relaxed) {
        super::library::note_used(&p.key);
    }
    if p.made.get().is_none() {
        start(p, REQUESTED);
        wait(p, crate::frame::current());
    }
    p.made.get()?.as_ref()
}

/// Makes `p` on this thread if it is not made yet (waiting for a worker
/// that is making it); whether it has a host pipeline.
pub(crate) fn now(p: &Arc<Pending>) -> bool {
    p.make();
    p.made.get().is_some_and(|made| made.is_some())
}

/// Whether `p` has a host pipeline, without waiting: `Some(true)` made,
/// `Some(false)` it cannot be, `None` still being made (queued for the
/// workers now if it was not). For the ray-tracing structure builder, which
/// waits for all of its pipelines ([`super::builder_runs`]).
pub(crate) fn asked(p: &Arc<Pending>) -> Option<bool> {
    if !p.used.load(Ordering::Relaxed) && !p.used.swap(true, Ordering::Relaxed) {
        super::library::note_used(&p.key);
    }
    if p.made.get().is_none() {
        start(p, REQUESTED);
    }
    p.made.get().map(Option::is_some)
}

/// At the title's creation of a pipeline: starts it at once if an earlier
/// run drew with it (during loading, so the world's first frames find it).
pub(crate) fn early(p: &Arc<Pending>) {
    if !p.shaders.is_empty() && super::library::wanted_early(&p.key) {
        super::library::EARLY.fetch_add(1, Ordering::Relaxed);
        start(p, EARLY);
    }
}

/// Whether the frame's budget is one for all threads together: a thread
/// that waits takes what is left of it and gives back what it did not
/// use. Counted only when a wait had ended, the budget was one per thread:
/// each of the title's five recording threads began its first wait of a
/// frame with nothing counted yet and waited the whole half second, 2.5 s
/// of waiting in every frame in which pipelines were being made (runs
/// 34-49: "2504 ms waiting for pipelines" in each such frame).
/// On by default; `VICTORY_PIPELINE_WAIT_SHARED=0` counts as before.
fn shared_budget() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_PIPELINE_WAIT_SHARED").is_none_or(|v| v != "0"))
}

/// Waits for `p` while frame `frame` has budget left.
fn wait(p: &Pending, frame: u64) {
    let budget = wait_budget();
    if budget.is_zero() || slow(p) {
        return;
    }
    // What was waited past the budget since the start, in milliseconds; the
    // second of each is the compute pipelines'.
    static LOADINGS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    let compute = p.computes();
    let past_total = &LOADINGS[usize::from(compute)];
    let loading = loading_left(Duration::from_millis(past_total.load(Ordering::Relaxed)));
    let budget = budget.max((if compute { LOADING_FRAME_COMPUTE } else { LOADING_FRAME }).min(loading));
    // The frame the budget belongs to, and what was waited of it (or is
    // taken by a thread waiting now).
    static SPENTS: [Mutex<(u64, Duration)>; 2] = [const { Mutex::new((u64::MAX, Duration::ZERO)) }; 2];
    let frame_spent = &SPENTS[usize::from(compute)];
    let shared = shared_budget();
    let left = {
        let mut spent = frame_spent.lock().unwrap_or_else(|e| e.into_inner());
        if spent.0 != frame {
            *spent = (frame, Duration::ZERO);
        }
        let left = budget.saturating_sub(spent.1);
        if shared {
            spent.1 += left;
        }
        left
    };
    if left.is_zero() {
        return;
    }
    let started = Instant::now();
    let (lock, done) = &DONE;
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while p.made.get().is_none() && !slow(p) {
        let rest = left.saturating_sub(started.elapsed());
        if rest.is_zero() {
            break;
        }
        // Woken now and then to see whether it has become a slow one.
        let rest = rest.min(Duration::from_millis(250));
        held = done.wait_timeout(held, rest).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(held);
    let waited = started.elapsed().min(left);
    let past = waited.saturating_sub(wait_budget());
    past_total.fetch_add(past.as_millis() as u64, Ordering::Relaxed);
    {
        let mut spent = frame_spent.lock().unwrap_or_else(|e| e.into_inner());
        // A frame that began meanwhile has its own budget.
        if spent.0 == frame {
            if shared {
                spent.1 = spent.1.saturating_sub(left - waited);
            } else {
                spent.1 += waited;
            }
        }
    }
    crate::framelog::add(crate::framelog::T::PipelineWaitUs, started.elapsed().as_micros() as u64);
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(
        crate::copystat::C::PipelineWaitUs,
        started.elapsed().as_micros() as u64,
    );
}

const NOT_QUEUED: u8 = 0;
const EARLY: u8 = 1;
const REQUESTED: u8 = 2;
const MAKING: u8 = 3;

struct Queue {
    early: VecDeque<Arc<Pending>>,
    early_depth: VecDeque<Arc<Pending>>,
    requested: VecDeque<Arc<Pending>>,
}

impl Queue {
    const fn new() -> Self {
        Self {
            early: VecDeque::new(),
            early_depth: VecDeque::new(),
            requested: VecDeque::new(),
        }
    }

    /// Queues an unrequested pipeline or promotes one queued at creation.
    /// Returns whether it is new to the queue, or `None` if nothing changed.
    fn push(&mut self, p: &Arc<Pending>, priority: u8) -> Option<bool> {
        let prev = p.queue_state.load(Ordering::Relaxed);
        if prev >= priority {
            return None;
        }
        p.queue_state.store(priority, Ordering::Relaxed);
        if priority == REQUESTED {
            self.requested.push_back(p.clone());
        } else if p.graphics.as_ref().is_some_and(|state| state.writes_depth_only()) {
            // Shadow maps and depth prepasses supply several later draws. Rebuild
            // their previously used PSOs first while no draw is requesting one.
            self.early_depth.push_back(p.clone());
        } else {
            self.early.push_back(p.clone());
        }
        Some(prev == NOT_QUEUED)
    }

    fn pop(&mut self) -> Option<Arc<Pending>> {
        let p = self.requested.pop_front().or_else(|| {
            // An early entry can have been promoted and claimed already.
            std::iter::from_fn(|| self.early_depth.pop_front().or_else(|| self.early.pop_front()))
                .find(|p| p.queue_state.load(Ordering::Relaxed) == EARLY)
        })?;
        p.queue_state.store(MAKING, Ordering::Relaxed);
        Some(p)
    }
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue::new());
static AVAILABLE: Condvar = Condvar::new();

/// Start or promote work for a draw without waiting behind the entire early
/// list. Queue changes and worker claims happen under the same lock.
fn start(p: &Arc<Pending>, priority: u8) {
    if p.queue_state.load(Ordering::Relaxed) >= priority {
        return;
    }
    if !workers() {
        if p.queue_state.swap(MAKING, Ordering::Relaxed) == NOT_QUEUED {
            p.make();
        }
        return;
    }
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(new) = queue.push(p, priority) {
        if new {
            PEAK.fetch_max(WAITING.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        }
        AVAILABLE.notify_one();
    }
}

fn next() -> Arc<Pending> {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(p) = queue.pop() {
            return p;
        }
        queue = AVAILABLE.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// Starts the workers (half the logical processors, 2 to 8) on first use.
fn workers() -> bool {
    static RUNNING: OnceLock<bool> = OnceLock::new();
    *RUNNING.get_or_init(|| {
        let mut running = false;
        let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 8));
        guard_start();
        for _ in 0..workers {
            if spawn_worker() {
                running = true;
            }
        }
        running
    })
}

/// Starts one worker.
fn spawn_worker() -> bool {
    let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name(format!("victory-pipelines-{slot}"))
        .spawn(move || {
            WORKER_SLOT.set(slot);
            // Below the title's own threads (loading, recording).
            // SAFETY: a plain call on our own thread.
            unsafe {
                use windows::Win32::System::Threading::*;
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
            }
            loop {
                let p = next();
                {
                    // (If the compiler crashes in here this thread never
                    // returns: `lost` does what follows for it.)
                    let _compile = Compile::begin(slot, &p);
                    p.make();
                }
                if WAITING.fetch_sub(1, Ordering::Relaxed) == 1 {
                    drained();
                } else {
                    super::library::save(false);
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "no pipeline worker");
        false
    } else {
        true
    }
}

/// The most pipelines waiting at once since the workers last ran out.
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The workers ran out of work: logged after a batch of 10 or more, and
/// the cache saved (at most every 20 s).
fn drained() {
    use super::library::{EARLY, LOADED, MADE};
    if PEAK.swap(0, Ordering::Relaxed) >= 10 {
        tracing::info!(
            made = MADE.load(Ordering::Relaxed),
            loaded = LOADED.load(Ordering::Relaxed),
            started_early = EARLY.load(Ordering::Relaxed),
            "host pipelines all made"
        );
    }
    super::library::save(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A first use queues the pipeline and, within the budget, gets the
    /// worker's answer (here: no host pipeline for no shaders).
    #[test]
    fn first_use_is_made_by_a_worker() {
        let p = Arc::new(Pending::new(Vec::new(), None));
        assert!(ready(&p).is_none());
        assert_ne!(p.queue_state.load(Ordering::Relaxed), NOT_QUEUED);
        let started = Instant::now();
        while p.made.get().is_none() && started.elapsed() < Duration::from_secs(10) {
            std::thread::yield_now();
        }
        assert!(matches!(p.made.get(), Some(None)));
    }

    #[test]
    fn pending_pipeline_is_recorded_for_the_next_launch() {
        let mut p = Pending::new(Vec::new(), None);
        p.key = "pending-pipeline-test".to_owned();
        // Simulate a worker that has taken the job but cannot finish it yet.
        p.queue_state.store(MAKING, Ordering::Relaxed);
        let p = Arc::new(p);

        assert!(!super::super::library::wanted_early(&p.key));
        assert!(ready(&p).is_none());
        assert!(p.made.get().is_none());
        assert!(super::super::library::wanted_early(&p.key));
    }

    #[test]
    fn draws_promote_early_pipelines_without_claiming_them_twice() {
        let earlier = Arc::new(Pending::new(Vec::new(), None));
        let promoted = Arc::new(Pending::new(Vec::new(), None));
        let requested = Arc::new(Pending::new(Vec::new(), None));
        let mut queue = Queue::new();

        assert_eq!(queue.push(&earlier, EARLY), Some(true));
        assert_eq!(queue.push(&promoted, EARLY), Some(true));
        assert_eq!(queue.push(&requested, REQUESTED), Some(true));
        assert_eq!(queue.push(&promoted, REQUESTED), Some(false));
        assert_eq!(queue.push(&promoted, REQUESTED), None);

        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &requested));
        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &promoted));
        assert_eq!(queue.push(&promoted, REQUESTED), None);
        assert!(Arc::ptr_eq(&queue.pop().unwrap(), &earlier));
        assert!(queue.pop().is_none());
    }

    #[test]
    fn early_depth_precedes_other_prewarming_and_requests_precede_both() {
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_D32_FLOAT;
        let depth_pipeline = || {
            let mut depth = Box::new(GraphicsState::default());
            let format = [16u32, DXGI_FORMAT_D32_FLOAT.0 as u32];
            // SAFETY: a complete depth-format subobject of the title pipeline stream.
            unsafe { depth.read(16, format.as_ptr().cast()) };
            Arc::new(Pending::new(Vec::new(), Some(depth)))
        };
        let normal = Arc::new(Pending::new(Vec::new(), None));
        let promoted = depth_pipeline();
        let shadow = depth_pipeline();
        let mut queue = Queue::new();
        for p in [&normal, &shadow, &promoted] {
            assert_eq!(queue.push(p, EARLY), Some(true));
        }
        assert_eq!(queue.push(&promoted, REQUESTED), Some(false));
        for expected in [&promoted, &shadow, &normal] {
            assert!(Arc::ptr_eq(&queue.pop().unwrap(), expected));
        }
        assert!(queue.pop().is_none());
    }
}
