use std::{collections::VecDeque, f64::consts::TAU};

use crate::{config::ShutterFrames, measurement::Mode};

use super::{NEEDLE_RADIUS, NEEDLE_WIDTH};
use crate::scales::{Scale, MAX_ANGLE};

/// Sifam's two published peak-meter movement curves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PeakMovement {
    FullSpec,
    #[default]
    VintageFr,
}

impl PeakMovement {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::FullSpec => Self::VintageFr,
            Self::VintageFr => Self::FullSpec,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::FullSpec => "Full Spec",
            Self::VintageFr => "Vintage FR",
        }
    }
}

/// Normalized moving-coil mechanics: theta'' + 2*alpha*theta' + omega^2*theta
/// = omega^2*drive. Drive is the equilibrium deflection for the coil current.
#[derive(Clone, Copy)]
struct Response {
    omega: f64,
    damping: Damping,
}

#[derive(Clone, Copy)]
enum Damping {
    Critical,
    Under { ratio: f64 },
}

impl Response {
    fn for_mode(mode: Mode, peak_movement: PeakMovement) -> Self {
        match mode {
            // 99% of a step at 300 ms, with 1.25% overshoot.
            Mode::Vu => Self {
                omega: 13.51191256253271,
                damping: Damping::Under {
                    ratio: 0.8127169864286775,
                },
            },
            Mode::Ppm | Mode::SamplePeak => Self::for_peak_movement(peak_movement),
            Mode::Rms => Self {
                omega: TAU * 4.0,
                damping: Damping::Critical,
            },
        }
    }

    fn for_peak_movement(movement: PeakMovement) -> Self {
        // Fits to the two normalized curves in Sifam's Vintage FR datasheet.
        let (frequency, ratio) = match movement {
            PeakMovement::FullSpec => (5.0, 0.68),
            PeakMovement::VintageFr => (3.07, 0.763),
        };
        Self {
            omega: TAU * frequency,
            damping: Damping::Under { ratio },
        }
    }

    fn alpha(self) -> f64 {
        match self.damping {
            Damping::Critical => self.omega,
            Damping::Under { ratio } => self.omega * ratio,
        }
    }

    fn turning_times(self, start: State, target: f64, duration: f64) -> impl Iterator<Item = f64> {
        let displacement = start.position - target;
        let (first, period) = match self.damping {
            Damping::Critical => {
                let slope = start.velocity + self.omega * displacement;
                (start.velocity / (self.omega * slope), None)
            }
            Damping::Under { ratio } => {
                let beta = self.omega * (1.0 - ratio * ratio).sqrt();
                let sine = (start.velocity + self.alpha() * displacement) / beta;
                let velocity_sine = -self.alpha() * sine - beta * displacement;
                (
                    (-start.velocity)
                        .atan2(velocity_sine)
                        .rem_euclid(std::f64::consts::PI)
                        / beta,
                    Some(std::f64::consts::PI / beta),
                )
            }
        };
        std::iter::successors(Some(first), move |&time| period.map(|period| time + period))
            .take_while(move |&time| time < duration)
            .filter(|&time| time.is_finite() && time > 0.0)
    }
}
const IMPACT_SPEED: f64 = 2.0;
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
    fn at(self, target: f64, dt: f64, response: Response) -> Self {
        let displacement = self.position - target;
        let alpha = response.alpha();
        let decay = (-alpha * dt).exp();
        match response.damping {
            Damping::Critical => {
                let slope = self.velocity + alpha * displacement;
                Self {
                    position: target + (displacement + slope * dt) * decay,
                    velocity: (self.velocity - alpha * slope * dt) * decay,
                }
            }
            Damping::Under { ratio } => {
                let beta = response.omega * (1.0 - ratio * ratio).sqrt();
                let sine = (self.velocity + alpha * displacement) / beta;
                let (sin, cos) = (beta * dt).sin_cos();
                Self {
                    position: target + decay * (displacement * cos + sine * sin),
                    velocity: decay
                        * (self.velocity * cos - (alpha * sine + beta * displacement) * sin),
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Stop {
    Lower,
    Upper,
}

#[derive(Clone, Copy)]
enum Movement {
    Driven { target: f64, response: Response },
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
            Movement::Driven { target, response } => self.start.at(target, dt, response),
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
            Movement::Driven { target, response } => {
                for time in response.turning_times(start, target, dt) {
                    let turning = start.at(target, time, response).position;
                    minimum = minimum.min(turning);
                    maximum = maximum.max(turning);
                }
                target
                    - (end.velocity - start.velocity
                        + 2.0 * response.alpha() * (end.position - start.position))
                        / (response.omega * response.omega * dt)
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
    response: Response,
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

    pub fn new(mode: Mode, peak_movement: PeakMovement) -> Self {
        Self {
            response: Response::for_mode(mode, peak_movement),
            state: State {
                position: 0.0,
                velocity: 0.0,
            },
            bend: Bend::default(),
            movement: Movement::Rest,
            history: VecDeque::with_capacity(ShutterFrames::MAX as usize),
        }
    }

    pub fn set_peak_movement(&mut self, movement: PeakMovement) {
        self.response = Response::for_peak_movement(movement);
    }

    pub fn repair(&mut self) {
        self.bend = Bend::default();
        self.state.position = self.state.position.min(self.bend.limit());
        self.state.velocity = 0.0;
        self.movement = Movement::Rest;
        self.history.clear();
    }

    #[cfg(test)]
    pub fn advance(&mut self, target: f32, dt: f32, shutter: ShutterFrames) -> Exposure {
        self.advance_path(std::iter::once((target, f64::from(dt))), shutter)
    }

    /// One frame can contain many audio-time drives; its history remains one exposure.
    pub fn advance_path(
        &mut self,
        drives: impl Iterator<Item = (f32, f64)>,
        shutter: ShutterFrames,
    ) -> Exposure {
        let mut drives = drives
            .filter(|&(target, dt)| {
                assert!(
                    dt.is_finite() && dt >= 0.0,
                    "drive duration must be finite and nonnegative"
                );
                assert!(
                    target.is_finite() && target >= 0.0,
                    "needle target must be finite and nonnegative"
                );
                dt > 0.0
            })
            .peekable();
        if drives.peek().is_none() {
            return Exposure {
                minimum: self.state.position,
                average: self.state.position,
                maximum: self.state.position,
                bend: self.bend,
            };
        }
        let mut duration = 0.0;
        let mut segments = if self.history.len() == ShutterFrames::MAX as usize {
            self.history
                .pop_front()
                .expect("full history has a first frame")
                .segments
        } else {
            Vec::with_capacity(3)
        };
        segments.clear();
        while let Some((target, mut dt)) = drives.next() {
            while let Some(&(next_target, next_dt)) = drives.peek() {
                if next_target != target {
                    break;
                }
                dt += next_dt;
                drives.next();
            }
            let target = f64::from(target);
            duration += dt;
            let mut remaining = dt;
            while remaining > 0.0 {
                let mut contact = None;
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
                    if (self.state.position >= limit && target >= limit)
                        || (self.state.position == target && self.state.velocity == 0.0)
                    {
                        (Movement::Rest, remaining)
                    } else {
                        let mut previous = 0.0;
                        let mut elapsed = remaining;
                        for time in self
                            .response
                            .turning_times(self.state, target, remaining)
                            .chain(std::iter::once(remaining))
                        {
                            let position = self.state.at(target, time, self.response).position;
                            let boundary = if position > limit {
                                Some(Stop::Upper)
                            } else if position < 0.0 {
                                Some(Stop::Lower)
                            } else {
                                None
                            };
                            if let Some(boundary) = boundary {
                                let mut low = previous;
                                let mut high = time;
                                for _ in 0..40 {
                                    let middle = (low + high) / 2.0;
                                    let position =
                                        self.state.at(target, middle, self.response).position;
                                    let before = match boundary {
                                        Stop::Lower => position > 0.0,
                                        Stop::Upper => position < limit,
                                    };
                                    if before {
                                        low = middle;
                                    } else {
                                        high = middle;
                                    }
                                }
                                elapsed = high;
                                contact = Some(boundary);
                                break;
                            }
                            previous = time;
                        }
                        (
                            Movement::Driven {
                                target,
                                response: self.response,
                            },
                            elapsed,
                        )
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
                self.movement = match contact {
                    Some(Stop::Lower) => {
                        self.state.position = 0.0;
                        self.state.velocity = 0.0;
                        Movement::Rest
                    }
                    Some(Stop::Upper) => {
                        self.state.position = self.bend.limit();
                        if self.state.velocity >= IMPACT_SPEED && self.bend.0 < MAX_BEND {
                            Movement::Yielding
                        } else {
                            self.state.velocity = 0.0;
                            Movement::Rest
                        }
                    }
                    None => match movement {
                        Movement::Yielding
                            if remaining > 0.0
                                || self.state.velocity <= 0.0
                                || self.state.position >= Bend(MAX_BEND).limit() =>
                        {
                            self.state.velocity = 0.0;
                            Movement::Rest
                        }
                        other => other,
                    },
                };
            }
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
    fn vu_step_has_specified_rise_overshoot_and_continuous_velocity() {
        let mut motion = Motion::new(Mode::Vu, PeakMovement::default());
        motion.advance(0.5, 0.3, ShutterFrames::default());
        assert!((motion.state.position / 0.5 - 0.99).abs() < 1e-7);
        let velocity = motion.state.velocity;
        let exposure = motion.advance(0.5, 0.3, ShutterFrames::default());
        assert!((exposure.maximum / 0.5 - 1.0125).abs() < 1e-7);
        motion.advance(0.0, 0.0, ShutterFrames::default());
        assert!(velocity > 0.0);
        let before = motion.state;
        motion.advance(0.0, 0.00001, ShutterFrames::default());
        assert!((motion.state.position - before.position).abs() < 1e-4);
        assert!((motion.state.velocity - before.velocity).abs() < 0.01);
        run(&mut motion, 0.0, 1.0, 60);
        assert!(motion.state.position >= 0.0);
    }

    fn tone_response(mode: Mode, samplerate: usize, fps: usize, burst_seconds: f64) -> (f64, f64) {
        let scale = Scale::new();
        let mut processor = crate::processor::Processor::new(1.0, mode);
        processor.set_samplerate(samplerate);
        let mut motion = Motion::new(mode, PeakMovement::default());
        let mut maximum: f64 = 0.0;
        for frame in 0..fps * 4 {
            let input: Vec<_> = (frame * samplerate / fps..(frame + 1) * samplerate / fps)
                .map(|sample| {
                    let time = sample as f64 / samplerate as f64;
                    let value = if time < burst_seconds {
                        0.5 * (TAU * 5000.0 * time).sin()
                    } else {
                        0.0
                    };
                    [value as f32; 2]
                })
                .collect();
            processor.consume_buf(&input);
            let drives = processor.frame_drives(1.0 / fps as f64);
            let exposure = motion.advance_path(
                drives
                    .iter()
                    .map(|drive| (mode.position(&scale, drive.levels[0]), drive.seconds)),
                ShutterFrames::try_from(1.0).unwrap(),
            );
            maximum = maximum.max(exposure.maximum);
        }
        (maximum, motion.state.position)
    }

    fn db_from_position(position: f64) -> f64 {
        let scale = Scale::new();
        position.powf(1.0 / f64::from(scale.bend)) * (f64::from(scale.negative_db_range) + 6.0)
            - f64::from(scale.negative_db_range)
    }

    #[test]
    fn peak_movements_follow_sifams_published_physical_responses() {
        // Approximate deflections from both curves in the Sifam Vintage FR
        // datasheet, linked in docs/meter-measurements.md.
        let curves = [
            (
                PeakMovement::FullSpec,
                [
                    (0.02, 0.13),
                    (0.04, 0.41),
                    (0.05, 0.56),
                    (0.06, 0.72),
                    (0.08, 0.89),
                    (0.10, 0.99),
                    (0.14, 1.055),
                    (0.20, 1.01),
                    (0.25, 1.0),
                ],
                0.04,
            ),
            (
                PeakMovement::VintageFr,
                [
                    (0.02, 0.044),
                    (0.04, 0.196),
                    (0.05, 0.281),
                    (0.06, 0.360),
                    (0.08, 0.525),
                    (0.10, 0.679),
                    (0.15, 0.906),
                    (0.24, 1.025),
                    (0.40, 1.0),
                ],
                0.025,
            ),
        ];
        for (movement, reference, tolerance) in curves {
            for mode in [Mode::Ppm, Mode::SamplePeak] {
                let mut motion = Motion::new(mode, movement);
                let mut previous = 0.0;
                for (time, deflection) in reference {
                    motion.advance(0.5, (time - previous) as f32, ShutterFrames::default());
                    let actual = motion.state.position / 0.5;
                    assert!(
                        (actual - deflection).abs() < tolerance,
                        "{mode:?}/{movement:?} at {time}s: {actual} vs {deflection}"
                    );
                    previous = time;
                }
            }
        }
    }

    #[test]
    fn switching_peak_movements_preserves_pose_momentum_and_exposure() {
        let mut motion = Motion::new(Mode::Ppm, PeakMovement::FullSpec);
        motion.advance(0.5, 0.04, ShutterFrames::default());
        let before = motion.state;
        motion.set_peak_movement(PeakMovement::VintageFr);
        assert_eq!(motion.state.position, before.position);
        assert_eq!(motion.state.velocity, before.velocity);
        let exposure = motion.advance(0.5, 0.001, ShutterFrames::default());
        assert!(motion.state.position > before.position);
        assert!((motion.state.position - before.position).abs() < 0.01);
        assert_eq!(
            exposure.minimum, 0.0,
            "shutter still includes the first drive's motion"
        );
        assert_eq!(motion.bend, Bend::default());
    }

    #[test]
    fn ppm_bursts_and_return_survive_the_complete_physical_needle_path() {
        for samplerate in [44100, 48000, 96000] {
            for fps in [15, 60, 240] {
                // The reference is the settled reading, not startup overswing.
                let (_, steady) = tone_response(Mode::Ppm, samplerate, fps, 4.0);
                assert!((db_from_position(steady) + 6.0206).abs() < 0.6);
                for (seconds, expected_db, tolerance) in
                    [(0.0015, -9.0, 1.0), (0.005, -4.0, 0.6), (0.010, -2.0, 0.6)]
                {
                    let (burst, _) = tone_response(Mode::Ppm, samplerate, fps, seconds);
                    let under_read = db_from_position(burst) - db_from_position(steady);
                    assert!(
                        (under_read - expected_db).abs() < tolerance,
                        "{samplerate}/{fps}, {seconds}s burst: {under_read} dB vs {expected_db}"
                    );
                }
                // A longer drive can show the movement's published overswing.
                let (long_burst, _) = tone_response(Mode::Ppm, samplerate, fps, 0.1);
                let overswing = db_from_position(long_burst) - db_from_position(steady);
                assert!((0.0..1.5).contains(&overswing));
                let (_, returned) = tone_response(Mode::Ppm, samplerate, fps, 1.0);
                let fall = db_from_position(returned) - db_from_position(steady);
                assert!((fall + 26.0).abs() < 0.5);
            }
        }
    }

    #[test]
    fn vu_short_bursts_agree_across_audio_and_display_rates() {
        let (reference, _) = tone_response(Mode::Vu, 48000, 240, 0.01);
        for samplerate in [44100, 48000, 96000] {
            for fps in [15, 60, 240] {
                let (actual, returned) = tone_response(Mode::Vu, samplerate, fps, 0.01);
                assert!(
                    (actual / reference - 1.0).abs() < 0.015,
                    "{samplerate}/{fps}: {actual} vs {reference}"
                );
                assert!(returned < 1e-6);
            }
        }
    }

    #[test]
    fn hard_hits_yield_over_time_and_repeated_hits_accumulate_damage() {
        let mut motion = Motion::new(Mode::Rms, PeakMovement::default());
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
        let mut gentle = Motion::new(Mode::Rms, PeakMovement::default());
        for step in 0..3000 {
            gentle.advance(step as f32 / 2500.0, 1.0 / 240.0, ShutterFrames::default());
        }
        assert_eq!(gentle.bend, Bend::default());
        near(gentle.state.position, 1.0);
        let mut reference = Motion::new(Mode::Rms, PeakMovement::default());
        run(&mut reference, 1.2, 1.0, 240);
        for fps in [15, 30, 60, 120] {
            let mut motion = Motion::new(Mode::Rms, PeakMovement::default());
            run(&mut motion, 1.2, 1.0, fps);
            near(motion.bend.0, reference.bend.0);
            near(motion.state.position, reference.state.position);
        }
        let mut extreme = Motion::new(Mode::Rms, PeakMovement::default());
        run(&mut extreme, 100.0, 1.0, 15);
        near(extreme.bend.0, MAX_BEND);
        near(extreme.state.position, Bend(MAX_BEND).limit());
    }

    #[test]
    fn underdamped_movements_find_contacts_and_preserve_shutter_extrema() {
        for mode in [Mode::Vu, Mode::Ppm, Mode::SamplePeak] {
            let movements: &[PeakMovement] = if mode.uses_peak_movement() {
                &[PeakMovement::FullSpec, PeakMovement::VintageFr]
            } else {
                &[PeakMovement::FullSpec]
            };
            for &movement in movements {
                let mut reference = Motion::new(mode, movement);
                run(&mut reference, 1.2, 1.0, 240);
                for fps in [15, 30, 60, 120] {
                    let mut motion = Motion::new(mode, movement);
                    run(&mut motion, 1.2, 1.0, fps);
                    near(motion.bend.0, reference.bend.0);
                    near(motion.state.position, reference.state.position);
                }
                let mut motion = Motion::new(mode, movement);
                let exposure = motion.advance(0.5, 1.0, ShutterFrames::default());
                assert!(exposure.maximum > 0.5);
                assert!(exposure.minimum >= 0.0);
                assert!((exposure.minimum..=exposure.maximum).contains(&exposure.average));
                let return_exposure = motion.advance(0.0, 1.0, ShutterFrames::default());
                assert!(return_exposure.minimum >= -1e-12);
                assert!(motion.state.position >= 0.0);
            }
        }
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
        let mut one_frame = Motion::new(Mode::Rms, PeakMovement::default());
        let mut two_frames = Motion::new(Mode::Rms, PeakMovement::default());
        let shutter = ShutterFrames::try_from(2.0).unwrap();
        one_frame.advance(1.0, 0.01, ShutterFrames::try_from(1.0).unwrap());
        two_frames.advance(1.0, 0.01, shutter);
        let short = one_frame.advance(1.0, 0.02, ShutterFrames::try_from(1.0).unwrap());
        let long = two_frames.advance(1.0, 0.02, shutter);
        let mut uninterrupted = Motion::new(Mode::Rms, PeakMovement::default());
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
        let mut motion = Motion::new(Mode::Rms, PeakMovement::default());
        for _ in 0..3 {
            motion.advance(1.0, 0.01, ShutterFrames::try_from(1.0).unwrap());
        }
        let actual = motion.advance(1.0, 0.01, ShutterFrames::try_from(2.5).unwrap());
        let mut expected_motion = Motion::new(Mode::Rms, PeakMovement::default());
        expected_motion.advance(1.0, 0.015, ShutterFrames::try_from(1.0).unwrap());
        let expected = expected_motion.advance(1.0, 0.025, ShutterFrames::try_from(1.0).unwrap());
        near(actual.minimum, expected.minimum);
        near(actual.maximum, expected.maximum);
        near(actual.average, expected.average);
    }

    #[test]
    fn shutter_keeps_reversals_and_expires_old_motion() {
        let mut motion = Motion::new(Mode::Rms, PeakMovement::default());
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
