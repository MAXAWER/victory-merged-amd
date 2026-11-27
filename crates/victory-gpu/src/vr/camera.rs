//! (Local) The title's camera among its constants.
//!
//! The headset mode shows the frame the title's own camera drew. For a view
//! from the character's eyes the camera itself has to stand there: its
//! matrices, which the title writes into constant buffers its draws name,
//! have to be found and moved.
//!
//! **The probe** (`VICTORY_VR_CAMERA_PROBE=<folder>`): every constant buffer
//! a draw names is counted for the frame (its address in title memory, its
//! size, how many times); when a file `go` appears in the folder, the next
//! frame's buffers are written there as `cam_<n>.txt` (a line `B <address>
//! <size> <times named> <kind>` and a line of their bytes in hex for each;
//! kind 0 a table's constant buffer, 1 a root one, 2 a root buffer a shader
//! reads: bones come that way), and `go` is removed. A script outside looks for the
//! matrices in them. Counting costs the submitting thread a lock for every
//! buffer named: for looking, not for playing.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

/// A buffer the frame's draws named: its size (0 not known: a root one), how
/// many times, and as what (0 a table's constant buffer, 1 a root constant
/// buffer, 2 a root buffer read by a shader).
type Seen = HashMap<u64, (u32, u32, u8)>;

static SEEN: Mutex<Option<Seen>> = Mutex::new(None);
static WRITTEN: AtomicU32 = AtomicU32::new(0);

/// How many of a frame's buffers are written, and how much of each.
const MOST: usize = 600;
const BYTES: usize = 8192;
/// How much of a root buffer a shader reads is written: a character's bones.
const READ_BYTES: usize = 24 << 10;

fn folder() -> Option<&'static std::path::Path> {
    static FOLDER: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    FOLDER.get_or_init(|| std::env::var_os("VICTORY_VR_CAMERA_PROBE").filter(|v| !v.is_empty()).map(Into::into)).as_deref()
}

/// A draw names the constant buffer at `address` of title memory.
pub(crate) fn seen(address: u64, size: u32, kind: u8) {
    if folder().is_none() || address == 0 {
        return;
    }
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let entry = seen.get_or_insert_with(HashMap::new).entry(address).or_insert((size, 0, kind));
    entry.0 = entry.0.max(size);
    entry.1 += 1;
}

/// How many bytes from `address` on can be read, `most` at most.
fn readable(address: u64, most: usize) -> usize {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
    // SAFETY: plain data for the query to fill.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a query of our own address space into a struct of ours.
    let got = unsafe { VirtualQuery(address as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    if got == 0 || info.State != MEM_COMMIT || info.Protect & (PAGE_GUARD | PAGE_NOACCESS) != 0 {
        return 0;
    }
    let end = info.BaseAddress as u64 + info.RegionSize as u64;
    most.min(end.saturating_sub(address) as usize)
}

/// The title's frame is over: the buffers counted for it are forgotten, or,
/// when asked for, written out first.
pub(crate) fn frame_done() {
    fp_keys();
    let Some(folder) = folder() else {
        return;
    };
    let Some(seen) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let go = folder.join("go");
    if !go.exists() {
        return;
    }
    let _ = std::fs::remove_file(&go);
    let n = WRITTEN.fetch_add(1, Relaxed);
    let mut most: Vec<(u64, (u32, u32, u8))> = seen.into_iter().filter(|(_, (size, _, kind))| *kind != 0 || *size >= 48).collect();
    most.sort_by_key(|(address, (_, times, _))| (std::cmp::Reverse(*times), *address));
    most.truncate(MOST);
    let mut text = Vec::new();
    for (address, (size, times, kind)) in &most {
        let want = match (*kind, *size) {
            (2, _) => READ_BYTES,
            (_, 0) => 2048,
            (_, size) => (size as usize).min(BYTES),
        };
        let len = readable(*address, want);
        // SAFETY: committed, readable memory of that length (asked just
        // now; the title's rings stay mapped).
        let bytes = unsafe { std::slice::from_raw_parts(*address as *const u8, len) };
        let _ = writeln!(text, "B {address:x} {size} {times} {kind}");
        for b in bytes {
            let _ = write!(text, "{b:02x}");
        }
        let _ = writeln!(text);
    }
    let path = folder.join(format!("cam_{n:03}.txt"));
    match std::fs::write(&path, text) {
        Ok(()) => tracing::info!(buffers = most.len(), path = %path.display(), "the camera probe: a frame's most named constant buffers are written"),
        Err(e) => tracing::warn!(error = %e, path = %path.display(), "the camera probe: could not write"),
    }
}

/// **First person** (`VICTORY_VR_FP=<right>,<up>,<back>`, metres; F11 turns
/// it on and off while playing, off at the start).
///
/// What the probe found: the title draws relative to its camera, and gives
/// every pass one block of 480 bytes for a view (30 rows of four floats,
/// vectors as rows): the view matrix (rows 0-3, its fourth row 0 0 0 1), the
/// projection (4-7: a perspective one looking down -z, the nearest depth
/// 0.05 at 1, no farthest), the view's inverse (8-11), the camera's place in
/// the world (12) and from its grid cell (13), the frame before's view (14-17,
/// its fourth row what the camera moved since) and projection (18-21), and
/// the two products, view by projection (22-25, 26-29).
///
/// A camera standing `at` from the title's own (in the title's view: to the
/// right, up, back) is those matrices with the place taken off: the view's
/// and the frame before's view's fourth rows less `at`, the inverse's
/// fourth row `at` turned into the world, and the products made again. The
/// camera's place in the world is left as it is: the title's positions are
/// relative to it. The block is changed where the title wrote it, once (a
/// view not yet moved has zeros in its fourth row), before the card reads
/// it. What still stands where the title's camera stands: what the title's
/// CPU decides by it (what is in view at all, levels of detail), and any
/// shader that takes the eye for the origin of its positions.
pub(crate) fn first_person(address: u64, size: u32) {
    if size != 480 || !FP_ON.load(Relaxed) {
        return;
    }
    let Some(at) = fp_place() else {
        return;
    };
    let f = address as *mut f32;
    // SAFETY: a constant buffer of 480 bytes of title memory that a draw
    // names: 120 floats, read and written unaligned.
    unsafe {
        let get = |i: usize| f.add(i).read_unaligned();
        let set = |i: usize, v: f32| f.add(i).write_unaligned(v);
        // A view not moved yet, and a perspective projection down -z.
        if [get(12), get(13), get(14), get(15)] != [0.0, 0.0, 0.0, 1.0] || [get(3), get(7), get(11)] != [0.0; 3] {
            return;
        }
        if get(16) <= 0.0 || get(21) <= 0.0 || get(27) != -1.0 || get(31) != 0.0 || get(17) != 0.0 || get(20) != 0.0 {
            return;
        }
        let row = |m: usize, r: usize| [get(m + r * 4), get(m + r * 4 + 1), get(m + r * 4 + 2), get(m + r * 4 + 3)];
        // Rows of `a` by the matrix at `b`.
        let product = |a: [[f32; 4]; 4], b: usize| -> [[f32; 4]; 4] {
            let b: [[f32; 4]; 4] = std::array::from_fn(|r| row(b, r));
            std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k] * b[k][c]).sum()))
        };
        // The inverse's fourth row: the place turned into the world.
        let inverse: [[f32; 4]; 3] = std::array::from_fn(|r| row(32, r));
        for c in 0..3 {
            set(32 + 12 + c, at[0] * inverse[0][c] + at[1] * inverse[1][c] + at[2] * inverse[2][c]);
        }
        // The view and the frame before's view.
        for c in 0..3 {
            set(12 + c, -at[c]);
            set(56 + 12 + c, get(56 + 12 + c) - at[c]);
        }
        // The products again.
        for (view, projection, to) in [(0, 16, 88), (56, 72, 104)] {
            let moved: [[f32; 4]; 4] = std::array::from_fn(|r| row(view, r));
            let made = product(moved, projection);
            for (r, made) in made.iter().enumerate() {
                for (c, v) in made.iter().enumerate() {
                    set(to + r * 4 + c, *v);
                }
            }
        }
    }
    FP_MOVED.fetch_add(1, Relaxed);
}

static FP_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FP_MOVED: AtomicU32 = AtomicU32::new(0);

/// Where the first-person camera stands from the title's own, in the
/// title's view (right, up, back): asked for (`VICTORY_VR_FP`, or written
/// into the probe folder's `fp` file while playing), or the eyes of a
/// character the title's camera follows from two metres behind.
static FP_PLACE: Mutex<Option<[f32; 3]>> = Mutex::new(None);

/// The eyes of a character followed from behind, where nothing else is said.
const FP_EYES: [f32; 3] = [-0.46, 0.33, -2.12];

fn places(text: &str) -> Option<[f32; 3]> {
    let v: Vec<f32> = text.split(',').filter_map(|v| v.trim().parse().ok()).collect();
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

fn fp_place() -> Option<[f32; 3]> {
    static ASKED: OnceLock<Option<[f32; 3]>> = OnceLock::new();
    let asked = *ASKED.get_or_init(|| std::env::var("VICTORY_VR_FP").ok().map(|v| places(&v).unwrap_or(FP_EYES)));
    FP_PLACE.lock().unwrap_or_else(|e| e.into_inner()).or(asked)
}

/// Once a frame: F11 turns first person on or off; so does a file `fp`
/// appearing in the probe's folder (it is removed), and with three numbers
/// in it the camera is put there and first person is on.
fn fp_keys() {
    static WAS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if fp_place().is_none() {
        return;
    }
    // SAFETY: a plain query of the keyboard.
    let down = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x7A) } < 0;
    let pressed = down && !WAS.swap(down, Relaxed);
    if !down {
        WAS.store(false, Relaxed);
    }
    let file = folder().map(|f| f.join("fp")).filter(|f| f.exists());
    let said = file.as_ref().map(|f| std::fs::read_to_string(f).unwrap_or_default());
    if let Some(file) = &file {
        let _ = std::fs::remove_file(file);
    }
    let put = said.as_deref().and_then(places);
    if let Some(put) = put {
        *FP_PLACE.lock().unwrap_or_else(|e| e.into_inner()) = Some(put);
        FP_ON.store(true, Relaxed);
    } else if pressed || said.is_some() {
        FP_ON.fetch_xor(true, Relaxed);
    } else {
        return;
    }
    tracing::info!(on = FP_ON.load(Relaxed), place = ?fp_place(), views_moved = FP_MOVED.swap(0, Relaxed), "the headset mode: first person");
}
