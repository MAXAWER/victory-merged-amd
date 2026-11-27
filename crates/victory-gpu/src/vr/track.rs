//! The thread of the headset mode that follows the devices ("victory-vr"):
//! some ninety times a second it asks where the head, the controllers and
//! the trackers worn on the body are and what the controllers' buttons say,
//! and leaves three things for others: the title's pad as the controllers
//! hold it ([`super::pad`], taken by `victory_hle`'s input), the hands and
//! trackers for the thread that draws ([`super::Views`]), and the player's
//! body ([`super::body`]).
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
//! Where the picture is fixed to the head (`immersive`) the head's turning
//! is added to the right stick: a full push for `VICTORY_VR_LOOK` degrees a
//! second (160).
//!
//! The keys: F6 the next way of looking, F7 the view centred again, F8 and
//! F9 the frame shown smaller and larger (with Ctrl: the whole nearer and
//! further off), F10 hands or controllers.

use std::time::{Duration, Instant};

use super::math::{IDENTITY, M34, V3, dot, level_forward, pitch, place, scale, sub};
use super::{Api, BodyPart, HandIn, HandView, Mode, Pad, TrackerView, UNIVERSE_STANDING, Views, emu, hands, number, system};

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
type DeviceClass = unsafe extern "system" fn(u32) -> i32;
type StringProperty = unsafe extern "system" fn(u32, i32, *mut std::ffi::c_char, u32, *mut i32) -> u32;

const DEVICES: usize = 64;
const HEAD: usize = 0;
/// `ETrackedControllerRole`: left, right.
const ROLES: [i32; 2] = [1, 2];
/// `ETrackedDeviceClass`: a tracker worn or fixed to something.
const CLASS_TRACKER: i32 = 3;
/// `ETrackedDeviceProperty`: the model SteamVR draws a device with, and the
/// kind of a controller or tracker.
const PROPERTY_MODEL: i32 = 1003;
const PROPERTY_KIND: i32 = 7000;
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

/// A string property of a device; empty if it has none.
fn property(api: &Api, device: u32, property: i32) -> String {
    let mut text = [0u8; 256];
    let mut error = 0i32;
    // SAFETY: the table's entry by its type; the buffer is of the size said.
    let length = unsafe { api.system::<StringProperty>(system::STRING_PROPERTY)(device, property, text.as_mut_ptr().cast(), text.len() as u32, &mut error) };
    if error != 0 || length == 0 {
        return String::new();
    }
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    String::from_utf8_lossy(&text[..end]).into_owned()
}

/// One hand's state from tick to tick.
struct Hand {
    right: bool,
    curl: [f32; 3],
    /// For the speed of an emulated hand, and for strikes.
    before: Option<(V3, Instant)>,
    strike_until: Option<Instant>,
    strike_again: Instant,
    /// The controller's model, asked once for each device number.
    model: Option<(u32, String)>,
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

/// The trackers worn, as SteamVR has them now.
fn trackers(api: &Api, poses: &[Pose; DEVICES], kinds: &mut [Option<BodyPart>; DEVICES]) -> Vec<TrackerView> {
    let mut out = Vec::new();
    for device in 1..DEVICES {
        if !(poses[device].valid && poses[device].connected) {
            kinds[device] = None;
            continue;
        }
        let part = match kinds[device] {
            Some(part) => part,
            None => {
                // SAFETY: the table's entry by its type.
                if unsafe { api.system::<DeviceClass>(system::DEVICE_CLASS)(device as u32) } != CLASS_TRACKER {
                    continue;
                }
                let kind = property(api, device as u32, PROPERTY_KIND);
                let part = BodyPart::of(&kind);
                tracing::info!(device, kind, ?part, "the headset mode: a tracker on the body");
                kinds[device] = Some(part);
                part
            }
        };
        out.push(TrackerView { part, device: Some(device as u32), pose: poses[device].at });
    }
    out
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
    let reaching = std::env::var("VICTORY_VR_EMU_POSE").is_ok_and(|v| v == "reach");
    let no_trackers = std::env::var_os("VICTORY_VR_EMU_TRACKERS").is_some_and(|v| v == "0");
    let emulated_model = std::env::var("VICTORY_VR_EMU_MODEL").unwrap_or_else(|_| "vr_controller_vive_1_5".into());
    let strike_speed = number("VICTORY_VR_STRIKE_SPEED", 1.4).max(0.2);
    let look = number("VICTORY_VR_LOOK", 160.0).max(10.0);
    let strike = strike_button();
    let began = Instant::now();
    let new = |right| Hand { right, curl: [0.0; 3], before: None, strike_until: None, strike_again: began, model: None };
    let mut state = [new(false), new(true)];
    let mut kinds: [Option<BodyPart>; DEVICES] = [None; DEVICES];
    let mut head = IDENTITY;
    let mut head_seen = false;
    let mut keys = [false; 5];
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
        let seconds = (now - began).as_secs_f32();
        let hands_in = match (emulated, reaching) {
            (true, true) => emu::reaching(seconds, &head),
            (true, false) => emu::hands(seconds, &head),
            (false, _) => controllers(api, &poses),
        };
        let worn = match emulated {
            true if no_trackers => Vec::new(),
            true => emu::trackers(&head),
            false => trackers(api, &poses, &mut kinds),
        };
        let (forward, yaw) = level_forward(&head);

        // The keys.
        let down = [key(0x75), key(0x76), key(0x77), key(0x78), key(0x79)];
        let pressed = |k: usize| down[k] && !keys[k];
        if pressed(0) {
            let mode = super::next_mode();
            tracing::info!(?mode, "the headset mode: the way of looking changed (F6)");
        }
        if pressed(1) {
            super::RECENTRE.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if pressed(2) || pressed(3) {
            // With Ctrl: how near the whole is; without: how large the frame is shown.
            if key(0x11) {
                let scale = super::scale_world(if pressed(2) { 0.8 } else { 1.25 });
                tracing::info!(scale, "the headset mode: the world's scale (Ctrl+F8 nearer, Ctrl+F9 further)");
            } else {
                let degrees = super::size_picture(if pressed(2) { 0.9 } else { 1.0 / 0.9 }).to_degrees();
                tracing::info!(degrees, "the headset mode: the frame's size (F8, F9)");
            }
        }
        if pressed(4) {
            let held = super::next_held();
            tracing::info!(?held, "the headset mode: what is drawn at the controllers (F10)");
        }
        keys = down;
        let mode = super::mode();

        // The hands, and what they strike.
        let mut pad = pad_of(&hands_in);
        let mut views = Views { hands: Default::default(), trackers: worn };
        for ((hand, state), view) in hands_in.iter().zip(&mut state).zip(&mut views.hands) {
            if !hand.present {
                state.before = None;
                continue;
            }
            for (have, want) in state.curl.iter_mut().zip(hands::wanted(hand.grip, hand.trigger)) {
                *have += (want - *have) * (dt * 22.0).min(1.0);
            }
            let model = match hand.device {
                None => emulated_model.clone(),
                Some(device) => {
                    if state.model.as_ref().is_none_or(|(d, _)| *d != device) {
                        let name = property(api, device, PROPERTY_MODEL);
                        tracing::info!(right = state.right, device, model = name, kind = property(api, device, PROPERTY_KIND), "the headset mode: a controller");
                        state.model = Some((device, name));
                    }
                    state.model.as_ref().map(|(_, name)| name.clone()).unwrap_or_default()
                }
            };
            *view = HandView { present: true, device: hand.device, pose: hand.pose, curl: state.curl, grip: hand.grip, model };
            let at = place(&hand.pose);
            let velocity = hand.velocity.or_else(|| state.before.map(|(was, when)| scale(sub(at, was), 1.0 / (now - when).as_secs_f32().max(1e-3))));
            state.before = Some((at, now));
            let thrown = velocity.map_or(0.0, |v| dot(v, forward));
            if hand.grip > 0.7 && thrown > strike_speed && now >= state.strike_again {
                state.strike_until = Some(now + STRIKE_HELD);
                state.strike_again = now + STRIKE_AGAIN;
                strikes += 1;
                if strikes <= 12 || strikes.is_power_of_two() {
                    tracing::info!(right = state.right, speed = thrown, strikes, ?strike, "the headset mode: a strike");
                }
                if let Some(device) = hand.device {
                    // SAFETY: the table's entry by its type.
                    unsafe { api.system::<Haptic>(system::HAPTIC_PULSE)(device, 0, 3000) };
                }
            }
            if state.strike_until.is_some_and(|until| now < until) {
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
        let hand_at = |i: usize| views.hands[i].present.then_some(views.hands[i].pose);
        *super::BODY.lock().unwrap_or_else(|e| e.into_inner()) = Some(super::Body {
            head: head_seen.then_some(head),
            left_hand: hand_at(0),
            right_hand: hand_at(1),
            trackers: views.trackers.iter().map(|t| (t.part, t.pose)).collect(),
        });
        let seen = (views.hands[0].present, views.hands[1].present, views.trackers.len());
        *super::VIEWS.lock().unwrap_or_else(|e| e.into_inner()) = Some(views);

        if now - said >= Duration::from_secs(5) {
            said = now;
            tracing::info!(
                ?mode,
                held = ?super::held(),
                head_seen,
                head = ?place(&head).map(|v| (v * 100.0).round() / 100.0),
                hands = ?(seen.0, seen.1),
                trackers = seen.2,
                curl_left = ?state[0].curl.map(|v| (v * 100.0).round() / 100.0),
                curl_right = ?state[1].curl.map(|v| (v * 100.0).round() / 100.0),
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
