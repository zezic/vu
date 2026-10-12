use std::{collections::VecDeque, f64::consts::TAU};

use crate::config::ShutterFrames;

const RESPONSE: f64 = TAU * 4.0;

#[derive(Clone, Copy)]
struct State {
    position: f64,
    velocity: f64,
}

impl State {
    fn at(self, target: f64, dt: f64) -> Self {
        if dt == 0.0 {
            return self;
        }
        let displacement = self.position - target;
        let slope = self.velocity + RESPONSE * displacement;
        let decay = (-RESPONSE * dt).exp();
        Self {
            position: target + (displacement + slope * dt) * decay,
            velocity: (self.velocity - RESPONSE * slope * dt) * decay,
        }
    }
}

/// The actual path under one frame's target, before endpoint clamping.
struct Segment {
    start: State,
    target: f64,
    duration: f64,
}

impl Segment {
    fn exposure(&self, fraction: f64) -> Exposure {
        let dt = self.duration * fraction;
        let start = self.start.at(self.target, self.duration - dt);
        let end = start.at(self.target, dt);
        // Integrating the differential equation gives the exposure's mean position.
        let average = self.target
            - (end.velocity - start.velocity + 2.0 * RESPONSE * (end.position - start.position))
                / (RESPONSE * RESPONSE * dt);
        let mut minimum = start.position.min(end.position).clamp(0.0, 1.0);
        let mut maximum = start.position.max(end.position).clamp(0.0, 1.0);
        let displacement = start.position - self.target;
        let slope = start.velocity + RESPONSE * displacement;
        if slope != 0.0 {
            let turning_time = start.velocity / (RESPONSE * slope);
            if turning_time > 0.0 && turning_time < dt {
                let turning_position = start.at(self.target, turning_time).position.clamp(0.0, 1.0);
                minimum = minimum.min(turning_position);
                maximum = maximum.max(turning_position);
            }
        }
        Exposure {
            minimum,
            average: average.clamp(minimum, maximum),
            maximum,
        }
    }
}

/// A critically damped needle with a trailing shutter over recent frame intervals.
pub(super) struct Motion {
    state: State,
    history: VecDeque<Segment>,
}

pub(super) struct Exposure {
    pub minimum: f64,
    pub average: f64,
    pub maximum: f64,
}

impl Motion {
    #[cfg(test)]
    pub fn position(&self) -> f64 {
        self.state.position
    }

    pub fn new() -> Self {
        Self {
            state: State {
                position: 0.0,
                velocity: 0.0,
            },
            history: VecDeque::with_capacity(ShutterFrames::MAX as usize),
        }
    }

    pub fn advance(&mut self, target: f32, dt: f32, shutter: ShutterFrames) -> Exposure {
        assert!(
            dt.is_finite() && dt >= 0.0,
            "frame duration must be finite and nonnegative"
        );
        if dt == 0.0 {
            return Exposure {
                minimum: self.state.position,
                average: self.state.position,
                maximum: self.state.position,
            };
        }
        let target = f64::from(target.clamp(0.0, 1.0));
        let duration = f64::from(dt);
        if self.history.len() == ShutterFrames::MAX as usize {
            self.history.pop_front();
        }
        self.history.push_back(Segment {
            start: self.state,
            target,
            duration,
        });
        let end = self.state.at(target, duration);
        let position = end.position.clamp(0.0, 1.0);
        self.state = State {
            position,
            velocity: if position == end.position {
                end.velocity
            } else {
                0.0
            },
        };

        let mut remaining = f64::from(shutter.frames());
        let mut minimum = position;
        let mut maximum = position;
        let mut integral = 0.0;
        let mut elapsed = 0.0;
        for segment in self.history.iter().rev() {
            let fraction = remaining.min(1.0);
            let exposure = segment.exposure(fraction);
            let duration = segment.duration * fraction;
            minimum = minimum.min(exposure.minimum);
            maximum = maximum.max(exposure.maximum);
            integral += exposure.average * duration;
            elapsed += duration;
            remaining -= fraction;
            if remaining == 0.0 {
                break;
            }
        }
        Exposure {
            minimum,
            average: (integral / elapsed).clamp(minimum, maximum),
            maximum,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-7, "{actual} != {expected}");
    }

    #[test]
    fn longer_shutter_integrates_the_same_path_without_changing_motion() {
        let mut one_frame = Motion::new();
        let mut two_frames = Motion::new();
        let shutter = ShutterFrames::try_from(2.0).unwrap();
        one_frame.advance(1.0, 0.01, ShutterFrames::default());
        two_frames.advance(1.0, 0.01, shutter);
        let short = one_frame.advance(1.0, 0.02, ShutterFrames::default());
        let long = two_frames.advance(1.0, 0.02, shutter);
        let mut uninterrupted = Motion::new();
        let expected = uninterrupted.advance(1.0, 0.03, ShutterFrames::default());
        near(one_frame.state.position, two_frames.state.position);
        near(one_frame.state.velocity, two_frames.state.velocity);
        near(long.minimum, expected.minimum);
        near(long.maximum, expected.maximum);
        near(long.average, expected.average);
        assert!(long.maximum - long.minimum > short.maximum - short.minimum);
    }

    #[test]
    fn fractional_shutter_uses_the_tail_of_the_oldest_frame() {
        let mut motion = Motion::new();
        for _ in 0..3 {
            motion.advance(1.0, 0.01, ShutterFrames::default());
        }
        let actual = motion.advance(1.0, 0.01, ShutterFrames::try_from(2.5).unwrap());
        let mut expected_motion = Motion::new();
        expected_motion.advance(1.0, 0.015, ShutterFrames::default());
        let expected = expected_motion.advance(1.0, 0.025, ShutterFrames::default());
        near(actual.minimum, expected.minimum);
        near(actual.maximum, expected.maximum);
        near(actual.average, expected.average);
    }

    #[test]
    fn shutter_keeps_reversals_and_expires_old_motion() {
        let mut motion = Motion::new();
        let shutter = ShutterFrames::try_from(2.0).unwrap();
        let rising = motion.advance(1.0, 0.1, shutter);
        let turning = motion.advance(0.0, 0.1, shutter);
        assert!(
            turning.maximum > rising.maximum,
            "the path turns inside the falling frame"
        );
        near(turning.minimum, 0.0);
        for _ in 0..100 {
            motion.advance(0.0, 0.1, shutter);
        }
        let settled = motion.advance(0.0, 0.1, shutter);
        assert!(settled.maximum - settled.minimum < 1e-10);
        assert_eq!(motion.history.len(), ShutterFrames::MAX as usize);
        let paused = motion.advance(0.0, 0.0, shutter);
        near(paused.minimum, paused.maximum);
        near(paused.average, motion.state.position);
    }
}
