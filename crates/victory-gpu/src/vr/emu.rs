//! Controllers that are not there (`VICTORY_VR_EMU=1`): two hands that go
//! through the motions by the clock, for trying the headset mode on a PC
//! with no controllers, or with SteamVR's own stand-in headset (its `null`
//! driver) and nothing else.
//!
//! Every four seconds a hand closes into a fist, strikes forward and opens
//! again, the left hand two seconds behind the right; the left hand also
//! pulls its trigger once a cycle, which bends its index finger alone. The
//! sticks and buttons stay at rest: the title is played from the keyboard
//! meanwhile.
//!
//! Or (`VICTORY_VR_EMU_POSE=reach`) both hands are held out in front, the
//! palms half up, as a player holds them out to look at them: they come up
//! from the sides, drift as living hands do, close into fists and open,
//! turn over and back, and go down again, every fourteen seconds. No
//! trigger is pulled meanwhile: the title is given nothing to do.

use super::{BodyPart, HandIn, TrackerView};
use super::math::{M34, add, cross, from_axes, level_forward, mul, place, scale};

/// The cycle of one hand, in seconds.
const CYCLE: f32 = 4.0;
/// How far a strike reaches, in metres, and how long it takes.
const REACH: f32 = 0.35;
const STRIKE: f32 = 0.12;

/// Up from 0 to 1 between `from` and `to`.
fn ramp(u: f32, from: f32, to: f32) -> f32 {
    ((u - from) / (to - from)).clamp(0.0, 1.0)
}

/// What the two hands (left, right) say at `seconds`, held in front of a
/// head at `head`.
pub(super) fn hands(seconds: f32, head: &M34) -> [HandIn; 2] {
    let (forward, _) = level_forward(head);
    let up = [0.0, 1.0, 0.0];
    let right = cross(forward, up);
    let hand = |is_right: bool| {
        let u = (seconds + if is_right { 0.0 } else { CYCLE / 2.0 }).rem_euclid(CYCLE);
        let grip = ramp(u, 1.0, 1.3) * (1.0 - ramp(u, 2.2, 2.5));
        let reach = REACH * ramp(u, 1.5, 1.5 + STRIKE) * (1.0 - ramp(u, 1.5 + STRIKE, 2.0));
        let trigger = if is_right { 0.0 } else { ramp(u, 3.0, 3.2) * (1.0 - ramp(u, 3.2, 3.4)) };
        let bob = 0.01 * (seconds * std::f32::consts::PI).sin();
        let side = if is_right { 0.2 } else { -0.2 };
        let at = add(add(place(head), scale(right, side)), add(scale(up, -0.28 + bob), scale(forward, 0.42 + reach)));
        HandIn {
            present: true,
            device: None,
            pose: from_axes(right, up, scale(forward, -1.0), at),
            velocity: None,
            grip,
            trigger,
            ..HandIn::default()
        }
    };
    [hand(false), hand(true)]
}

/// The time the hands held out take to come up, show themselves and go down.
const SHOW: f32 = 14.0;

/// Gently from 0 to 1 between `from` and `to`.
fn ease(u: f32, from: f32, to: f32) -> f32 {
    let t = ramp(u, from, to);
    t * t * (3.0 - 2.0 * t)
}

/// What the two hands (left, right) say at `seconds` when held out in front
/// of a head at `head`, the palms half up.
pub(super) fn reaching(seconds: f32, head: &M34) -> [HandIn; 2] {
    let (forward, _) = level_forward(head);
    let up = [0.0, 1.0, 0.0];
    let right = cross(forward, up);
    let u = seconds.rem_euclid(SHOW);
    // Held out (1) or down at the sides (0); the palms turned down (1) once.
    let out = ease(u, 0.3, 2.2) * (1.0 - ease(u, 12.2, 13.6));
    let over = ease(u, 8.4, 9.2) * (1.0 - ease(u, 10.0, 11.0));
    let hand = |is_right: bool| {
        let side = if is_right { 1.0 } else { -1.0 };
        let own = if is_right { 0.0 } else { 1.7 };
        // A living hand is never still.
        let drift = [0.006 * (seconds * 0.9 + own).sin(), 0.008 * (seconds * 0.6 + own * 2.0).sin(), 0.006 * (seconds * 0.75 + own).cos()];
        let at = add(
            add(place(head), scale(right, side * (0.24 - 0.07 * out) + drift[0])),
            add(scale(up, -(0.48 - 0.2 * out) + drift[1]), scale(forward, 0.3 + 0.27 * out + drift[2])),
        );
        // The palm from facing the other hand to half up, or down; the
        // fingers lifted a little and pointing a little inward.
        let roll = (55.0 * out - 105.0 * over * out + 4.0 * (seconds * 0.5 + own).sin()).to_radians() * side;
        let (lift, inward) = ((10.0 * out + 2.0 * (seconds * 0.4 + own).sin()).to_radians(), (6.0 * side).to_radians());
        let rolled = from_axes([roll.cos(), -roll.sin(), 0.0], [roll.sin(), roll.cos(), 0.0], [0.0, 0.0, 1.0], [0.0; 3]);
        let lifted = from_axes([1.0, 0.0, 0.0], [0.0, lift.cos(), lift.sin()], [0.0, -lift.sin(), lift.cos()], [0.0; 3]);
        let turned = from_axes([inward.cos(), 0.0, -inward.sin()], [0.0, 1.0, 0.0], [inward.sin(), 0.0, inward.cos()], [0.0; 3]);
        let level = from_axes(right, up, scale(forward, -1.0), at);
        // The fingers: a little bent at rest and never quite still, a fist
        // and open again once, the left hand a little behind the right.
        let lag = if is_right { 0.0 } else { 0.25 };
        let fist = ease(u, 5.4 + lag, 6.1 + lag) * (1.0 - ease(u, 7.0 + lag, 7.8 + lag));
        let grip = (0.16 + 0.06 * (seconds * 1.3 + own).sin() + 0.84 * fist).clamp(0.0, 1.0);
        HandIn { present: true, device: None, pose: mul(&level, &mul(&turned, &mul(&lifted, &rolled))), velocity: None, grip, trigger: 0.0, ..HandIn::default() }
    };
    [hand(false), hand(true)]
}

/// Trackers that are not there: one at the waist and one on each foot of a
/// player whose head is at `head`.
pub(super) fn trackers(head: &M34) -> Vec<TrackerView> {
    let (forward, _) = level_forward(head);
    let up = [0.0, 1.0, 0.0];
    let right = cross(forward, up);
    let at = |side: f32, down: f32, ahead: f32| {
        let at = add(add(place(head), scale(right, side)), add(scale(up, -down), scale(forward, ahead)));
        from_axes(right, up, scale(forward, -1.0), at)
    };
    vec![
        TrackerView { part: BodyPart::Waist, device: None, pose: at(0.0, 0.62, 0.0) },
        TrackerView { part: BodyPart::LeftFoot, device: None, pose: at(-0.13, 1.5, 0.05) },
        TrackerView { part: BodyPart::RightFoot, device: None, pose: at(0.13, 1.5, 0.05) },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vr::math::IDENTITY;

    #[test]
    fn a_hand_closes_strikes_and_opens() {
        let at = |t: f32| hands(t, &IDENTITY)[1];
        // Open at rest, to the right of the head, below and in front of it.
        let rest = at(0.5);
        assert_eq!((rest.grip, rest.trigger), (0.0, 0.0));
        let p = place(&rest.pose);
        assert!(p[0] > 0.15 && p[1] < -0.2 && p[2] < -0.4);
        // A fist before the strike, and the strike goes forward (-z).
        assert_eq!(at(1.45).grip, 1.0);
        let reached = place(&at(1.5 + STRIKE).pose)[2] - p[2];
        assert!((reached + REACH).abs() < 0.02, "{reached}");
        assert_eq!(at(3.0).grip, 0.0);
        // The left hand is half a cycle behind.
        assert_eq!(hands(3.45, &IDENTITY)[0].grip, 1.0);
        // The waist under the head, a foot under each side of it.
        let worn = trackers(&IDENTITY);
        assert_eq!(worn.iter().map(|t| t.part).collect::<Vec<_>>(), [BodyPart::Waist, BodyPart::LeftFoot, BodyPart::RightFoot]);
        assert!(place(&worn[1].pose)[0] < 0.0 && place(&worn[2].pose)[0] > 0.0 && place(&worn[1].pose)[1] < place(&worn[0].pose)[1]);
    }

    #[test]
    fn hands_held_out_show_their_palms() {
        use crate::vr::math::{axis, turn};
        // Held out: in front of the head and under it, a hand on each side.
        let [left, right] = reaching(4.0, &IDENTITY);
        let (l, r) = (place(&left.pose), place(&right.pose));
        assert!(l[0] < -0.1 && r[0] > 0.1 && r[1] < -0.2 && r[2] < -0.5, "{l:?} {r:?}");
        // The palm of a hand on a controller faces the other hand: the right
        // hand's looks along the controller's -x. Held out, it looks up more
        // than sideways, and the fingers (-z) still point ahead.
        let palm = turn(&right.pose, [-1.0, 0.0, 0.0]);
        assert!(palm[1] > 0.6 && palm[0] < 0.0, "{palm:?}");
        let other = turn(&left.pose, [1.0, 0.0, 0.0]);
        assert!(other[1] > 0.6 && other[0] > 0.0, "{other:?}");
        assert!(axis(&right.pose, 2)[2] > 0.9);
        // Open, with no trigger pulled; a fist once in the show; down at
        // the sides at its start.
        assert!(right.grip < 0.3 && right.trigger == 0.0 && left.trigger == 0.0);
        assert!(reaching(6.5, &IDENTITY)[1].grip > 0.95);
        assert!(place(&reaching(0.0, &IDENTITY)[1].pose)[1] < -0.4);
    }
}
