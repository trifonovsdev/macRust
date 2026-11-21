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

pub const BOUNCE_PERIOD: f32 = 0.62;

/// Height (0..1 of the peak) of the dock launch bounce at `t` seconds.
/// Each hop is a parabola; later hops are a little lower.
pub fn launch_bounce(t: f32) -> f32 {
    if t < 0.0 {
        return 0.0;
    }
    let hop = (t / BOUNCE_PERIOD).floor();
    let phase = (t - hop * BOUNCE_PERIOD) / BOUNCE_PERIOD;
    let damp = 0.86f32.powf(hop);
    let y = 4.0 * phase * (1.0 - phase);
    // The power < 1 makes the icon hang at the top of the hop like the original.
    y.max(0.0).powf(0.85) * damp
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
    fn bounce_starts_and_ends_on_the_ground() {
        assert!(launch_bounce(0.0) < 1e-3);
        assert!((launch_bounce(BOUNCE_PERIOD / 2.0) - 1.0).abs() < 1e-3);
        assert!(launch_bounce(BOUNCE_PERIOD * 0.999) < 0.02);
    }
}
