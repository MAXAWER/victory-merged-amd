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

use super::HandIn;
use super::math::{M34, add, cross, from_axes, level_forward, place, scale};

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
    }
}
