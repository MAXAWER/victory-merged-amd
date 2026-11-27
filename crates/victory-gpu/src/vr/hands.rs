//! The player's hands in the headset: each a palm and five fingers of two
//! segments, flat pieces (overlays) that follow the controller and bend with
//! its grip and trigger.
//!
//! A hand's pieces are laid out in the hand's own frame (x across the
//! knuckles, y along the fingers, z out of the back of the hand) and that
//! frame sits in the controller's (+x right, +y up, -z forward): held
//! naturally the thumb is up, the palm faces inward and the fingers point
//! forward. With a real controller every piece is fixed to the controller by
//! SteamVR itself, which then moves it at the headset's own rate; a piece's
//! place is told again only when the fingers bend. With emulated controllers
//! the pieces are placed in the room each tick.

use super::math::{M34, V3, add, cross, from_axes, mul, scale, unit};

/// Pieces of a hand: the palm, then the thumb's two, then two for each of
/// the four fingers.
pub(super) const PIECES: usize = 11;
/// Fingers that bend on their own: thumb, index, the other three together.
pub(super) const THUMB: usize = 0;
pub(super) const INDEX: usize = 1;
pub(super) const OTHERS: usize = 2;

/// A piece: its place in the controller's frame, and its size in metres
/// (across, along).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Piece {
    pub(super) at: M34,
    pub(super) size: [f32; 2],
}

/// Where the middle of the knuckle line lies in the controller's frame: the
/// hand closes around the handle, a little under and behind the controller's
/// own origin.
const KNUCKLES: V3 = [0.0, -0.035, -0.02];
const PALM: [f32; 2] = [0.086, 0.09];
/// Index to little finger: place across the knuckles, first and second
/// segment's length.
const FINGERS: [(f32, f32, f32); 4] = [(-0.031, 0.046, 0.040), (-0.0103, 0.050, 0.045), (0.0103, 0.047, 0.042), (0.031, 0.037, 0.033)];
const FINGER_WIDTH: f32 = 0.0185;
const THUMB_LENGTHS: [f32; 2] = [0.042, 0.034];
const THUMB_WIDTH: f32 = 0.021;

/// The pieces of the `right` or left hand with its fingers bent by `curl`
/// (0 straight, 1 a fist; [`THUMB`], [`INDEX`], [`OTHERS`]).
pub(super) fn pieces(right: bool, curl: [f32; 3]) -> [Piece; PIECES] {
    // The hand's frame in the controller's. The index finger is on top with
    // either hand, so the two hands' x axes are opposite and the fingers'
    // places across the knuckles change sign.
    let side = if right { 1.0 } else { -1.0 };
    let hx: V3 = [0.0, -side, 0.0];
    let hy: V3 = [0.0, 0.0, -1.0];
    let hz: V3 = [side, 0.0, 0.0];
    let hand = from_axes(hx, hy, hz, KNUCKLES);
    let local = |x: V3, y: V3, at: V3, size: [f32; 2]| Piece { at: mul(&hand, &from_axes(x, y, cross(x, y), at)), size };
    let mut out = [Piece { at: hand, size: PALM }; PIECES];
    // The palm lies behind the knuckle line.
    out[0] = local([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -PALM[1] / 2.0, 0.0], PALM);
    // A finger bends about the knuckle line, towards the palm (-z): its
    // first segment by up to 95 degrees, the second by up to 100 more.
    let bent = |angle: f32| -> V3 { [0.0, angle.cos(), -angle.sin()] };
    for (i, &(across, first, second)) in FINGERS.iter().enumerate() {
        let c = curl[if i == 0 { INDEX } else { OTHERS }].clamp(0.0, 1.0);
        let (a1, a2) = (c * 95f32.to_radians(), c * 195f32.to_radians());
        let knuckle: V3 = [across * side, 0.0, 0.0];
        let joint = add(knuckle, scale(bent(a1), first));
        out[3 + 2 * i] = local([1.0, 0.0, 0.0], bent(a1), add(knuckle, scale(bent(a1), first / 2.0)), [FINGER_WIDTH, first]);
        out[4 + 2 * i] = local([1.0, 0.0, 0.0], bent(a2), add(joint, scale(bent(a2), second / 2.0)), [FINGER_WIDTH, second]);
    }
    // The thumb starts beside the palm on the index finger's side and points
    // forward and up; bent, it folds over the fingers.
    let c = curl[THUMB].clamp(0.0, 1.0);
    let open: V3 = [-0.62 * side, 0.78, 0.0];
    let folded: V3 = [0.35 * side, 0.80, -0.48];
    let along = |t: f32| unit(add(scale(open, 1.0 - t), scale(folded, t)), open);
    let base: V3 = [-0.043 * side, -0.062, -0.004 * c];
    let (d1, d2) = (along(c * 0.6), along(c));
    let across = |d: V3| unit(cross(d, [0.0, 0.0, 1.0]), [1.0, 0.0, 0.0]);
    let joint = add(base, scale(d1, THUMB_LENGTHS[0]));
    out[1] = local(across(d1), d1, add(base, scale(d1, THUMB_LENGTHS[0] / 2.0)), [THUMB_WIDTH, THUMB_LENGTHS[0]]);
    out[2] = local(across(d2), d2, add(joint, scale(d2, THUMB_LENGTHS[1] / 2.0)), [THUMB_WIDTH, THUMB_LENGTHS[1]]);
    out
}

/// A piece's picture: `width` by `height` texels of four bytes, white with a
/// darker rim, so that pieces of one colour still read as fingers.
pub(super) fn picture(size: [f32; 2]) -> (Vec<u8>, u32, u32) {
    let width = 16u32;
    let height = ((size[1] / size[0]) * width as f32).round().clamp(4.0, 128.0) as u32;
    let mut texels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let rim = x == 0 || y == 0 || x == width - 1 || y == height - 1;
            let shade = if rim { 150 } else { 255 };
            texels.extend_from_slice(&[shade, shade, shade, 255]);
        }
    }
    (texels, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vr::math::{axis, dot, place, sub};

    #[test]
    fn an_open_hand_points_forward_and_a_fist_folds_under() {
        for right in [true, false] {
            let open = pieces(right, [0.0; 3]);
            // The index finger's tip segment lies in front of the knuckles
            // (-z), above the little finger's.
            let (index, little) = (place(&open[4].at), place(&open[10].at));
            assert!(index[2] < KNUCKLES[2] - 0.05 && index[1] > little[1] + 0.04, "{right}: {index:?} {little:?}");
            // The back of the hand faces outward: +x for the right hand.
            let out = axis(&open[0].at, 2);
            assert!((out[0] - if right { 1.0 } else { -1.0 }).abs() < 1e-5);
            // Every piece's axes are at right angles and of length one.
            for p in &open {
                let (x, y, z) = (axis(&p.at, 0), axis(&p.at, 1), axis(&p.at, 2));
                assert!(dot(x, y).abs() < 1e-4 && (dot(z, z) - 1.0).abs() < 1e-4 && (dot(x, x) - 1.0).abs() < 1e-4);
            }
            let fist = pieces(right, [1.0; 3]);
            // The tips have come back behind the knuckles' first segments,
            // on the palm's side (inward).
            let tip = sub(place(&fist[4].at), KNUCKLES);
            let inward = if right { -1.0 } else { 1.0 };
            assert!(tip[2] > -0.04 && tip[0] * inward > 0.01, "{right}: {tip:?}");
        }
    }

    #[test]
    fn a_picture_has_its_pieces_shape() {
        let (texels, w, h) = picture([0.02, 0.05]);
        assert_eq!((w, h, texels.len()), (16, 40, 16 * 40 * 4));
        assert_eq!((texels[0], texels[(17 * 4) as usize]), (150, 255));
    }
}
