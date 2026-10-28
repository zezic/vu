use std::time::{Duration, Instant};

use super::timeline::{AudioClock, AudioInterval, AudioTime};

/// Maps redraw times onto the captured sample timeline. Buffering starts only
/// when the source timestamps cannot supply the requested frame; ready future
/// data is sampled directly. A buffered cursor keeps one capture block in
/// reserve.
pub struct AudioCursor {
    samplerate: usize,
    bin_samples: usize,
    samples: u64,
    bin_count: usize,
    clock: Option<AudioClock>,
    read: ReadMode,
}

impl AudioCursor {
    pub fn new() -> Self {
        Self {
            samplerate: 48_000,
            bin_samples: 48,
            samples: 0,
            bin_count: 0,
            clock: None,
            read: ReadMode::default(),
        }
    }

    pub fn set_samplerate(&mut self, samplerate: usize) {
        assert!(samplerate > 0, "sample rate must be positive");
        self.samplerate = samplerate;
        self.bin_samples = (samplerate / 1000).max(1);
        self.reset();
    }

    fn reset(&mut self) {
        self.samples = 0;
        self.bin_count = 0;
        self.clock = None;
        self.read = ReadMode::default();
    }

    /// A capture buffer of `frames` stereo frames ending at `captured_at`.
    pub fn push(&mut self, frames: usize, captured_at: Instant) {
        if frames == 0 {
            return;
        }
        self.samples += frames as u64;
        self.bin_count = (self.bin_count + frames) % self.bin_samples;
        self.clock = Some(AudioClock {
            position: self.audio_time(),
            at: captured_at + Duration::from_secs_f64(frames as f64 / self.samplerate as f64),
        });
    }

    fn audio_time(&self) -> AudioTime {
        AudioTime::from_samples(self.samples, self.samplerate)
    }

    /// Completed 1 ms bins from the most recent capture block. The latest
    /// complete bin bounds the data the display may read ahead into.
    fn available_interval(&self) -> AudioInterval {
        let end = AudioTime::from_samples(self.samples - self.bin_count as u64, self.samplerate);
        let step = 1.0 / self.bin_samples as f64;
        AudioInterval::new(end.shifted(-step), end)
    }

    fn requested_interval(&self, start: Instant, end: Instant) -> AudioInterval {
        self.clock.map_or_else(
            || AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO),
            |clock| AudioInterval::new(clock.position_at(start), clock.position_at(end)),
        )
    }

    /// The audio interval covered by one displayed frame.
    pub fn frame(&mut self, start: Instant, end: Instant) -> AudioInterval {
        let requested = self.requested_interval(start, end);
        let available = self.available_interval();
        self.read.frame(start, end, requested, available)
    }
}

impl Default for AudioCursor {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
enum ReadMode {
    #[default]
    Uninitialized,
    FollowingSource,
    Buffered(AudioClock),
    Waiting(AudioTime),
}

impl ReadMode {
    fn frame(
        &mut self,
        display_start: Instant,
        display_end: Instant,
        requested: AudioInterval,
        available: AudioInterval,
    ) -> AudioInterval {
        let seconds = display_end.duration_since(display_start).as_secs_f64();
        match *self {
            Self::Uninitialized if available.end() == AudioTime::ZERO => {
                AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO)
            }
            Self::Uninitialized if requested.end() <= available.end() => {
                *self = Self::FollowingSource;
                requested
            }
            Self::Uninitialized => {
                let clock = AudioClock {
                    position: available.start(),
                    at: display_end,
                };
                *self = Self::Buffered(clock);
                AudioInterval::new(clock.position_at(display_start), clock.position)
            }
            Self::FollowingSource if requested.end() <= available.end() => requested,
            Self::FollowingSource => {
                *self = Self::Waiting(available.end());
                AudioInterval::new(available.end(), available.end())
            }
            Self::Buffered(mut clock) => {
                let end = clock.position_at(display_end);
                if end > available.end() {
                    *self = Self::Waiting(available.end());
                    return AudioInterval::new(available.end(), available.end());
                }
                // Keep capture/host clock drift or a stalled UI from growing
                // into a replay backlog. Rejoin the newest buffered block.
                if available.start() > end
                    && available.start().seconds_since(end) > available.seconds() + seconds
                {
                    clock = AudioClock {
                        position: available.start(),
                        at: display_end,
                    };
                    *self = Self::Buffered(clock);
                }
                AudioInterval::new(clock.position_at(display_start), end)
            }
            Self::Waiting(at) if available.end() <= at => AudioInterval::new(at, at),
            Self::Waiting(at) => {
                let position = available.start().max(at);
                *self = Self::Buffered(AudioClock {
                    position,
                    at: display_end,
                });
                AudioInterval::new(position, position)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor() -> (AudioCursor, Instant) {
        let start = Instant::now();
        let mut cursor = AudioCursor::new();
        cursor.set_samplerate(1000);
        (cursor, start)
    }

    #[test]
    fn waits_for_the_first_capture_buffer() {
        let (mut cursor, start) = cursor();
        let frame = cursor.frame(start, start + Duration::from_millis(10));
        assert_eq!(frame.start(), AudioTime::ZERO);
        assert_eq!(frame.end(), AudioTime::ZERO);
    }

    #[test]
    fn follows_complete_audio_and_waits_when_the_frame_is_ahead() {
        let (mut cursor, start) = cursor();
        cursor.push(10, start);
        let first = cursor.frame(start, start + Duration::from_millis(10));
        assert_eq!(first.start(), AudioTime::ZERO);
        assert_eq!(first.end(), AudioTime::from_samples(10, 1000));

        let ahead = cursor.frame(
            start + Duration::from_millis(10),
            start + Duration::from_millis(20),
        );
        assert_eq!(ahead.start(), AudioTime::from_samples(10, 1000));
        assert_eq!(ahead.end(), ahead.start());
    }

    #[test]
    fn a_sample_rate_change_discards_the_old_timeline() {
        let (mut cursor, start) = cursor();
        cursor.push(10, start);
        cursor.set_samplerate(48_000);
        let frame = cursor.frame(start, start + Duration::from_millis(10));
        assert_eq!(frame.start(), AudioTime::ZERO);
        assert_eq!(frame.end(), AudioTime::ZERO);
    }
}
