use std::time::{Duration, Instant};

use vello::kurbo::Point;

const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClickKind {
    Single,
    Double,
}

struct Click {
    at: Instant,
    point: Point,
}

/// Recognizes non-overlapping click pairs within a physical-pixel radius.
#[derive(Default)]
pub(super) struct ClickSequence {
    pending: Option<Click>,
}

impl ClickSequence {
    pub(super) fn record(&mut self, point: Point, now: Instant, radius: f64) -> ClickKind {
        assert!(
            radius.is_finite() && radius > 0.0,
            "click radius must be positive and finite"
        );
        if self.pending.take().is_some_and(|previous| {
            now.duration_since(previous.at) <= DOUBLE_CLICK_INTERVAL
                && previous.point.distance(point) <= radius
        }) {
            ClickKind::Double
        } else {
            self.pending = Some(Click { at: now, point });
            ClickKind::Single
        }
    }

    pub(super) fn reset(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_clicks_form_consumed_pairs() {
        let now = Instant::now();
        let mut sequence = ClickSequence::default();
        assert_eq!(sequence.record(Point::ZERO, now, 6.0), ClickKind::Single);
        let second = now + DOUBLE_CLICK_INTERVAL;
        let point = Point::new(6.0, 0.0);
        assert_eq!(sequence.record(point, second, 6.0), ClickKind::Double);
        assert_eq!(
            sequence.record(point, second + Duration::from_millis(1), 6.0),
            ClickKind::Single
        );
        assert_eq!(
            sequence.record(point, second + Duration::from_millis(2), 6.0),
            ClickKind::Double
        );
    }

    #[test]
    fn expired_or_remote_click_starts_a_new_pair() {
        let now = Instant::now();
        let mut sequence = ClickSequence::default();
        assert_eq!(sequence.record(Point::ZERO, now, 6.0), ClickKind::Single);
        let late = now + DOUBLE_CLICK_INTERVAL + Duration::from_millis(1);
        assert_eq!(sequence.record(Point::ZERO, late, 6.0), ClickKind::Single);
        assert_eq!(
            sequence.record(Point::ZERO, late + Duration::from_millis(1), 6.0),
            ClickKind::Double
        );
        assert_eq!(
            sequence.record(Point::ZERO, late + Duration::from_millis(2), 6.0),
            ClickKind::Single
        );
        let remote = Point::new(6.1, 0.0);
        assert_eq!(
            sequence.record(remote, late + Duration::from_millis(3), 6.0),
            ClickKind::Single
        );
        assert_eq!(
            sequence.record(remote, late + Duration::from_millis(4), 6.0),
            ClickKind::Double
        );
    }

    #[test]
    fn reset_and_caller_radius_define_the_next_pair() {
        let now = Instant::now();
        let mut sequence = ClickSequence::default();
        sequence.record(Point::ZERO, now, 12.0);
        sequence.reset();
        assert_eq!(
            sequence.record(Point::ZERO, now + Duration::from_millis(1), 12.0),
            ClickKind::Single
        );
        assert_eq!(
            sequence.record(Point::new(11.0, 0.0), now + Duration::from_millis(2), 12.0),
            ClickKind::Double
        );
    }
}
