use std::time::{Duration, Instant};

use cpal::{InputStreamTimestamp, StreamInstant};

/// Seconds on a stream's sample timeline, independent of callback and redraw cadence.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct AudioTime(f64);

impl AudioTime {
    pub const ZERO: Self = Self(0.0);

    pub fn from_samples(samples: u64, samplerate: usize) -> Self {
        assert!(samplerate > 0, "sample rate must be positive");
        Self(samples as f64 / samplerate as f64)
    }

    pub fn seconds_since(self, earlier: Self) -> f64 {
        assert!(self >= earlier, "audio time must advance monotonically");
        self.0 - earlier.0
    }

    pub fn shifted(self, seconds: f64) -> Self {
        assert!(seconds.is_finite(), "audio time offset must be finite");
        Self((self.0 + seconds).max(0.0))
    }

    pub fn min(self, other: Self) -> Self {
        Self(self.0.min(other.0))
    }

    pub fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AudioInterval {
    start: AudioTime,
    end: AudioTime,
}

impl AudioInterval {
    pub fn new(start: AudioTime, end: AudioTime) -> Self {
        assert!(end >= start, "audio interval must be ordered");
        Self { start, end }
    }

    pub fn start(self) -> AudioTime {
        self.start
    }

    pub fn end(self) -> AudioTime {
        self.end
    }

    pub fn seconds(self) -> f64 {
        self.end.seconds_since(self.start)
    }
}

/// Correlates the native stream clock with the application clock once per stream.
/// Subsequent buffers use native capture timestamps, not their delivery times.
#[derive(Debug)]
pub struct CaptureClock {
    native: StreamInstant,
    local: Instant,
}

impl CaptureClock {
    pub fn new(native: StreamInstant, local: Instant) -> Self {
        Self { native, local }
    }

    pub fn capture_at(&self, timestamp: InputStreamTimestamp) -> Instant {
        self.local_at(timestamp.capture)
    }
    pub fn playback_at(&self, timestamp: cpal::OutputStreamTimestamp) -> Instant {
        self.local_at(timestamp.playback)
    }
    fn local_at(&self, native: StreamInstant) -> Instant {
        if native >= self.native {
            self.local + native.duration_since(self.native)
        } else {
            self.local - self.native.duration_since(native)
        }
    }
}

pub(super) enum CaptureContinuity {
    Continuous,
    Discontinuous,
}

/// Detect missing or restarted capture time without confusing late delivery with
/// missing samples. Native timestamps may differ by small scheduling jitter.
#[derive(Default)]
pub(super) struct CaptureSequence {
    next: Option<StreamInstant>,
}

impl CaptureSequence {
    pub fn observe(
        &mut self,
        capture: StreamInstant,
        frames: usize,
        samplerate: usize,
    ) -> CaptureContinuity {
        let continuity = match self.next {
            Some(next)
                if capture.max(next).duration_since(capture.min(next))
                    > Duration::from_millis(2) =>
            {
                CaptureContinuity::Discontinuous
            }
            _ => CaptureContinuity::Continuous,
        };
        self.next = Some(capture + Duration::from_secs_f64(frames as f64 / samplerate as f64));
        continuity
    }
}

/// Correlates a sample position with monotonic host time.
#[derive(Clone, Copy)]
pub struct AudioClock {
    pub position: AudioTime,
    pub at: Instant,
}

impl AudioClock {
    /// Convert clock coordinates; data coverage is checked by the consumer.
    pub fn position_at(self, now: Instant) -> AudioTime {
        if now >= self.at {
            self.position.shifted((now - self.at).as_secs_f64())
        } else {
            self.position.shifted(-(self.at - now).as_secs_f64())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_clock_preserves_native_time_despite_late_callbacks() {
        let now = Instant::now();
        let first = InputStreamTimestamp {
            callback: StreamInstant::from_secs_f64(10.0),
            capture: StreamInstant::from_secs_f64(9.99),
        };
        let clock = CaptureClock::new(first.callback, now);
        assert_eq!(clock.capture_at(first), now - Duration::from_millis(10));
        let later = InputStreamTimestamp {
            callback: StreamInstant::from_secs_f64(10.1),
            capture: StreamInstant::from_secs_f64(10.01),
        };
        assert_eq!(clock.capture_at(later), now + Duration::from_millis(10));
    }

    #[test]
    fn capture_sequence_distinguishes_jitter_from_missing_audio_and_restarts() {
        let mut sequence = CaptureSequence::default();
        assert!(matches!(
            sequence.observe(StreamInstant::from_millis(10), 480, 48000),
            CaptureContinuity::Continuous
        ));
        assert!(matches!(
            sequence.observe(StreamInstant::from_micros(20001), 480, 48000),
            CaptureContinuity::Continuous
        ));
        assert!(matches!(
            sequence.observe(StreamInstant::from_millis(80), 480, 48000),
            CaptureContinuity::Discontinuous
        ));
        assert!(matches!(
            sequence.observe(StreamInstant::from_millis(90), 480, 48000),
            CaptureContinuity::Continuous
        ));
        assert!(matches!(
            sequence.observe(StreamInstant::from_millis(50), 480, 48000),
            CaptureContinuity::Discontinuous
        ));
    }
}
