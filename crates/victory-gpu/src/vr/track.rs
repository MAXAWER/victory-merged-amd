//! The thread of the headset mode ("victory-vr"): some ninety times a second
//! it asks where the head and the controllers are and what their buttons
//! say, moves the hands' pieces and the screen, and leaves the title's pad
//! as the controllers hold it for `victory_hle`'s input to take
//! ([`super::pad`]).
//!
//! The controllers as the pad (SteamVR's legacy controller state: an
//! analogue stick or pad, a trigger, a grip, two face buttons):
//!
//! | controller | stick | trigger | lower button | upper button | stick pressed |
//! | --- | --- | --- | --- | --- | --- |
//! | left | left stick | left trigger | X | Y | left stick button |
//! | right | right stick | right trigger | A | B | right stick button |
//!
//! Both sticks pressed together are the menu button. The grips close the
//! hands and press nothing by themselves: a closed hand thrown forward
//! faster than `VICTORY_VR_STRIKE_SPEED` metres a second (1.4) presses the
//! title's strike button for a moment (`VICTORY_VR_STRIKE`: `a`, `b`, `x`,
//! `y`, `lb`, `rb`, `lt`, `rt`; `b` unless said otherwise) and the
//! controller ticks.
//!
//! In `immersive` the head's turning is added to the right stick: a full
//! push for `VICTORY_VR_LOOK` degrees a second (160).

use std::time::{Duration, Instant};

use super::hands::{self, INDEX, OTHERS, PIECES, THUMB};
use super::math::{IDENTITY, M34, V3, dot, from_axes, level_forward, mul, pitch, place, scale, sub};
use super::{
    Api, HandIn, Handle, Mode, NO_BACKSIDE, Pad, SetColor, SetFlag, SetFloat, SetOrder, SetRaw, SetTransformAbsolute, SetTransformRelative,
    UNIVERSE_STANDING, emu, number, overlay, system,
};

/// `TrackedDevicePose_t`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Pose {
    at: M34,
    velocity: V3,
    angular: V3,
    result: i32,
    valid: bool,
    connected: bool,
}

/// `VRControllerState_t` (Windows: the natural layout).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ControllerState {
    packet: u32,
    pressed: u64,
    touched: u64,
    axes: [[f32; 2]; 5],
}

const _: () = assert!(size_of::<Pose>() == 80 && size_of::<ControllerState>() == 64);

type Poses = unsafe extern "system" fn(i32, f32, *mut Pose, u32);
type IndexForRole = unsafe extern "system" fn(i32) -> u32;
type GetState = unsafe extern "system" fn(u32, *mut ControllerState, u32) -> bool;
type Haptic = unsafe extern "system" fn(u32, u32, u16);

const DEVICES: usize = 64;
const HEAD: usize = 0;
/// `ETrackedControllerRole`: left, right.
const ROLES: [i32; 2] = [1, 2];
/// `EVRButtonId` bits of the legacy state.
const BUTTON_UPPER: u64 = 1 << 1;
const BUTTON_GRIP: u64 = 1 << 2;
const BUTTON_LOWER: u64 = 1 << 7;
const BUTTON_STICK: u64 = 1 << 32;

const TICK: Duration = Duration::from_millis(11);
const DEAD_ZONE: f32 = 0.12;
/// How long a strike holds the button, and how soon the same hand may
/// strike again.
const STRIKE_HELD: Duration = Duration::from_millis(120);
const STRIKE_AGAIN: Duration = Duration::from_millis(400);

/// What a strike presses.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Strike {
    Button(u32),
    LeftTrigger,
    RightTrigger,
}

fn strike_button() -> Strike {
    match std::env::var("VICTORY_VR_STRIKE").unwrap_or_default().to_ascii_lowercase().as_str() {
        "a" => Strike::Button(Pad::A),
        "x" => Strike::Button(Pad::X),
        "y" => Strike::Button(Pad::Y),
        "lb" => Strike::Button(Pad::LEFT_SHOULDER),
        "rb" => Strike::Button(Pad::RIGHT_SHOULDER),
        "lt" => Strike::LeftTrigger,
        "rt" => Strike::RightTrigger,
        _ => Strike::Button(Pad::B),
    }
}

/// A key of the keyboard is down now.
fn key(virtual_key: i32) -> bool {
    // SAFETY: a plain query of the keyboard.
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(virtual_key) as u16 & 0x8000 != 0 }
}

/// One hand as it is shown.
struct Shown {
    right: bool,
    /// The pieces' overlays, front sides then back sides if there are any.
    handles: Vec<u64>,
    visible: bool,
    curl: [f32; 3],
    /// What the pieces were last placed for, with a real controller.
    placed: Option<(u32, [f32; 3])>,
    /// For the speed of an emulated hand, and for strikes.
    before: Option<(V3, Instant)>,
    strike_until: Option<Instant>,
    strike_again: Instant,
}

impl Shown {
    fn new(right: bool, now: Instant) -> Self {
        Self { right, handles: Vec::new(), visible: false, curl: [0.0; 3], placed: None, before: None, strike_until: None, strike_again: now }
    }

    /// Makes the pieces' overlays the first time the hand is there.
    fn make(&mut self, api: &Api, sides: usize) {
        if !self.handles.is_empty() {
            return;
        }
        let pieces = hands::pieces(self.right, [0.0; 3]);
        for side in 0..sides {
            for (i, piece) in pieces.iter().enumerate() {
                let key = std::ffi::CString::new(format!("victory.hand.{}.{side}.{i}", if self.right { "right" } else { "left" })).unwrap_or_default();
                let Ok(handle) = api.overlay_named(&key, c"VIctory hand") else {
                    continue;
                };
                let (texels, width, height) = hands::picture(piece.size);
                // The palm a shade darker than the fingers.
                let tone = if i == 0 { [0.80, 0.60, 0.48] } else { [0.90, 0.70, 0.56] };
                // SAFETY: the table's entries by their types; the texels
                // live through the call, which copies them.
                unsafe {
                    api.overlay::<SetRaw>(overlay::SET_RAW)(handle, texels.as_ptr().cast(), width, height, 4);
                    api.overlay::<SetFloat>(overlay::SET_WIDTH)(handle, piece.size[0]);
                    api.overlay::<SetColor>(overlay::SET_COLOR)(handle, tone[0], tone[1], tone[2]);
                    api.overlay::<SetFlag>(overlay::SET_FLAG)(handle, NO_BACKSIDE, true);
                    // Over the screen; the palm under the fingers.
                    api.overlay::<SetOrder>(overlay::SET_SORT_ORDER)(handle, if i == 0 { 1 } else { 2 });
                }
                self.handles.push(handle);
            }
        }
        tracing::info!(right = self.right, pieces = self.handles.len(), "the headset mode: a hand's pieces are made");
    }

    fn show(&mut self, api: &Api, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.placed = None;
        let entry = if visible { overlay::SHOW } else { overlay::HIDE };
        for &handle in &self.handles {
            // SAFETY: the table's entry by its type.
            unsafe { api.overlay::<Handle>(entry)(handle) };
        }
    }

    /// Places the pieces for the controller `hand`.
    fn place(&mut self, api: &Api, hand: &HandIn) {
        // With a real controller SteamVR moves the pieces itself: they are
        // told their place on it only when the fingers have moved.
        let curl = self.curl.map(|c| (c * 50.0).round() / 50.0);
        if let Some(device) = hand.device {
            if self.placed == Some((device, curl)) {
                return;
            }
            self.placed = Some((device, curl));
        }
        let pieces = hands::pieces(self.right, curl);
        // The back side of a piece: the same place, turned about its length.
        let back = from_axes([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [0.0; 3]);
        for (n, &handle) in self.handles.iter().enumerate() {
            let piece = &pieces[n % PIECES];
            let at = if n < PIECES { piece.at } else { mul(&piece.at, &back) };
            // SAFETY: the table's entries by their types; the matrix lives
            // through the call.
            unsafe {
                match hand.device {
                    Some(device) => api.overlay::<SetTransformRelative>(overlay::SET_TRANSFORM_RELATIVE)(handle, device, &at),
                    None => api.overlay::<SetTransformAbsolute>(overlay::SET_TRANSFORM_ABSOLUTE)(handle, UNIVERSE_STANDING, &mul(&hand.pose, &at)),
                };
            }
        }
    }
}

/// The real controllers, as SteamVR has them now.
fn controllers(api: &Api, poses: &[Pose; DEVICES]) -> [HandIn; 2] {
    ROLES.map(|role| {
        // SAFETY: the table's entries by their types; the state lives
        // through the call and is of the size said.
        unsafe {
            let device = api.system::<IndexForRole>(system::INDEX_FOR_ROLE)(role);
            let Some(pose) = poses.get(device as usize).filter(|p| p.valid && p.connected) else {
                return HandIn::default();
            };
            let mut state = ControllerState::default();
            if !api.system::<GetState>(system::CONTROLLER_STATE)(device, &mut state, size_of::<ControllerState>() as u32) {
                // Tracked, but its buttons are not ours to read now.
                return HandIn { present: true, device: Some(device), pose: pose.at, velocity: Some(pose.velocity), ..HandIn::default() };
            }
            let grip = if state.pressed & BUTTON_GRIP != 0 { 1.0 } else { state.axes[2][0].clamp(0.0, 1.0) };
            HandIn {
                present: true,
                device: Some(device),
                pose: pose.at,
                velocity: Some(pose.velocity),
                grip,
                trigger: state.axes[1][0].clamp(0.0, 1.0),
                stick: state.axes[0],
                a: state.pressed & BUTTON_LOWER != 0,
                b: state.pressed & BUTTON_UPPER != 0,
                stick_click: state.pressed & BUTTON_STICK != 0,
            }
        }
    })
}

fn dead(v: f32) -> f32 {
    if v.abs() < DEAD_ZONE { 0.0 } else { v.clamp(-1.0, 1.0) }
}

/// The pad as the two controllers (left, right) hold it, without strikes
/// and without the head.
fn pad_of(hands: &[HandIn; 2]) -> Pad {
    let [left, right] = hands;
    let mut buttons = 0;
    for (on, bit) in [(left.a, Pad::X), (left.b, Pad::Y), (right.a, Pad::A), (right.b, Pad::B)] {
        if on {
            buttons |= bit;
        }
    }
    match (left.stick_click, right.stick_click) {
        (true, true) => buttons |= Pad::MENU,
        (true, false) => buttons |= Pad::LEFT_STICK,
        (false, true) => buttons |= Pad::RIGHT_STICK,
        (false, false) => {}
    }
    Pad {
        buttons,
        left_trigger: left.trigger,
        right_trigger: right.trigger,
        left_x: dead(left.stick[0]),
        left_y: dead(left.stick[1]),
        right_x: dead(right.stick[0]),
        right_y: dead(right.stick[1]),
    }
}

/// The thread's body: never returns.
pub(super) fn run(api: &'static Api) {
    let emulated = std::env::var_os("VICTORY_VR_EMU").is_some_and(|v| v != "0");
    let sides = if std::env::var_os("VICTORY_VR_HAND_SIDES").is_some_and(|v| v == "1") { 1 } else { 2 };
    let strike_speed = number("VICTORY_VR_STRIKE_SPEED", 1.4).max(0.2);
    let look = number("VICTORY_VR_LOOK", 160.0).max(10.0);
    let strike = strike_button();
    let began = Instant::now();
    let mut shown = [Shown::new(false, began), Shown::new(true, began)];
    let mut head = IDENTITY;
    let mut head_seen = false;
    let mut keys = [false; 4];
    let mut turned: Option<(f32, f32)> = None;
    let mut rates = [0.0f32; 2];
    let mut strikes = 0u64;
    let mut said = began;
    let mut last = began;
    tracing::info!(emulated, mode = ?super::mode(), ?strike, strike_speed, look, "the headset mode: following the head and the hands");
    loop {
        std::thread::sleep(TICK);
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().max(1e-3);
        last = now;
        // SAFETY: plain data the call fills, of the count said.
        let mut poses: [Pose; DEVICES] = unsafe { std::mem::zeroed() };
        // SAFETY: the table's entry by its type; the array lives through
        // the call.
        unsafe { api.system::<Poses>(system::POSES)(UNIVERSE_STANDING, 0.0, poses.as_mut_ptr(), DEVICES as u32) };
        if poses[HEAD].valid {
            head = poses[HEAD].at;
            if !head_seen {
                head_seen = true;
                super::RECENTRE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let hands_in = if emulated { emu::hands((now - began).as_secs_f32(), &head) } else { controllers(api, &poses) };
        let (forward, yaw) = level_forward(&head);

        // The keys: F6 the next way of looking, F7 the view centred again,
        // F8 and F9 the relief smaller and larger.
        let down = [key(0x75), key(0x76), key(0x77), key(0x78)];
        let pressed = |k: usize| down[k] && !keys[k];
        if pressed(0) {
            let mode = super::next_mode();
            tracing::info!(?mode, "the headset mode: the way of looking changed (F6)");
        }
        if pressed(1) {
            super::RECENTRE.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if pressed(2) || pressed(3) {
            let scale = super::scale_world(if pressed(2) { 0.8 } else { 1.25 });
            tracing::info!(scale, "the headset mode: the relief's scale (F8, F9)");
        }
        keys = down;
        let mode = super::mode();

        // The hands, and what they strike.
        let mut pad = pad_of(&hands_in);
        for (hand, shown) in hands_in.iter().zip(&mut shown) {
            if !hand.present {
                shown.show(api, false);
                shown.before = None;
                continue;
            }
            shown.make(api, sides);
            let wanted = {
                let mut c = [0.0; 3];
                c[THUMB] = hand.grip;
                c[INDEX] = hand.grip.max(hand.trigger);
                c[OTHERS] = hand.grip;
                c
            };
            for (have, want) in shown.curl.iter_mut().zip(wanted) {
                *have += (want - *have) * (dt * 22.0).min(1.0);
            }
            shown.place(api, hand);
            shown.show(api, true);
            let at = place(&hand.pose);
            let velocity = hand.velocity.or_else(|| shown.before.map(|(was, when)| scale(sub(at, was), 1.0 / (now - when).as_secs_f32().max(1e-3))));
            shown.before = Some((at, now));
            let thrown = velocity.map_or(0.0, |v| dot(v, forward));
            if hand.grip > 0.7 && thrown > strike_speed && now >= shown.strike_again {
                shown.strike_until = Some(now + STRIKE_HELD);
                shown.strike_again = now + STRIKE_AGAIN;
                strikes += 1;
                if strikes <= 12 || strikes.is_power_of_two() {
                    tracing::info!(right = shown.right, speed = thrown, strikes, ?strike, "the headset mode: a strike");
                }
                if let Some(device) = hand.device {
                    // SAFETY: the table's entry by its type.
                    unsafe { api.system::<Haptic>(system::HAPTIC_PULSE)(device, 0, 3000) };
                }
            }
            if shown.strike_until.is_some_and(|until| now < until) {
                match strike {
                    Strike::Button(bit) => pad.buttons |= bit,
                    Strike::LeftTrigger => pad.left_trigger = 1.0,
                    Strike::RightTrigger => pad.right_trigger = 1.0,
                }
            }
        }

        // The head turns the camera where the picture is fixed to it.
        let facing = (yaw, pitch(&head));
        if mode == Mode::Immersive && head_seen {
            if let Some((yaw_before, pitch_before)) = turned {
                let mut by = facing.0 - yaw_before;
                if by > std::f32::consts::PI {
                    by -= std::f32::consts::TAU;
                } else if by < -std::f32::consts::PI {
                    by += std::f32::consts::TAU;
                }
                let now_rates = [by.to_degrees() / dt, (facing.1 - pitch_before).to_degrees() / dt];
                for (rate, new) in rates.iter_mut().zip(now_rates) {
                    *rate += (new - *rate) * 0.5;
                }
                // Turning left is a stick pushed left.
                pad.right_x = (pad.right_x - rates[0] / look).clamp(-1.0, 1.0);
                pad.right_y = (pad.right_y + rates[1] / look).clamp(-1.0, 1.0);
            }
        } else {
            rates = [0.0; 2];
        }
        turned = Some(facing);

        *super::PAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(pad);

        if now - said >= Duration::from_secs(5) {
            said = now;
            tracing::info!(
                ?mode,
                head_seen,
                head = ?place(&head).map(|v| (v * 100.0).round() / 100.0),
                left = ?hands_in[0].present.then(|| place(&hands_in[0].pose).map(|v| (v * 100.0).round() / 100.0)),
                right = ?hands_in[1].present.then(|| place(&hands_in[1].pose).map(|v| (v * 100.0).round() / 100.0)),
                curl_left = ?shown[0].curl.map(|v| (v * 100.0).round() / 100.0),
                curl_right = ?shown[1].curl.map(|v| (v * 100.0).round() / 100.0),
                ?pad,
                strikes,
                "the headset mode: where things are"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controllers_hold_the_pad() {
        let left = HandIn { present: true, stick: [0.05, 0.9], trigger: 0.4, a: true, stick_click: true, ..HandIn::default() };
        let right = HandIn { present: true, stick: [-0.6, 0.0], b: true, ..HandIn::default() };
        let pad = pad_of(&[left, right]);
        assert_eq!(pad.buttons, Pad::X | Pad::B | Pad::LEFT_STICK);
        // The stick's play is taken out.
        assert_eq!((pad.left_x, pad.left_y, pad.right_x, pad.left_trigger), (0.0, 0.9, -0.6, 0.4));
        // Both sticks pressed are the menu button, and nothing else.
        let both = pad_of(&[left, HandIn { stick_click: true, ..right }]);
        assert_eq!(both.buttons, Pad::X | Pad::B | Pad::MENU);
    }
}
