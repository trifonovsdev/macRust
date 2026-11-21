//! Spring physics for every moving value in the shell.
//!
//! Springs are parameterized the way SwiftUI does it: `response` is the period of
//! the undamped oscillation in seconds, `damping` is the damping ratio
//! (1.0 = critically damped, < 1.0 overshoots). Retargeting a moving spring keeps
//! its velocity, which is what makes interrupted animations look continuous.

use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    stiffness: f32,
    friction: f32,
    epsilon: f32,
}

impl Spring {
    pub fn new(value: f32, response: f32, damping: f32) -> Self {
        let omega = TAU / response.max(0.01);
        Self {
            value,
            velocity: 0.0,
            target: value,
            stiffness: omega * omega,
            friction: 2.0 * damping * omega,
            epsilon: 0.0005,
        }
    }

    /// Settle threshold in the spring's own units (default 0.0005, right for 0..1 values).
    pub fn epsilon(mut self, epsilon: f32) -> Self {
        self.epsilon = epsilon;
        self
    }

    pub fn to(&mut self, target: f32) {
        self.target = target;
    }

    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// Advances the spring; returns true while it is still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.settled() {
            return false;
        }
        // A stalled frame slows the animation down instead of teleporting it.
        let mut left = dt.clamp(0.0, 0.064);
        while left > 1e-6 {
            let h = left.min(1.0 / 480.0);
            let accel = -self.stiffness * (self.value - self.target) - self.friction * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
            left -= h;
        }
        if (self.value - self.target).abs() < self.epsilon && self.velocity.abs() < self.epsilon * 8.0 {
            self.snap(self.target);
            return false;
        }
        true
    }

    pub fn settled(&self) -> bool {
        self.value == self.target && self.velocity == 0.0
    }
}

/// One launch hop as measured on macOS 26: a decelerating rise, a short hang
/// at the top and an accelerating fall, without squash.
pub const BOUNCE_RISE: f32 = 0.28;
pub const BOUNCE_HANG: f32 = 0.12;
pub const BOUNCE_FALL: f32 = 0.27;
pub const BOUNCE_PERIOD: f32 = BOUNCE_RISE + BOUNCE_HANG + BOUNCE_FALL;

/// Height (0..1 of the peak) of the dock launch bounce at `t` seconds; hops repeat.
pub fn launch_bounce(t: f32) -> f32 {
    if t < 0.0 {
        return 0.0;
    }
    let p = t % BOUNCE_PERIOD;
    if p < BOUNCE_RISE {
        let u = 1.0 - p / BOUNCE_RISE;
        1.0 - u * u
    } else if p < BOUNCE_RISE + BOUNCE_HANG {
        1.0
    } else {
        let u = ((p - BOUNCE_RISE - BOUNCE_HANG) / BOUNCE_FALL).min(1.0);
        1.0 - u * u
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_settles_on_target() {
        let mut s = Spring::new(0.0, 0.3, 1.0);
        s.to(1.0);
        let mut frames = 0;
        while s.step(1.0 / 120.0) {
            frames += 1;
            assert!(frames < 1000, "spring never settled");
        }
        assert_eq!(s.value, 1.0);
    }

    #[test]
    fn underdamped_spring_overshoots() {
        let mut s = Spring::new(0.0, 0.4, 0.5);
        s.to(1.0);
        let mut peak = 0.0f32;
        while s.step(1.0 / 120.0) {
            peak = peak.max(s.value);
        }
        assert!(peak > 1.05);
    }

    #[test]
    fn bounce_rises_hangs_and_falls() {
        assert!(launch_bounce(0.0) < 1e-3);
        assert!((launch_bounce(BOUNCE_RISE) - 1.0).abs() < 1e-3);
        assert!((launch_bounce(BOUNCE_RISE + BOUNCE_HANG * 0.5) - 1.0).abs() < 1e-6);
        // Decelerating rise: more than half the height in the first half of the rise.
        assert!(launch_bounce(BOUNCE_RISE * 0.5) > 0.7);
        // Accelerating fall: still high halfway down in time.
        assert!(launch_bounce(BOUNCE_RISE + BOUNCE_HANG + BOUNCE_FALL * 0.5) > 0.7);
        assert!(launch_bounce(BOUNCE_PERIOD * 0.999) < 0.02);
        assert!((BOUNCE_PERIOD - 0.67).abs() < 1e-4);
    }
}
