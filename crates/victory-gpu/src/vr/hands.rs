//! The player's hands in the headset: SteamVR's own hand model
//! ([`super::glove`]) on each controller, its fingers bent by what the
//! controller says.
//!
//! Three numbers bend a hand, each from 0 (flat) to 1 (a fist): the thumb's,
//! the index finger's, and one for the other three. The grip closes all of
//! them; the trigger bends the index finger alone.

/// Fingers that bend on their own: thumb, index, the other three together.
pub(super) const THUMB: usize = 0;
pub(super) const INDEX: usize = 1;
pub(super) const OTHERS: usize = 2;

/// The bends a controller's `grip` and `trigger` (each 0 to 1) ask for.
pub(super) fn wanted(grip: f32, trigger: f32) -> [f32; 3] {
    let mut c = [0.0; 3];
    c[THUMB] = grip;
    c[INDEX] = grip.max(trigger);
    c[OTHERS] = grip;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grip_and_trigger_bend_the_fingers() {
        assert_eq!(wanted(0.0, 0.0), [0.0; 3]);
        // The trigger alone: the index finger alone.
        assert_eq!(wanted(0.0, 0.8), [0.0, 0.8, 0.0]);
        assert_eq!(wanted(1.0, 0.2), [1.0; 3]);
    }
}
