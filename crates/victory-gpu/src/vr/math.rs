//! Vectors and the 3x4 matrices OpenVR places things with (`HmdMatrix34_t`:
//! rows, the fourth column the place; -z is forward, +y up, +x right).

pub(super) type V3 = [f32; 3];
pub(super) type M34 = [[f32; 4]; 3];

pub(super) const IDENTITY: M34 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

pub(super) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(super) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(super) fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(super) fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(super) fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub(super) fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` at length one; `fallback` where it has none.
pub(super) fn unit(a: V3, fallback: V3) -> V3 {
    let l = length(a);
    if l > 1e-6 { scale(a, 1.0 / l) } else { fallback }
}

/// The matrix with the axes `x`, `y`, `z` as columns and the place `at`.
pub(super) fn from_axes(x: V3, y: V3, z: V3, at: V3) -> M34 {
    [[x[0], y[0], z[0], at[0]], [x[1], y[1], z[1], at[1]], [x[2], y[2], z[2], at[2]]]
}

/// Column `i` of the rotation (0 right, 1 up, 2 back).
pub(super) fn axis(m: &M34, i: usize) -> V3 {
    [m[0][i], m[1][i], m[2][i]]
}

pub(super) fn place(m: &M34) -> V3 {
    [m[0][3], m[1][3], m[2][3]]
}

/// A direction turned by `m`.
pub(super) fn turn(m: &M34, v: V3) -> V3 {
    [dot([m[0][0], m[0][1], m[0][2]], v), dot([m[1][0], m[1][1], m[1][2]], v), dot([m[2][0], m[2][1], m[2][2]], v)]
}

/// A point moved by `m`.
pub(super) fn point(m: &M34, v: V3) -> V3 {
    add(turn(m, v), place(m))
}

/// What `b` places, placed by `a` (`a * b`).
pub(super) fn mul(a: &M34, b: &M34) -> M34 {
    from_axes(turn(a, axis(b, 0)), turn(a, axis(b, 1)), turn(a, axis(b, 2)), point(a, place(b)))
}

/// Where `m` looks, level with the floor (-z without its height), and the
/// angle of that from -z about +y, in radians (turning left is positive).
pub(super) fn level_forward(m: &M34) -> (V3, f32) {
    let back = axis(m, 2);
    let forward = unit([-back[0], 0.0, -back[2]], [0.0, 0.0, -1.0]);
    (forward, f32::atan2(-forward[0], -forward[2]))
}

/// How far above the horizon `m` looks, in radians.
pub(super) fn pitch(m: &M34) -> f32 {
    (-axis(m, 2)[1]).clamp(-1.0, 1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matrix_places_and_turns() {
        // Turned a quarter to the left about +y, standing at (1, 2, 3).
        let m = from_axes([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
        let (forward, yaw) = level_forward(&m);
        assert!((forward[0] + 1.0).abs() < 1e-6 && forward[2].abs() < 1e-6);
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(point(&m, [0.0, 0.0, 0.0]), [1.0, 2.0, 3.0]);
        let both = mul(&m, &from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -2.0]));
        // Two metres in front of it is two metres to its left in the room.
        assert!((place(&both)[0] + 1.0).abs() < 1e-6 && (place(&both)[2] - 3.0).abs() < 1e-6);
        assert!(pitch(&m).abs() < 1e-6);
    }
}
