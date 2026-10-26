use std::{
    fmt,
    time::{Duration, Instant},
};

use strum::VariantArray;
use winit::dpi::PhysicalSize;

const BACKGROUND_INTERVAL: Duration = Duration::from_millis(50);

/// The actual wall-clock interval of a displayed frame.
#[derive(Clone, Copy)]
pub struct FrameTime {
    start: Instant,
    end: Instant,
}

impl FrameTime {
    pub fn new(start: Instant, end: Instant) -> Self {
        assert!(end >= start, "display frame must be ordered");
        Self { start, end }
    }

    pub fn start(self) -> Instant {
        self.start
    }

    pub fn end(self) -> Instant {
        self.end
    }

    pub fn seconds(self) -> f32 {
        (self.end - self.start).as_secs_f32()
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    strum::VariantArray,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum FrameRateLimit {
    Fps15,
    Fps30,
    Fps48,
    Fps55,
    Fps60,
    Fps72,
    Fps90,
    #[default]
    Fps120,
    Fps180,
    Fps240,
    VSync,
}

impl FrameRateLimit {
    fn fps(self) -> Option<u64> {
        Some(match self {
            Self::Fps15 => 15,
            Self::Fps30 => 30,
            Self::Fps48 => 48,
            Self::Fps55 => 55,
            Self::Fps60 => 60,
            Self::Fps72 => 72,
            Self::Fps90 => 90,
            Self::Fps120 => 120,
            Self::Fps180 => 180,
            Self::Fps240 => 240,
            Self::VSync => return None,
        })
    }

    pub fn interval(self) -> Option<Duration> {
        self.fps()
            .map(|fps| Duration::from_nanos(1_000_000_000_u64.div_ceil(fps)))
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        match adjustment {
            Adjustment::Decrease => Self::VARIANTS
                .iter()
                .copied()
                .take_while(|limit| *limit != self)
                .last()
                .unwrap_or(Self::VSync),
            Adjustment::Increase => Self::VARIANTS
                .iter()
                .copied()
                .skip_while(|limit| *limit != self)
                .nth(1)
                .unwrap_or(Self::Fps15),
        }
    }
}

impl fmt::Display for FrameRateLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.fps() {
            Some(fps) => write!(f, "{fps}"),
            None => f.write_str("VSync"),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Adjustment {
    Decrease,
    Increase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceArea {
    Empty,
    NonEmpty,
}

impl From<PhysicalSize<u32>> for SurfaceArea {
    fn from(size: PhysicalSize<u32>) -> Self {
        if size.width == 0 || size.height == 0 {
            Self::Empty
        } else {
            Self::NonEmpty
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occlusion {
    Visible,
    Occluded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    Visible,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedrawPermit {
    Idle,
    Requested,
}

/// Retains the deadline phase and skips expired slots without catch-up bursts.
pub struct FrameSchedule {
    area: SurfaceArea,
    occlusion: Occlusion,
    retry_at: Option<Instant>,
    next_due: Instant,
    permit: RedrawPermit,
}

impl FrameSchedule {
    pub fn new(now: Instant) -> Self {
        Self {
            area: SurfaceArea::Empty,
            occlusion: Occlusion::Visible,
            retry_at: None,
            next_due: now,
            permit: RedrawPermit::Idle,
        }
    }

    pub fn limit_changed(&mut self, now: Instant) {
        self.next_due = now;
    }

    pub fn kind(&self, now: Instant) -> FrameKind {
        let retrying = self.retry_at.is_some_and(|at| now < at);
        match (self.area, self.occlusion, retrying) {
            (SurfaceArea::NonEmpty, Occlusion::Visible, false) => FrameKind::Visible,
            _ => FrameKind::Background,
        }
    }

    pub fn is_due(&self, now: Instant) -> bool {
        now >= self.next_due
    }

    pub fn deadline(&self, now: Instant) -> Instant {
        self.retry_at
            .filter(|retry| *retry > now)
            .map_or(self.next_due, |retry| self.next_due.min(retry))
    }

    pub fn resized(&mut self, area: SurfaceArea, now: Instant) {
        self.area = area;
        self.wake_if_drawable(now);
    }

    pub fn occlusion_changed(&mut self, occlusion: Occlusion, now: Instant) {
        self.occlusion = occlusion;
        self.wake_if_drawable(now);
    }

    pub fn request_visible_frame(&mut self, now: Instant, limit: FrameRateLimit) {
        assert!(
            self.kind(now) == FrameKind::Visible,
            "only drawable windows request visible frames"
        );
        assert!(
            self.is_due(now),
            "timer frame requested before its deadline"
        );
        self.retry_at = None;
        self.next_due = limit.interval().map_or(now, |interval| {
            let remainder = now.duration_since(self.next_due).as_nanos() % interval.as_nanos();
            now + interval - Duration::from_nanos(remainder as u64)
        });
        self.permit = RedrawPermit::Requested;
    }

    pub fn background_tick_finished(&mut self, now: Instant) {
        self.next_due = now + BACKGROUND_INTERVAL;
    }

    pub fn surface_unavailable(&mut self, now: Instant) {
        let retry_at = now + BACKGROUND_INTERVAL;
        self.retry_at = Some(retry_at);
        self.next_due = retry_at;
        self.permit = RedrawPermit::Idle;
    }

    pub fn take_redraw_permit(&mut self) -> RedrawPermit {
        std::mem::replace(&mut self.permit, RedrawPermit::Idle)
    }

    pub fn redraw_is_pending(&self) -> bool {
        self.permit == RedrawPermit::Requested
    }

    fn wake_if_drawable(&mut self, now: Instant) {
        if self.area == SurfaceArea::NonEmpty && self.occlusion == Occlusion::Visible {
            self.retry_at = None;
        } else {
            self.permit = RedrawPermit::Idle;
        }
        self.next_due = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_cycle_in_both_directions_and_leave_vsync_unlimited() {
        let mut choice = FrameRateLimit::Fps15;
        let expected = [
            "15", "30", "48", "55", "60", "72", "90", "120", "180", "240", "VSync",
        ];
        for label in expected {
            assert_eq!(choice.to_string(), label);
            let next = choice.adjusted(Adjustment::Increase);
            assert_eq!(next.adjusted(Adjustment::Decrease), choice);
            choice = next;
        }
        assert_eq!(choice, FrameRateLimit::Fps15);
        assert!(FrameRateLimit::VSync.interval().is_none());
    }

    #[test]
    fn late_wakes_preserve_every_limited_cadence_without_catch_up() {
        for &limit in FrameRateLimit::VARIANTS {
            let Some(interval) = limit.interval() else {
                continue;
            };
            let start = Instant::now();
            let mut schedule = FrameSchedule::new(start);
            schedule.resized(SurfaceArea::NonEmpty, start);
            assert_eq!(schedule.take_redraw_permit(), RedrawPermit::Idle);
            schedule.request_visible_frame(start, limit);
            assert_eq!(schedule.take_redraw_permit(), RedrawPermit::Requested);
            assert_eq!(schedule.take_redraw_permit(), RedrawPermit::Idle);
            for frame in 1..1000 {
                let wake = start + interval * frame + Duration::from_micros(800);
                schedule.request_visible_frame(wake, limit);
                assert_eq!(schedule.deadline(wake), start + interval * (frame + 1));
                assert!(!schedule.is_due(wake));
            }
        }
    }

    #[test]
    fn hidden_and_unavailable_surfaces_back_off_and_restore_promptly() {
        let start = Instant::now();
        let mut schedule = FrameSchedule::new(start);
        schedule.resized(SurfaceArea::NonEmpty, start);
        schedule.request_visible_frame(start, FrameRateLimit::VSync);
        schedule.occlusion_changed(Occlusion::Occluded, start);
        assert_eq!(schedule.kind(start), FrameKind::Background);
        assert_eq!(schedule.take_redraw_permit(), RedrawPermit::Idle);
        schedule.background_tick_finished(start);
        assert_eq!(schedule.deadline(start), start + BACKGROUND_INTERVAL);
        let restored = start + Duration::from_millis(10);
        schedule.occlusion_changed(Occlusion::Visible, restored);
        assert!(schedule.is_due(restored));
        schedule.surface_unavailable(restored);
        assert_eq!(schedule.kind(restored), FrameKind::Background);
        assert_eq!(
            schedule.kind(restored + BACKGROUND_INTERVAL),
            FrameKind::Visible
        );
        let changed = restored + BACKGROUND_INTERVAL;
        schedule.limit_changed(changed);
        assert!(schedule.is_due(changed));
        schedule.request_visible_frame(changed, FrameRateLimit::Fps240);
        assert_eq!(
            schedule.deadline(changed),
            changed + FrameRateLimit::Fps240.interval().unwrap()
        );
    }
}
