//! (Local) A headset mode over SteamVR (`VICTORY_VR=1`; off by default).
//!
//! What there is of it:
//!
//! - **The picture** ([`scene`]): a scene of our own for each eye at the
//!   headset's rate, made of the last frame the title presented and its
//!   depth buffer. Three ways to see it (`VICTORY_VR_MODE`, F6 goes to the
//!   next while playing): `3d`, the frame as a relief around the player, as
//!   if standing where the title's camera stands, `cinema`, a flat screen in
//!   the room, and `immersive`, the relief fixed to the head with the head's
//!   turning given to the title's camera stick. F7 centres the view where
//!   the head looks; F8 and F9 show the frame smaller and larger (with Ctrl:
//!   the whole nearer and further off).
//! - **What the player holds** ([`glove`], [`hands`], [`scene`]): at each
//!   controller the player's hand, SteamVR's own hand model with its fingers
//!   bent by grip and trigger, or the controller as SteamVR draws it; F10
//!   goes from one to the other (by default the controllers while watching,
//!   the hands where the head turns the camera, and a closing hand is always
//!   a hand).
//! - **What the player wears** ([`track`], [`body`]): the trackers on the
//!   body (waist, feet, knees, elbows...), by the role SteamVR gives each,
//!   shown as markers and given out for whoever moves a body with them.
//! - **The controllers as the title's pad** ([`track`]): sticks, triggers
//!   and buttons, and a fist thrown forward presses the title's strike
//!   button.
//! - **Controllers that are not there** ([`emu`], `VICTORY_VR_EMU=1`), for
//!   trying all of it without them; with SteamVR's own stand-in headset
//!   (`Play-VI-VR.cmd -VREmu`) no headset is needed at all.
//!
//! How it talks to SteamVR: `openvr_api.dll` of the installed runtime (found
//! through `%LOCALAPPDATA%\openvr\openvrpaths.vrpath`, or
//! `VICTORY_VR_RUNTIME=<the runtime's folder>`), loaded from its own folder,
//! as a scene application. The interfaces are the C function tables of
//! `IVRSystem_026`, `IVRCompositor_029` and `IVRRenderModels_006`
//! (ValveSoftware/openvr, `headers/openvr_capi.h`, BSD-3-Clause): the
//! indices here are those tables'.
//!
//! The frame for the headset is taken where the title's frame goes to the
//! window ([`frame`], [`show`]): the back buffer and the depth buffer the
//! title draws its frames with ([`depth_bound`]) are copied into one of
//! three snapshots of ours, and the copy is fenced on the title's queue.

use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::host::{Host, transition};

pub(crate) mod camera;
mod emu;
mod glove;
mod hands;
mod json;
mod math;
mod scene;
mod track;

use math::M34;

const SYSTEM_INTERFACE: &CStr = c"FnTable:IVRSystem_026";
const COMPOSITOR_INTERFACE: &CStr = c"FnTable:IVRCompositor_029";
const MODELS_INTERFACE: &CStr = c"FnTable:IVRRenderModels_006";

/// `IVRSystem_026`'s function table.
mod system {
    pub(super) const RECOMMENDED_SIZE: usize = 0;
    pub(super) const PROJECTION_RAW: usize = 2;
    pub(super) const EYE_TO_HEAD: usize = 5;
    pub(super) const POSES: usize = 12;
    pub(super) const INDEX_FOR_ROLE: usize = 18;
    pub(super) const DEVICE_CLASS: usize = 20;
    pub(super) const STRING_PROPERTY: usize = 28;
    pub(super) const CONTROLLER_STATE: usize = 37;
    pub(super) const HAPTIC_PULSE: usize = 39;
    pub(super) const ENTRIES: usize = 51;
}

/// `IVRCompositor_029`'s function table.
mod compositor {
    pub(super) const WAIT_GET_POSES: usize = 2;
    pub(super) const SUBMIT: usize = 6;
    pub(super) const ENTRIES: usize = 53;
}

/// `IVRRenderModels_006`'s function table.
mod models {
    pub(super) const LOAD_MODEL: usize = 0;
    pub(super) const FREE_MODEL: usize = 1;
    pub(super) const LOAD_TEXTURE: usize = 2;
    pub(super) const FREE_TEXTURE: usize = 3;
    pub(super) const ENTRIES: usize = 19;
}

const APPLICATION_SCENE: i32 = 1;
const UNIVERSE_STANDING: i32 = 1;
const TEXTURE_DIRECTX12: i32 = 4;
type InitInternal = unsafe extern "C" fn(*mut i32, i32, *const c_char) -> u32;
type GetGenericInterface = unsafe extern "C" fn(*const c_char, *mut i32) -> isize;
type ErrorText = unsafe extern "C" fn(i32) -> *const c_char;

/// `Texture_t`.
#[repr(C)]
struct Texture {
    handle: *mut c_void,
    kind: i32,
    color_space: i32,
}

/// `D3D12TextureData_t`.
#[repr(C)]
struct TextureData {
    resource: *mut c_void,
    queue: *mut c_void,
    node_mask: u32,
}

/// Whether the headset mode is on (`VICTORY_VR=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_VR").is_some_and(|v| v == "1"))
}

fn number(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(default)
}

/// How the frame is shown in the headset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Mode {
    /// A relief around the player, from the title's depth.
    Relief,
    /// A flat screen in the room.
    Cinema,
    /// A flat screen fixed to the head; the head turns the title's camera.
    Immersive,
}

static MODE: AtomicU8 = AtomicU8::new(u8::MAX);

pub(crate) fn mode() -> Mode {
    let mut m = MODE.load(Relaxed);
    if m == u8::MAX {
        m = match std::env::var("VICTORY_VR_MODE").as_deref() {
            Ok("cinema") => 1,
            Ok("immersive") => 2,
            _ => 0,
        };
        MODE.store(m, Relaxed);
    }
    match m {
        1 => Mode::Cinema,
        2 => Mode::Immersive,
        _ => Mode::Relief,
    }
}

/// The next way of showing the frame (F6).
fn next_mode() -> Mode {
    let next: u8 = match mode() {
        Mode::Relief => 1,
        Mode::Cinema => 2,
        Mode::Immersive => 0,
    };
    MODE.store(next, Relaxed);
    RECENTRE.store(true, Relaxed);
    mode()
}

/// The view is to be centred where the head looks now (F7, a change of mode).
static RECENTRE: AtomicBool = AtomicBool::new(false);

/// The relief's scale (bits of an `f32`; 0 not yet read).
static WORLD_SCALE: AtomicU32 = AtomicU32::new(0);

fn world_scale() -> f32 {
    match WORLD_SCALE.load(Relaxed) {
        0 => number("VICTORY_VR_SCALE", 1.0).clamp(0.05, 20.0),
        bits => f32::from_bits(bits),
    }
}

/// How much of the view the title's frame takes: its height as an angle
/// (bits of an `f32` of radians; 0 not yet read). This is not the title's
/// own field of view, which is not known here and changes from shot to shot:
/// it is how large the frame is shown.
static PICTURE: AtomicU32 = AtomicU32::new(0);

fn picture(mode: Mode) -> f32 {
    match PICTURE.load(Relaxed) {
        0 => match std::env::var("VICTORY_VR_FOV").ok().and_then(|v| v.trim().parse::<f32>().ok()) {
            Some(asked) => asked.clamp(20.0, 140.0).to_radians(),
            // As the headset sees: a little over its own view where the
            // head looks about in the frame (no edge of it in sight), the
            // view itself where the frame is fixed to the head (its corners
            // stay in sight).
            None => match f32::from_bits(VIEW_UP.load(Relaxed)) {
                up if up > 0.0 => (2.0 * (up * if mode == Mode::Immersive { 1.0 } else { 1.05 }).atan()).clamp(60f32.to_radians(), 125f32.to_radians()),
                _ => 90f32.to_radians(),
            },
        },
        bits => f32::from_bits(bits),
    }
}

/// The headset's own view: the tangent of half its height (bits of an
/// `f32`; 0 not known).
static VIEW_UP: AtomicU32 = AtomicU32::new(0);

fn view_is(up: f32) {
    VIEW_UP.store(up.max(0.0).to_bits(), Relaxed);
}

/// The frame shown larger (`by` over 1) or smaller.
fn size_picture(by: f32) -> f32 {
    let angle = (picture(mode()) * by).clamp(20f32.to_radians(), 140f32.to_radians());
    PICTURE.store(angle.to_bits(), Relaxed);
    angle
}

/// The whole further off and larger (`by` over 1) or nearer and smaller.
/// The relief deeper (`by` over 1) or shallower.
fn scale_world(by: f32) -> f32 {
    let scale = (world_scale() * by).clamp(0.05, 20.0);
    WORLD_SCALE.store(scale.to_bits(), Relaxed);
    scale
}

/// What is drawn where a controller is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Held {
    /// The controller itself, as SteamVR draws it.
    Controller,
    /// The player's hand.
    Hand,
}

/// 0 by the way of looking (the controller while watching, the hand where
/// the head turns the camera), 1 hands, 2 controllers (F10 goes round).
static HELD: AtomicU8 = AtomicU8::new(u8::MAX);

pub(crate) fn held() -> Held {
    let mut h = HELD.load(Relaxed);
    if h == u8::MAX {
        h = match std::env::var("VICTORY_VR_HANDS").as_deref() {
            Ok("1") => 1,
            Ok("0") => 2,
            _ => 0,
        };
        HELD.store(h, Relaxed);
    }
    match (h, mode()) {
        (1, _) | (0, Mode::Immersive) => Held::Hand,
        _ => Held::Controller,
    }
}

/// The other thing at the controllers (F10).
fn next_held() -> Held {
    HELD.store(if held() == Held::Hand { 2 } else { 1 }, Relaxed);
    held()
}

/// A hand as the scene draws it.
#[derive(Clone, Debug, PartialEq)]
struct HandView {
    present: bool,
    /// SteamVR's number for the controller (the scene takes its place from
    /// the poses it draws with); `None` for an emulated one, at `pose`.
    device: Option<u32>,
    pose: M34,
    /// Thumb, index, the others: 0 flat, 1 a fist.
    curl: [f32; 3],
    grip: f32,
    /// The controller's model, by SteamVR's name for it.
    model: String,
}

impl Default for HandView {
    fn default() -> Self {
        Self { present: false, device: None, pose: math::IDENTITY, curl: [0.0; 3], grip: 0.0, model: String::new() }
    }
}

/// Where a tracker is worn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyPart {
    Waist,
    Chest,
    LeftFoot,
    RightFoot,
    LeftKnee,
    RightKnee,
    LeftElbow,
    RightElbow,
    LeftShoulder,
    RightShoulder,
    /// Worn somewhere SteamVR was not told.
    Other,
}

impl BodyPart {
    /// From the tracker's controller type (`vive_tracker_waist` and so on).
    fn of(kind: &str) -> Self {
        let has = |word: &str| kind.contains(word);
        match () {
            () if has("waist") => Self::Waist,
            () if has("chest") => Self::Chest,
            () if has("left_foot") => Self::LeftFoot,
            () if has("right_foot") => Self::RightFoot,
            () if has("left_knee") => Self::LeftKnee,
            () if has("right_knee") => Self::RightKnee,
            () if has("left_elbow") => Self::LeftElbow,
            () if has("right_elbow") => Self::RightElbow,
            () if has("left_shoulder") => Self::LeftShoulder,
            () if has("right_shoulder") => Self::RightShoulder,
            () => Self::Other,
        }
    }
}

/// A tracker on the body as the scene draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TrackerView {
    part: BodyPart,
    device: Option<u32>,
    pose: M34,
}

/// What the thread that follows the devices leaves for the one that draws.
#[derive(Clone, Debug, Default)]
struct Views {
    hands: [HandView; 2],
    trackers: Vec<TrackerView>,
}

static VIEWS: Mutex<Option<Views>> = Mutex::new(None);

/// The player's body as the headset's devices have it: the head, the two
/// hands, and every tracker worn (waist, feet, knees, elbows), each where it
/// is in the room, in SteamVR's standing space (metres; +y up, -z forward).
/// For what turns it into the character's pose one day.
#[derive(Clone, Debug, Default)]
pub struct Body {
    pub head: Option<[[f32; 4]; 3]>,
    pub left_hand: Option<[[f32; 4]; 3]>,
    pub right_hand: Option<[[f32; 4]; 3]>,
    pub trackers: Vec<(BodyPart, [[f32; 4]; 3])>,
}

static BODY: Mutex<Option<Body>> = Mutex::new(None);

/// The player's body now; `None` without the headset mode.
pub fn body() -> Option<Body> {
    if !on() {
        return None;
    }
    BODY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The title's pad as the headset's controllers hold it: buttons as the
/// bits below, triggers 0 to 1, sticks -1 to 1 (up positive).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pad {
    pub buttons: u32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
}

impl Pad {
    pub const A: u32 = 1;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const MENU: u32 = 1 << 4;
    pub const VIEW: u32 = 1 << 5;
    pub const LEFT_SHOULDER: u32 = 1 << 6;
    pub const RIGHT_SHOULDER: u32 = 1 << 7;
    pub const LEFT_STICK: u32 = 1 << 8;
    pub const RIGHT_STICK: u32 = 1 << 9;
}

static PAD: Mutex<Option<Pad>> = Mutex::new(None);

/// What the headset's controllers hold of the title's pad now; `None`
/// without the headset mode or before it has started.
pub fn pad() -> Option<Pad> {
    if !on() {
        return None;
    }
    *PAD.lock().unwrap_or_else(|e| e.into_inner())
}

/// What one controller says, a real one or an emulated one.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HandIn {
    present: bool,
    /// SteamVR's number for the controller; `None` for an emulated one.
    device: Option<u32>,
    /// Where it is in the room.
    pose: M34,
    /// How fast it moves, metres a second, if the source says.
    velocity: Option<[f32; 3]>,
    grip: f32,
    trigger: f32,
    stick: [f32; 2],
    a: bool,
    b: bool,
    stick_click: bool,
}

impl Default for HandIn {
    fn default() -> Self {
        Self {
            present: false,
            device: None,
            pose: math::IDENTITY,
            velocity: None,
            grip: 0.0,
            trigger: 0.0,
            stick: [0.0; 2],
            a: false,
            b: false,
            stick_click: false,
        }
    }
}

/// The folder of the installed OpenVR runtime: `VICTORY_VR_RUNTIME`, or the
/// first one `openvrpaths.vrpath` names.
fn runtime_folder() -> Option<std::path::PathBuf> {
    if let Some(folder) = std::env::var_os("VICTORY_VR_RUNTIME") {
        return Some(folder.into());
    }
    let paths = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("openvr").join("openvrpaths.vrpath");
    let text = std::fs::read_to_string(paths).ok()?;
    first_runtime(&text).map(Into::into)
}

/// The first string of the `"runtime"` list in `openvrpaths.vrpath` (JSON).
fn first_runtime(text: &str) -> Option<String> {
    let rest = &text[text.find("\"runtime\"")? + 9..];
    let rest = &rest[rest.find('[')? + 1..];
    let rest = &rest[rest.find('"')? + 1..];
    let mut out = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
}

/// SteamVR's function tables.
struct Api {
    system: usize,
    compositor: usize,
    models: usize,
}

impl Api {
    /// Entry `i` of the system table as a function of type `F`.
    ///
    /// # Safety
    /// `F` must be the type of entry `i` of `IVRSystem_026`'s table.
    unsafe fn system<F: Copy>(&self, i: usize) -> F {
        debug_assert!(i < system::ENTRIES && size_of::<F>() == size_of::<usize>());
        // SAFETY: as above.
        unsafe { std::mem::transmute_copy(&*(self.system as *const usize).add(i)) }
    }

    /// Entry `i` of the compositor table as a function of type `F`.
    ///
    /// # Safety
    /// `F` must be the type of entry `i` of `IVRCompositor_029`'s table.
    unsafe fn compositor<F: Copy>(&self, i: usize) -> F {
        debug_assert!(i < compositor::ENTRIES && size_of::<F>() == size_of::<usize>());
        // SAFETY: as above.
        unsafe { std::mem::transmute_copy(&*(self.compositor as *const usize).add(i)) }
    }

    /// Entry `i` of the render models' table as a function of type `F`.
    ///
    /// # Safety
    /// `F` must be the type of entry `i` of `IVRRenderModels_006`'s table.
    unsafe fn models<F: Copy>(&self, i: usize) -> F {
        debug_assert!(i < models::ENTRIES && size_of::<F>() == size_of::<usize>());
        // SAFETY: as above.
        unsafe { std::mem::transmute_copy(&*(self.models as *const usize).add(i)) }
    }
}

/// SteamVR as far as it could be reached: asked for once, and with it the
/// threads that follow the head and the hands and draw the headset's picture
/// are started.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    static STARTED: std::sync::Once = std::sync::Once::new();
    let api = API
        .get_or_init(|| match open() {
            Ok(api) => Some(api),
            Err(why) => {
                tracing::warn!("no headset mode (VICTORY_VR=1): {why}");
                None
            }
        })
        .as_ref()?;
    STARTED.call_once(|| {
        if let Err(e) = std::thread::Builder::new().name("victory-vr".into()).spawn(move || track::run(api)) {
            tracing::warn!(error = %e, "the headset mode's thread did not start: no hands, no controllers");
        }
        match crate::host::host() {
            Some(host) => {
                if let Err(e) = std::thread::Builder::new().name("victory-vr-scene".into()).spawn(move || scene::run(api, host)) {
                    tracing::warn!(error = %e, "the headset mode's drawing thread did not start: nothing in the headset");
                }
            }
            None => tracing::warn!("the headset mode: no host device yet, nothing in the headset"),
        }
    });
    Some(api)
}

fn open() -> Result<Api, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };
    let folder = runtime_folder().ok_or("no OpenVR runtime is installed (no openvrpaths.vrpath; VICTORY_VR_RUNTIME names its folder)")?;
    let path = std::path::absolute(folder.join("bin").join("win64").join("openvr_api.dll")).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err(format!("no {}", path.display()));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: a NUL-terminated full path to the installed runtime's library;
    // its own folder and System32 only.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32) };
    if module.is_null() {
        return Err(format!("{} did not load ({})", path.display(), std::io::Error::last_os_error()));
    }
    let export = |name: &CStr| {
        // SAFETY: a loaded module and a NUL-terminated name.
        unsafe { GetProcAddress(module, name.as_ptr().cast()) }.ok_or_else(|| format!("the runtime's library has no {}", name.to_string_lossy()))
    };
    // SAFETY: the library's exports, with the types `openvr_capi.h` gives them.
    #[allow(clippy::missing_transmute_annotations)]
    let (init, interface, text): (InitInternal, GetGenericInterface, ErrorText) = unsafe {
        (
            std::mem::transmute::<_, InitInternal>(export(c"VR_InitInternal2")?),
            std::mem::transmute::<_, GetGenericInterface>(export(c"VR_GetGenericInterface")?),
            std::mem::transmute::<_, ErrorText>(export(c"VR_GetVRInitErrorAsEnglishDescription")?),
        )
    };
    let said = |error: i32| {
        // SAFETY: a static string of the library, or null.
        let text = unsafe { text(error) };
        if text.is_null() { format!("error {error}") } else { format!("{} ({error})", unsafe { CStr::from_ptr(text) }.to_string_lossy()) }
    };
    let mut error = 0i32;
    // SAFETY: the library's own start, as a scene application.
    unsafe { init(&mut error, APPLICATION_SCENE, std::ptr::null()) };
    if error != 0 {
        return Err(format!("SteamVR did not start for us: {}", said(error)));
    }
    let mut table = |name: &CStr| {
        // SAFETY: a NUL-terminated interface name.
        let table = unsafe { interface(name.as_ptr(), &mut error) };
        if table == 0 || error != 0 { Err(format!("the runtime has no {}: {}", name.to_string_lossy(), said(error))) } else { Ok(table as usize) }
    };
    let api = Api { system: table(SYSTEM_INTERFACE)?, compositor: table(COMPOSITOR_INTERFACE)?, models: table(MODELS_INTERFACE)? };
    tracing::info!(runtime = %folder.display(), mode = ?mode(), "the headset mode: SteamVR is reached (VICTORY_VR=1)");
    Ok(api)
}

/// The kind of a depth buffer, as copied into a snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DepthKind {
    Float32,
    Unorm24,
    Unorm16,
}

impl DepthKind {
    /// Of a host depth texture's format; `None` for one that cannot be
    /// copied into a snapshot.
    fn of(format: DXGI_FORMAT) -> Option<Self> {
        Some(match format.0 {
            // R32 typeless, D32 float, R32 float; R32G8X24 typeless, D32
            // float S8X24, R32 float X8X24 (its depth plane).
            39..=41 | 19..=21 => Self::Float32,
            // R24G8 typeless, D24 unorm S8, R24 unorm X8 (its depth plane).
            44..=46 => Self::Unorm24,
            // R16 typeless, D16 unorm, R16 unorm.
            53 | 55 | 56 => Self::Unorm16,
            _ => return None,
        })
    }

    fn snapshot_format(self) -> DXGI_FORMAT {
        match self {
            Self::Unorm16 => DXGI_FORMAT_R16_TYPELESS,
            _ => DXGI_FORMAT_R32_TYPELESS,
        }
    }

    fn float_view(self) -> DXGI_FORMAT {
        match self {
            Self::Unorm16 => DXGI_FORMAT_R16_UNORM,
            _ => DXGI_FORMAT_R32_FLOAT,
        }
    }

    fn uint_view(self) -> DXGI_FORMAT {
        match self {
            Self::Unorm16 => DXGI_FORMAT_R16_UINT,
            _ => DXGI_FORMAT_R32_UINT,
        }
    }

    /// As the shader tells them apart.
    fn number(self) -> f32 {
        match self {
            Self::Float32 => 1.0,
            Self::Unorm24 => 2.0,
            Self::Unorm16 => 3.0,
        }
    }
}

/// The depth buffers the title bound since the last present, and how often
/// ([`depth_bound`]): a small open table, with no lock.
const DEPTHS: usize = 32;
static DEPTH_SEEN: [AtomicUsize; DEPTHS] = [const { AtomicUsize::new(0) }; DEPTHS];
static DEPTH_COUNT: [AtomicU32; DEPTHS] = [const { AtomicU32::new(0) }; DEPTHS];

/// The title bound the depth buffer `resource` (its object) for drawing.
pub(crate) fn depth_bound(resource: usize) {
    if !on() || resource == 0 {
        return;
    }
    let start = (resource >> 4) % DEPTHS;
    for k in 0..8 {
        let i = (start + k) % DEPTHS;
        let seen = DEPTH_SEEN[i].load(Relaxed);
        if seen == resource {
            DEPTH_COUNT[i].fetch_add(1, Relaxed);
            return;
        }
        if seen == 0 {
            match DEPTH_SEEN[i].compare_exchange(0, resource, Relaxed, Relaxed) {
                Ok(_) => {
                    DEPTH_COUNT[i].fetch_add(1, Relaxed);
                    return;
                }
                Err(now) if now == resource => {
                    DEPTH_COUNT[i].fetch_add(1, Relaxed);
                    return;
                }
                Err(_) => {}
            }
        }
    }
}

/// The depth buffer the title draws its frames with: the one bound the
/// most over the last few presents, of the picture's shape (not a shadow
/// map's square).
///
/// Counted over several presents and not for each: the title's threads
/// record a frame while the one before it is being submitted, so the binds
/// between two presents are not one frame's (counted for each present, some
/// frames had none and were shown flat). What each buffer was bound for is
/// halved at every present; one no frame draws with any more is forgotten
/// within some ten of them. The buffer holds the presented frame's depth
/// when our copy runs: the card runs submissions in their order, and the
/// next frame's come behind the present.
fn main_depth() -> Option<(std::sync::Arc<crate::resource::HostTexture>, DepthKind, (u32, u32))> {
    let mut best: Option<(u32, std::sync::Arc<crate::resource::HostTexture>, DepthKind, (u32, u32))> = None;
    for i in 0..DEPTHS {
        let resource = DEPTH_SEEN[i].load(Relaxed);
        if resource == 0 {
            continue;
        }
        let count = DEPTH_COUNT[i].load(Relaxed);
        if count == 0 {
            // Its place goes to others.
            let _ = DEPTH_SEEN[i].compare_exchange(resource, 0, Relaxed, Relaxed);
            continue;
        }
        DEPTH_COUNT[i].fetch_sub(count - count / 2, Relaxed);
        if best.as_ref().is_some_and(|b| b.0 >= count) {
            continue;
        }
        let Some(copy) = crate::resource::host_copy(resource as crate::object::This) else {
            continue;
        };
        // SAFETY: a live resource.
        let desc = unsafe { copy.texture.GetDesc() };
        let (w, h) = (desc.Width as u32, desc.Height);
        let aspect = w as f32 / h.max(1) as f32;
        if w < 640 || !(1.5..=2.5).contains(&aspect) || desc.SampleDesc.Count != 1 {
            continue;
        }
        if let Some(kind) = DepthKind::of(desc.Format) {
            best = Some((count, copy, kind, (w, h)));
        }
    }
    best.map(|(_, copy, kind, size)| (copy, kind, size))
}

/// One snapshot of a frame for the headset.
#[derive(Default)]
struct Slot {
    colour: Option<(ID3D12Resource, (u32, u32), DXGI_FORMAT)>,
    depth: Option<(ID3D12Resource, DepthKind, (u32, u32))>,
    /// Whether this frame's depth was copied (the depth texture is kept
    /// when a frame has none).
    has_depth: bool,
    /// The frame's number: the title's queue has passed its copy when our
    /// fence on it reaches this.
    serial: u64,
    /// Our drawing fence's value at which we are done reading it.
    busy_until: u64,
}

#[derive(Default)]
struct Snaps {
    slots: [Slot; 3],
    /// The last frame whose copy went to the title's queue.
    serial: u64,
    /// A frame copied by a list not yet sent ([`show`] sends its signal).
    pending: Option<u64>,
    /// Signalled on the title's queue behind each copy.
    title_fence: Option<ID3D12Fence>,
    /// Our drawing queue's fence.
    our_fence: Option<ID3D12Fence>,
    skipped: u64,
    depth_said: Option<(DepthKind, (u32, u32))>,
}

// SAFETY: free-threaded D3D12 objects.
unsafe impl Send for Snaps {}

static SNAPS: Mutex<Option<Snaps>> = Mutex::new(None);

/// A snapshot as the drawing thread uses it.
struct Snap {
    colour: ID3D12Resource,
    colour_format: DXGI_FORMAT,
    depth: Option<(ID3D12Resource, DepthKind)>,
    aspect: f32,
}

/// For our frame that will signal `value` on `fence`: the newest snapshot the
/// title's queue has finished, kept from the title until that frame is done.
fn snap_for_drawing(value: u64, fence: &ID3D12Fence) -> Option<Snap> {
    let mut snaps = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
    let snaps = snaps.get_or_insert_with(Snaps::default);
    if snaps.our_fence.is_none() {
        snaps.our_fence = Some(fence.clone());
    }
    // SAFETY: a plain query of our fence.
    let done = unsafe { snaps.title_fence.as_ref()?.GetCompletedValue() }.min(snaps.serial);
    if done == 0 {
        return None;
    }
    let slot = &mut snaps.slots[(done % 3) as usize];
    if slot.serial != done {
        return None;
    }
    slot.busy_until = value;
    let (colour, size, colour_format) = slot.colour.clone()?;
    Some(Snap {
        colour,
        colour_format,
        depth: slot.has_depth.then(|| slot.depth.as_ref().map(|(d, kind, _)| (d.clone(), *kind))).flatten(),
        aspect: size.0 as f32 / size.1.max(1) as f32,
    })
}

fn texture(host: &Host, size: (u32, u32), format: DXGI_FORMAT, state: D3D12_RESOURCE_STATES) -> Option<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(size.0),
        Height: size.1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: plain creation of a texture of ours.
    let result = unsafe { host.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut made) };
    if let Err(e) = result {
        tracing::warn!(error = %e, ?size, format = format.0, "the headset mode: no texture for a snapshot");
    }
    made
}

/// Records into `list` the copy of the back buffer `back` (in the copy
/// destination state, as the present path has it there), and of the frame's
/// depth buffer, into the next snapshot ([`Snaps`]).
pub(crate) fn frame(host: &Host, list: &ID3D12GraphicsCommandList, back: &ID3D12Resource, size: (u32, u32), format: DXGI_FORMAT) {
    if !on() || api().is_none() {
        return;
    }
    const COPY_SOURCE: u32 = 0x800;
    let depth = main_depth();
    let mut snaps = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
    let snaps = snaps.get_or_insert_with(Snaps::default);
    if snaps.title_fence.is_none() {
        // SAFETY: plain creation of a fence of ours.
        snaps.title_fence = unsafe { host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok();
    }
    let serial = snaps.serial + 1;
    // SAFETY: a plain query of our drawing fence.
    let ours = snaps.our_fence.as_ref().map_or(u64::MAX, |f| unsafe { f.GetCompletedValue() });
    let slot = &mut snaps.slots[(serial % 3) as usize];
    if ours < slot.busy_until {
        // Still being read for the headset: this frame is not taken.
        snaps.skipped += 1;
        return;
    }
    if slot.colour.as_ref().is_none_or(|c| (c.1, c.2) != (size, format)) {
        slot.colour = texture(host, size, format, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE).map(|t| (t, size, format));
    }
    let Some((colour, ..)) = slot.colour.clone() else {
        return;
    };
    slot.has_depth = false;
    if let Some((_, kind, depth_size)) = &depth {
        if slot.depth.as_ref().is_none_or(|d| (d.1, d.2) != (*kind, *depth_size)) {
            slot.depth = texture(host, *depth_size, kind.snapshot_format(), D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE).map(|t| (t, *kind, *depth_size));
        }
        slot.has_depth = slot.depth.is_some();
    }
    // SAFETY: an open list; the back buffer in the state the caller says,
    // our textures in the states they rest in, the title's depth texture in
    // the state its last submitted list left it in, and put back there.
    unsafe {
        list.ResourceBarrier(&[
            transition(back, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE),
            transition(&colour, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST),
        ]);
        list.CopyResource(&colour, back);
        list.ResourceBarrier(&[
            transition(&colour, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE),
            transition(back, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST),
        ]);
        if let (true, Some((copy, ..)), Some((ours, ..))) = (slot.has_depth, &depth, &slot.depth) {
            let real = copy.state.load(Relaxed);
            let state = |s: u32| D3D12_RESOURCE_STATES(s as i32);
            if real != COPY_SOURCE {
                list.ResourceBarrier(&[transition(&copy.texture, state(real), D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            }
            list.ResourceBarrier(&[transition(ours, D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST)]);
            let at = |resource: &ID3D12Resource| D3D12_TEXTURE_COPY_LOCATION {
                // SAFETY: a borrowed pointer the location never releases.
                pResource: std::mem::transmute_copy(resource),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            list.CopyTextureRegion(&at(ours), 0, 0, 0, &at(&copy.texture), None);
            list.ResourceBarrier(&[transition(ours, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE)]);
            if real != COPY_SOURCE {
                list.ResourceBarrier(&[transition(&copy.texture, D3D12_RESOURCE_STATE_COPY_SOURCE, state(real))]);
            }
        }
    }
    slot.serial = serial;
    snaps.pending = Some(serial);
    let now = depth.as_ref().map(|(_, kind, size)| (*kind, *size));
    if now != snaps.depth_said {
        snaps.depth_said = now;
        tracing::info!(depth = ?now, colour = ?size, "the headset mode: the frame's depth for the relief");
    }
}

/// After the list with [`frame`]'s copy went to the title's queue: the
/// snapshot is told done behind it.
pub(crate) fn show(host: &Host) {
    let mut snaps = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(snaps) = snaps.as_mut() else {
        return;
    };
    let Some(serial) = snaps.pending.take() else {
        return;
    };
    if let Some(fence) = &snaps.title_fence {
        // SAFETY: our fence on the title's queue.
        if unsafe { host.queue().Signal(fence, serial) }.is_ok() {
            snaps.serial = serial;
        }
    }
    if serial.is_power_of_two() && serial >= 64 {
        tracing::info!(frames = serial, skipped = snaps.skipped, "the headset mode: frames taken for the headset");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_runtime_folder_is_read_from_the_paths_file() {
        let text = "{\n\t\"config\" : \n\t[\n\t\t\"C:\\\\Users\\\\x\\\\AppData\\\\Local\\\\openvr\"\n\t],\n\t\"runtime\" : \n\t[\n\t\t\"C:\\\\Program Files (x86)\\\\Steam\\\\steamapps\\\\common\\\\SteamVR\"\n\t],\n\t\"version\" : 1\n}";
        assert_eq!(super::first_runtime(text).as_deref(), Some("C:\\Program Files (x86)\\Steam\\steamapps\\common\\SteamVR"));
        assert_eq!(super::first_runtime("{}"), None);
    }

    #[test]
    fn depth_formats_are_told_apart() {
        use super::DepthKind;
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
        assert_eq!(DepthKind::of(DXGI_FORMAT(40)), Some(DepthKind::Float32));
        assert_eq!(DepthKind::of(DXGI_FORMAT(20)), Some(DepthKind::Float32));
        assert_eq!(DepthKind::of(DXGI_FORMAT(45)), Some(DepthKind::Unorm24));
        assert_eq!(DepthKind::of(DXGI_FORMAT(55)), Some(DepthKind::Unorm16));
        assert_eq!(DepthKind::of(DXGI_FORMAT(28)), None);
    }
}
