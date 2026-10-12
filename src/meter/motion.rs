use std::{collections::VecDeque, f64::consts::TAU};

use crate::config::ShutterFrames;

use super::{NEEDLE_RADIUS, NEEDLE_WIDTH};
use crate::scales::{Scale, MAX_ANGLE};

const RESPONSE: f64 = TAU * 4.0;
const IMPACT_SPEED: f64 = 0.8;
const BENDING_RESISTANCE: f64 = 600.0;
const MAX_BEND: f64 = 1.3;
pub(super) const HINGE_RADIUS: f64 = NEEDLE_RADIUS * 2.0 / 3.0;
pub(super) const STOP_RADIUS: f64 = 2.5;
// Contact is just beyond the hinge, leaving room for the inner section to advance.
const STOP_DISTANCE: f64 = HINGE_RADIUS + 8.0;
const CLEARANCE: f64 = STOP_RADIUS + NEEDLE_WIDTH / 2.0;
const SWEEP: f64 = 2.0 * MAX_ANGLE as f64 * std::f64::consts::PI / 180.0;

pub(super) fn stop_center() -> vello::kurbo::Point {
    let angle = Scale::angle(1.0) + (CLEARANCE / STOP_DISTANCE).asin();
    vello::kurbo::Point::new(STOP_DISTANCE * angle.cos(), STOP_DISTANCE * angle.sin())
}

/// Permanent plastic deformation, in radians backward from the inner section.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Bend(f64);

impl Bend {
    pub fn radians(self) -> f64 {
        self.0
    }

    fn limit(self) -> f64 {
        1.0 + (self.0 + (CLEARANCE / STOP_DISTANCE).asin()
            - ((HINGE_RADIUS * self.0.sin() + CLEARANCE) / STOP_DISTANCE).asin())
            / SWEEP
    }

    fn at_contact(position: f64) -> Self {
        let mut low = 0.0;
        let mut high = MAX_BEND;
        for _ in 0..40 {
            let middle = (low + high) / 2.0;
            if Self(middle).limit() < position {
                low = middle;
            } else {
                high = middle;
            }
        }
        Self((low + high) / 2.0)
    }
}

#[derive(Clone, Copy)]
struct State {
    position: f64,
    velocity: f64,
}

impl State {
    fn at(self, target: f64, dt: f64) -> Self {
        let displacement = self.position - target;
        let slope = self.velocity + RESPONSE * displacement;
        let decay = (-RESPONSE * dt).exp();
        Self {
            position: target + (displacement + slope * dt) * decay,
            velocity: (self.velocity - RESPONSE * slope * dt) * decay,
        }
    }
}

#[derive(Clone, Copy)]
enum Movement {
    Driven { target: f64 },
    Yielding,
    Rest,
}

struct Segment {
    start: State,
    movement: Movement,
    duration: f64,
    bend: Bend,
}

impl Segment {
    fn at(&self, dt: f64) -> State {
        match self.movement {
            Movement::Driven { target } => self.start.at(target, dt),
            Movement::Yielding => State {
                position: self.start.position + self.start.velocity * dt
                    - 0.5 * BENDING_RESISTANCE * dt * dt,
                velocity: self.start.velocity - BENDING_RESISTANCE * dt,
            },
            Movement::Rest => self.start,
        }
    }

    fn bend_at(&self, dt: f64) -> Bend {
        match self.movement {
            Movement::Yielding => Bend::at_contact(self.at(dt).position),
            _ => self.bend,
        }
    }

    fn exposure(&self, from: f64, to: f64) -> Exposure {
        let start = self.at(from);
        let end = self.at(to);
        let dt = to - from;
        let mut minimum = start.position.min(end.position);
        let mut maximum = start.position.max(end.position);
        let average = match self.movement {
            Movement::Driven { target } => {
                let slope = start.velocity + RESPONSE * (start.position - target);
                if slope != 0.0 {
                    let turning_time = start.velocity / (RESPONSE * slope);
                    if turning_time > 0.0 && turning_time < dt {
                        let turning = start.at(target, turning_time).position;
                        minimum = minimum.min(turning);
                        maximum = maximum.max(turning);
                    }
                }
                target
                    - (end.velocity - start.velocity
                        + 2.0 * RESPONSE * (end.position - start.position))
                        / (RESPONSE * RESPONSE * dt)
            }
            Movement::Yielding => {
                start.position + start.velocity * dt / 2.0 - BENDING_RESISTANCE * dt * dt / 6.0
            }
            Movement::Rest => start.position,
        };
        Exposure {
            minimum,
            average: average.clamp(minimum, maximum),
            maximum,
            bend: self.bend_at((from + to) / 2.0),
        }
    }
}

struct FramePath {
    segments: Vec<Segment>,
    duration: f64,
}

/// Damped free motion and velocity-driven plastic deformation at the stop.
pub(super) struct Motion {
    state: State,
    bend: Bend,
    movement: Movement,
    history: VecDeque<FramePath>,
}

pub(super) struct Exposure {
    pub minimum: f64,
    pub average: f64,
    pub maximum: f64,
    pub bend: Bend,
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
            bend: Bend::default(),
            movement: Movement::Rest,
            history: VecDeque::with_capacity(ShutterFrames::MAX as usize),
        }
    }

    pub fn repair(&mut self) {
        self.bend = Bend::default();
        self.state.position = self.state.position.min(self.bend.limit());
        self.state.velocity = 0.0;
        self.movement = Movement::Rest;
        self.history.clear();
    }

    pub fn advance(&mut self, target: f32, dt: f32, shutter: ShutterFrames) -> Exposure {
        assert!(
            dt.is_finite() && dt >= 0.0,
            "frame duration must be finite and nonnegative"
        );
        assert!(
            target.is_finite() && target >= 0.0,
            "needle target must be finite and nonnegative"
        );
        if dt == 0.0 {
            return Exposure {
                minimum: self.state.position,
                average: self.state.position,
                maximum: self.state.position,
                bend: self.bend,
            };
        }
        let target = f64::from(target);
        let duration = f64::from(dt);
        let mut remaining = duration;
        let mut segments = if self.history.len() == ShutterFrames::MAX as usize {
            self.history
                .pop_front()
                .expect("full history has a first frame")
                .segments
        } else {
            Vec::with_capacity(3)
        };
        segments.clear();
        while remaining > 0.0 {
            let (movement, elapsed) = if matches!(self.movement, Movement::Yielding) {
                let distance = Bend(MAX_BEND).limit() - self.state.position;
                let stop_time = self.state.velocity / BENDING_RESISTANCE;
                let energy = self.state.velocity * self.state.velocity;
                let cap_time = if 2.0 * BENDING_RESISTANCE * distance < energy {
                    2.0 * distance
                        / (self.state.velocity
                            + (energy - 2.0 * BENDING_RESISTANCE * distance).sqrt())
                } else {
                    stop_time
                };
                (Movement::Yielding, remaining.min(cap_time))
            } else {
                let limit = self.bend.limit();
                if self.state.position >= limit && target >= limit {
                    (Movement::Rest, remaining)
                } else {
                    // The first crossing is on the increasing part of this analytic path.
                    let slope = self.state.velocity + RESPONSE * (self.state.position - target);
                    let turning = self.state.velocity / (RESPONSE * slope);
                    let peak_time = if slope > 0.0 && turning > 0.0 {
                        remaining.min(turning)
                    } else {
                        remaining
                    };
                    let end = self.state.at(target, peak_time);
                    if end.position > limit {
                        let mut low = 0.0;
                        let mut high = peak_time;
                        for _ in 0..40 {
                            let middle = (low + high) / 2.0;
                            if self.state.at(target, middle).position < limit {
                                low = middle;
                            } else {
                                high = middle;
                            }
                        }
                        (Movement::Driven { target }, high)
                    } else {
                        (Movement::Driven { target }, remaining)
                    }
                }
            };
            if matches!(movement, Movement::Rest) {
                self.state.velocity = 0.0;
            }
            let segment = Segment {
                start: self.state,
                movement,
                duration: elapsed,
                bend: self.bend,
            };
            self.state = segment.at(elapsed);
            self.bend = segment.bend_at(elapsed);
            remaining = (remaining - elapsed).max(0.0);
            segments.push(segment);
            self.movement = match movement {
                Movement::Driven { .. } if self.state.position >= self.bend.limit() => {
                    self.state.position = self.bend.limit();
                    if self.state.velocity >= IMPACT_SPEED && self.bend.0 < MAX_BEND {
                        Movement::Yielding
                    } else {
                        self.state.velocity = 0.0;
                        Movement::Rest
                    }
                }
                Movement::Yielding
                    if remaining > 0.0
                        || self.state.velocity <= 0.0
                        || self.state.position >= Bend(MAX_BEND).limit() =>
                {
                    self.state.velocity = 0.0;
                    Movement::Rest
                }
                other => other,
            };
        }
        self.history.push_back(FramePath { segments, duration });
        let mut frames = f64::from(shutter.frames());
        let mut minimum = self.state.position;
        let mut maximum = self.state.position;
        let mut integral = 0.0;
        let mut bend_integral = 0.0;
        let mut elapsed = 0.0;
        for frame in self.history.iter().rev() {
            let fraction = frames.min(1.0);
            let mut skip = frame.duration * (1.0 - fraction);
            for segment in &frame.segments {
                let from = skip.min(segment.duration);
                skip -= from;
                let duration = segment.duration - from;
                if duration > 0.0 {
                    let exposure = segment.exposure(from, segment.duration);
                    minimum = minimum.min(exposure.minimum);
                    maximum = maximum.max(exposure.maximum);
                    integral += exposure.average * duration;
                    bend_integral += exposure.bend.0 * duration;
                    elapsed += duration;
                }
            }
            frames -= fraction;
            if frames == 0.0 {
                break;
            }
        }
        Exposure {
            minimum,
            average: (integral / elapsed).clamp(minimum, maximum),
            maximum,
            bend: Bend(bend_integral / elapsed),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn run(motion: &mut Motion, target: f32, seconds: f64, fps: u32) {
        for _ in 0..(seconds * f64::from(fps)).round() as u32 {
            motion.advance(target, 1.0 / fps as f32, ShutterFrames::default());
        }
    }

    #[test]
    fn hard_hits_yield_over_time_and_repeated_hits_accumulate_damage() {
        let mut motion = Motion::new();
        let dt = 1.0 / 240.0;
        for _ in 0..240 {
            motion.advance(1.2, dt, ShutterFrames::default());
            if motion.bend != Bend::default() {
                break;
            }
        }
        assert!(motion.bend.0 > 0.0);
        let first = motion.bend;
        assert!(motion.state.position > 1.0);
        assert!(motion.state.velocity > 0.0);
        motion.advance(1.2, dt, ShutterFrames::default());
        assert!(motion.bend.0 > first.0);
        run(&mut motion, 1.2, 1.0, 240);
        let settled = motion.bend;
        run(&mut motion, 1.2, 1.0, 240);
        assert_eq!(
            motion.bend, settled,
            "static pressure does not bend the needle"
        );
        run(&mut motion, 0.0, 1.0, 240);
        run(&mut motion, 1.2, 1.0, 240);
        assert!(motion.bend.0 > settled.0);
        assert!(motion.bend.0 <= MAX_BEND);
        motion.repair();
        assert_eq!(motion.bend, Bend::default());
        assert!(motion.state.position <= 1.0);
        assert_eq!(motion.state.velocity, 0.0);
        assert!(motion.history.is_empty());
    }

    #[test]
    fn slow_contact_stays_straight_and_fast_hits_agree_across_frame_rates() {
        let mut gentle = Motion::new();
        for step in 0..3000 {
            gentle.advance(step as f32 / 2500.0, 1.0 / 240.0, ShutterFrames::default());
        }
        assert_eq!(gentle.bend, Bend::default());
        near(gentle.state.position, 1.0);
        let mut reference = Motion::new();
        run(&mut reference, 1.2, 1.0, 240);
        for fps in [15, 30, 60, 120] {
            let mut motion = Motion::new();
            run(&mut motion, 1.2, 1.0, fps);
            near(motion.bend.0, reference.bend.0);
            near(motion.state.position, reference.state.position);
        }
        let mut extreme = Motion::new();
        run(&mut extreme, 100.0, 1.0, 15);
        near(extreme.bend.0, MAX_BEND);
        near(extreme.state.position, Bend(MAX_BEND).limit());
    }

    #[test]
    fn stop_is_tangent_to_the_outer_section_for_every_bend() {
        let stop = stop_center();
        for step in 0..=12 {
            let bend = Bend(step as f64 / 10.0);
            let angle = Scale::angle(bend.limit() as f32);
            let hinge = super::super::NeedleAppearance::hinge(angle);
            let outer = vello::kurbo::Vec2::new((angle - bend.0).cos(), (angle - bend.0).sin());
            let relative = stop - hinge;
            assert!((outer.cross(relative) - CLEARANCE).abs() < 1e-4);
            assert!(outer.dot(relative) > 0.0);
            assert!(outer.dot(relative) < NEEDLE_RADIUS - HINGE_RADIUS);
        }
    }

    fn near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-7, "{actual} != {expected}");
    }

    #[test]
    fn longer_shutter_integrates_the_same_path_without_changing_motion() {
        let mut one_frame = Motion::new();
        let mut two_frames = Motion::new();
        let shutter = ShutterFrames::try_from(2.0).unwrap();
        one_frame.advance(1.0, 0.01, ShutterFrames::try_from(1.0).unwrap());
        two_frames.advance(1.0, 0.01, shutter);
        let short = one_frame.advance(1.0, 0.02, ShutterFrames::try_from(1.0).unwrap());
        let long = two_frames.advance(1.0, 0.02, shutter);
        let mut uninterrupted = Motion::new();
        let expected = uninterrupted.advance(1.0, 0.03, ShutterFrames::try_from(1.0).unwrap());
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
            motion.advance(1.0, 0.01, ShutterFrames::try_from(1.0).unwrap());
        }
        let actual = motion.advance(1.0, 0.01, ShutterFrames::try_from(2.5).unwrap());
        let mut expected_motion = Motion::new();
        expected_motion.advance(1.0, 0.015, ShutterFrames::try_from(1.0).unwrap());
        let expected = expected_motion.advance(1.0, 0.025, ShutterFrames::try_from(1.0).unwrap());
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
