//! (Merged runtime, a measurement) Who holds the process's mapped memory.
//!
//! Every buffer the host maps for the CPU is noted with where it was asked
//! for (`#[track_caller]`). With `VICTORY_MEMORY_CENSUS=1` a thread walks the
//! address space every 30 s and sums the committed private allocations above
//! the title's 64 GiB by who made them (and how many read as zeros in their
//! middle page), write-combined and write-back apart. Allocations nobody
//! noted are the driver's or the D3D12 runtime's own.

use std::collections::HashMap;
use std::panic::Location;
use std::sync::{Mutex, OnceLock};

type Noted = HashMap<usize, (u64, &'static Location<'static>, &'static str)>;

static NOTED: Mutex<Option<Noted>> = Mutex::new(None);

/// Whether the census runs (`VICTORY_MEMORY_CENSUS=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_MEMORY_CENSUS").is_some_and(|v| v == "1"))
}

/// Notes a mapped buffer: its CPU address, bytes, where it was asked for and
/// its kind. Starts the census thread with the first one.
pub(crate) fn note(ptr: *mut u8, size: u64, at: &'static Location<'static>, kind: &'static str) {
    if !on() || ptr.is_null() {
        return;
    }
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new().name("victory-census".into()).spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                census();
            }
        });
    });
    NOTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(ptr as usize, (size, at, kind));
}

#[cfg(windows)]
fn census() {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PAGE_WRITECOMBINE, VirtualQuery};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // Allocation base -> (committed bytes, write-combined).
    let mut allocs: HashMap<usize, (u64, bool)> = HashMap::new();
    let mut at = 0usize;
    loop {
        // SAFETY: a query that fills our structure.
        let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { VirtualQuery(at as *const _, &mut m, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        let base = m.BaseAddress as usize;
        let next = base + m.RegionSize;
        if m.State == MEM_COMMIT && m.Type == MEM_PRIVATE && (base >> 32) >= 0x10 {
            let wc = m.Protect & PAGE_WRITECOMBINE != 0;
            allocs.entry(m.AllocationBase as usize).or_insert((0, wc)).0 += m.RegionSize as u64;
        }
        if next <= at {
            break;
        }
        at = next;
    }
    let noted = NOTED.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    // Who -> [allocations, bytes, reading zeros, their bytes].
    let mut by: HashMap<String, [u64; 4]> = HashMap::new();
    let mut page = vec![0u8; 4096];
    for (&base, &(bytes, wc)) in &allocs {
        // A note whose buffer was freed may name another allocation made at
        // the same address since: it counts only if the sizes agree (a
        // buffer's allocation is its size rounded up to 64 KiB at most).
        let who = match noted.get(&base) {
            Some((size, loc, kind)) if bytes >= *size && bytes <= size.next_multiple_of(64 << 10) + (64 << 10) => {
                format!("{kind} {}:{}", loc.file().rsplit(['/', '\\']).next().unwrap_or(""), loc.line())
            }
            Some(_) => "not noted (driver or runtime, at a freed buffer's address)".to_owned(),
            None => "not noted (driver or runtime)".to_owned(),
        };
        let key = format!("{} {who}", if wc { "write-combined" } else { "write-back" });
        let mid = base + (bytes as usize / 2) / 4096 * 4096;
        let mut read = 0usize;
        // SAFETY: a read of our own process that fails, not faults, on a page
        // freed meanwhile.
        let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), mid as *const _, page.as_mut_ptr().cast(), page.len(), &mut read) } != 0;
        let zero = ok && read == page.len() && page.iter().all(|&b| b == 0);
        let e = by.entry(key).or_insert([0; 4]);
        e[0] += 1;
        e[1] += bytes;
        if zero {
            e[2] += 1;
            e[3] += bytes;
        }
    }
    let mut rows: Vec<_> = by.into_iter().collect();
    rows.sort_by(|a, b| b.1[1].cmp(&a.1[1]));
    tracing::info!("memory census: {} allocations above 64 GiB, {} noted buffers", allocs.len(), noted.len());
    let [pooled, peak, overflow_live, overflows] = crate::windowup::stats();
    tracing::info!(
        "memory census: transient windows {} ({} MB), shader writes refused {}; their upload pool {} MB (peak {} MB), overflow slabs alive {} MB, overflows {}, sent {} MB since the last census",
        crate::shadow::TRANSIENT[0].load(std::sync::atomic::Ordering::Relaxed),
        crate::shadow::TRANSIENT[1].load(std::sync::atomic::Ordering::Relaxed) >> 20,
        crate::shadow::TRANSIENT[2].load(std::sync::atomic::Ordering::Relaxed),
        pooled >> 20,
        peak >> 20,
        overflow_live >> 20,
        overflows,
        crate::windowup::SENT.swap(0, std::sync::atomic::Ordering::Relaxed) >> 20
    );
    let (idle, idle_bytes) = crate::list::idle_lists();
    tracing::info!(
        "memory census: host lists made since the start {}, alive and never reset {}, kept idle {} ({} MB of upload memory); host list scratch chunks alive {} ({} MB)",
        crate::host::LISTS[0].load(std::sync::atomic::Ordering::Relaxed),
        crate::host::LISTS[3].load(std::sync::atomic::Ordering::Relaxed),
        idle,
        idle_bytes >> 20,
        crate::host::ALIVE[1].load(std::sync::atomic::Ordering::Relaxed),
        crate::host::ALIVE[2].load(std::sync::atomic::Ordering::Relaxed) >> 20
    );
    if let Some(host) = crate::host::host()
        && let Some([budget, used, shared_budget, shared_used]) = host.video_memory()
    {
        tracing::info!(
            "memory census: the card's memory in use {} MB of a budget of {} MB; the PC's memory shared with the card {} MB of {} MB",
            used >> 20,
            budget >> 20,
            shared_used >> 20,
            shared_budget >> 20
        );
    }
    if crate::gpucopy::on() {
        let [on_card, bytes, cpu] = crate::gpucopy::take_stats();
        tracing::info!(
            "memory census: title buffer copies since the last census: {on_card} on the card ({} KB), {cpu} on the CPU (VICTORY_GPU_COPY=1)",
            bytes >> 10
        );
    }
    let ([copies, src_written, into_card, into_pc], lines) = crate::host::blocked_copies_report(8);
    tracing::info!(
        "memory census: title buffer copies into flights' memory that settled the flights since the last census: {copies}, \
         {src_written} of them with the source in flights' memory too; {into_card} into card copies, {into_pc} into PC copies"
    );
    for line in lines {
        tracing::info!("memory census: blocked copy: {line}");
    }
    let (stale, lines) = crate::shadow::stale_report(10);
    tracing::info!("memory census: copies found stale under flights' shader writes since the last census: {stale} in all");
    for line in lines {
        tracing::info!("memory census: stale under writes: {line}");
    }
    tracing::info!(
        "memory census: CPU sides of card copies in the process's own memory {} ({} MB)",
        crate::shadow::PAGEABLE[0].load(std::sync::atomic::Ordering::Relaxed),
        crate::shadow::PAGEABLE[1].load(std::sync::atomic::Ordering::Relaxed) >> 20
    );
    let r = |i: usize| crate::texture::RELEASED[i].load(std::sync::atomic::Ordering::Relaxed);
    tracing::info!(
        "memory census: texture staging buffers alive {} ({} MB); since the start {} let go after their upload ({} MB) and {} taken anew ({} MB)",
        crate::texture::STAGED[0].load(std::sync::atomic::Ordering::Relaxed),
        crate::texture::STAGED[1].load(std::sync::atomic::Ordering::Relaxed) >> 20,
        r(0),
        r(1) >> 20,
        r(2),
        r(3) >> 20
    );
    for (key, [n, bytes, zn, zbytes]) in rows.into_iter().take(16) {
        tracing::info!(
            "memory census: {key}: {n} allocations {} MB, of them {zn} reading zeros in their middle page {} MB",
            bytes >> 20,
            zbytes >> 20
        );
    }
}

#[cfg(not(windows))]
fn census() {}
