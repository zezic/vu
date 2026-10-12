use std::collections::VecDeque;

use crate::measurement::Mode;

/// A detector drive held for a span of audio time.
#[derive(Clone, Copy)]
pub struct Drive {
    pub levels: [f32; 2],
    pub seconds: f64,
}

enum Detector {
    Vu,
    Ppm {
        slow: [f64; 2],
        fast: [f64; 2],
        slow_attack: f64,
        fast_attack: f64,
        release: f64,
    },
    Rms {
        squares: VecDeque<[f64; 2]>,
        sums: [f64; 2],
        window: usize,
    },
    SamplePeak {
        envelope: [f64; 2],
        hold: [usize; 2],
        hold_samples: usize,
        release: f64,
    },
}

impl Detector {
    fn new(mode: Mode, samplerate: usize) -> Self {
        let dt = 1.0 / samplerate as f64;
        match mode {
            Mode::Vu => Self::Vu,
            Mode::Ppm => Self::Ppm {
                slow: [0.0; 2],
                fast: [0.0; 2],
                // Parallel IEC Type II charging rates; see meter-measurements.md.
                slow_attack: 1.0 - (-200.0 * dt).exp(),
                fast_attack: 1.0 - (-860.0 * dt).exp(),
                release: 10.0_f64.powf(-26.0 * dt / 60.0),
            },
            Mode::Rms => Self::Rms {
                squares: VecDeque::new(),
                sums: [0.0; 2],
                window: (samplerate as f64 * 0.3).round().max(1.0) as usize,
            },
            Mode::SamplePeak => Self::SamplePeak {
                envelope: [0.0; 2],
                hold: [0; 2],
                hold_samples: (samplerate as f64 * 0.1).round() as usize,
                release: 10.0_f64.powf(-dt),
            },
        }
    }

    fn sample(&mut self, input: [f64; 2]) -> [f64; 2] {
        match self {
            Self::Vu => {
                // Sine calibration: mean(|sin|) * pi/(2*sqrt(2)) = sine RMS.
                input.map(|sample| sample.abs() * std::f64::consts::PI / (2.0 * 2.0_f64.sqrt()))
            }
            Self::Ppm {
                slow,
                fast,
                slow_attack,
                fast_attack,
                release,
            } => {
                for (envelope, attack) in [(&mut *slow, *slow_attack), (&mut *fast, *fast_attack)] {
                    for channel in 0..2 {
                        envelope[channel] *= *release;
                        let sample = input[channel].abs();
                        if sample > envelope[channel] {
                            envelope[channel] += attack * (sample - envelope[channel]);
                        }
                    }
                }
                // Sine calibration of the summed slow and fast rectifier envelopes.
                std::array::from_fn(|channel| 0.5141 * (slow[channel] + fast[channel]))
            }
            Self::Rms {
                squares,
                sums,
                window,
            } => {
                let outgoing = if squares.len() == *window {
                    squares.pop_front().expect("a full RMS window has a head")
                } else {
                    [0.0; 2]
                };
                let incoming = input.map(|sample| sample * sample);
                squares.push_back(incoming);
                for channel in 0..2 {
                    sums[channel] += incoming[channel] - outgoing[channel];
                }
                // Roundoff in a running sum can leave a tiny negative at silence.
                sums.map(|sum| (sum.max(0.0) / *window as f64).sqrt())
            }
            Self::SamplePeak {
                envelope,
                hold,
                hold_samples,
                release,
            } => {
                for channel in 0..2 {
                    let sample = input[channel].abs();
                    if sample >= envelope[channel] {
                        envelope[channel] = sample;
                        hold[channel] = *hold_samples;
                    } else if hold[channel] > 0 {
                        hold[channel] -= 1;
                    } else {
                        envelope[channel] = (envelope[channel] * *release).max(sample);
                    }
                }
                *envelope
            }
        }
    }
}

pub struct Processor {
    mode: Mode,
    detector: Detector,
    samplerate: usize,
    bin_sum: [f64; 2],
    bin_count: usize,
    pending: VecDeque<Drive>,
    frame: Vec<Drive>,
    last_levels: [f32; 2],
    pub preamp: f32,
}

impl Processor {
    pub fn new(preamp: f32, mode: Mode) -> Self {
        Self {
            mode,
            detector: Detector::new(mode, 44100),
            samplerate: 44100,
            bin_sum: [0.0; 2],
            bin_count: 0,
            pending: VecDeque::new(),
            frame: Vec::new(),
            last_levels: [0.0; 2],
            preamp,
        }
    }

    fn reset(&mut self) {
        self.detector = Detector::new(self.mode, self.samplerate);
        self.bin_sum = [0.0; 2];
        self.bin_count = 0;
        self.pending.clear();
        self.frame.clear();
        self.last_levels = [0.0; 2];
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.reset();
    }

    pub fn set_samplerate(&mut self, samplerate: usize) {
        assert!(samplerate > 0, "sample rate must be positive");
        self.samplerate = samplerate;
        self.reset();
    }

    fn flush_bin(&mut self) {
        assert!(
            self.bin_count > 0,
            "only a populated drive bin can be flushed"
        );
        self.pending.push_back(Drive {
            levels: self.bin_sum.map(|sum| (sum / self.bin_count as f64) as f32),
            seconds: self.bin_count as f64 / self.samplerate as f64,
        });
        self.bin_sum = [0.0; 2];
        self.bin_count = 0;
    }

    pub fn consume_buf(&mut self, buf: &[[f32; 2]]) {
        let bin_samples = (self.samplerate / 1000).max(1);
        for pair in buf {
            assert!(
                pair.iter().all(|sample| sample.is_finite()),
                "audio samples must be finite"
            );
            let levels = self.detector.sample(pair.map(f64::from));
            for (sum, level) in self.bin_sum.iter_mut().zip(levels) {
                *sum += level;
            }
            self.bin_count += 1;
            if self.bin_count == bin_samples {
                self.flush_bin();
            }
        }
        // A paused display resumes with recent audio rather than replaying a backlog.
        let capacity = (self.samplerate as f64 * 0.3 / bin_samples as f64).ceil() as usize;
        while self.pending.len() > capacity {
            self.pending.pop_front();
        }
    }

    /// Consume audio-time drives for one display frame, retaining fractional bins.
    /// The latest drive covers any gap between delivery of capture buffers.
    pub fn frame_drives(&mut self, seconds: f64) -> &[Drive] {
        assert!(
            seconds.is_finite() && seconds >= 0.0,
            "frame duration must be finite and nonnegative"
        );
        self.frame.clear();
        if seconds > 0.0 && self.bin_count > 0 {
            self.flush_bin();
        }
        let mut remaining = seconds;
        while remaining > 0.0 {
            let Some(drive) = self.pending.front_mut() else {
                self.frame.push(Drive {
                    levels: self.last_levels.map(|level| level * self.preamp),
                    seconds: remaining,
                });
                break;
            };
            let elapsed = remaining.min(drive.seconds);
            self.last_levels = drive.levels;
            self.frame.push(Drive {
                levels: drive.levels.map(|level| level * self.preamp),
                seconds: elapsed,
            });
            drive.seconds -= elapsed;
            remaining -= elapsed;
            if drive.seconds == 0.0 {
                self.pending.pop_front();
            }
        }
        &self.frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last(processor: &mut Processor, seconds: f64) -> [f32; 2] {
        processor.frame_drives(seconds).last().unwrap().levels
    }

    #[test]
    fn meters_stereo_channels_independently_and_resets_on_sample_rate_change() {
        let mut processor = Processor::new(1.0, Mode::Rms);
        processor.set_samplerate(100);
        processor.consume_buf(&[[2.0, -1.5]; 60]);
        assert_eq!(last(&mut processor, 1.0), [2.0, 1.5]);
        processor.set_samplerate(50);
        processor.consume_buf(&[[0.0, 0.5]; 15]);
        assert_eq!(last(&mut processor, 1.0), [0.0, 0.5]);
    }

    #[test]
    fn rms_uses_energy_and_vu_uses_rectified_average() {
        let mut rms = Processor::new(1.0, Mode::Rms);
        let mut vu = Processor::new(1.0, Mode::Vu);
        for processor in [&mut rms, &mut vu] {
            processor.set_samplerate(48000);
            let samples: Vec<_> = (0..28800)
                .map(|n| [if n % 2 == 0 { 2.0 } else { 0.0 }, -0.5])
                .collect();
            processor.consume_buf(&samples);
        }
        let rms = last(&mut rms, 0.3);
        let vu = last(&mut vu, 0.3);
        assert!((rms[0] - 2.0_f32.sqrt()).abs() < 1e-6);
        assert!((vu[0] - std::f32::consts::PI / (2.0 * 2.0_f32.sqrt())).abs() < 1e-6);
        assert!(vu[0] < rms[0]);
    }

    #[test]
    fn ppm_detector_matches_type_two_burst_integration_without_pointer_overswing() {
        fn peak(samplerate: usize, burst_seconds: f64) -> f64 {
            let mut detector = Detector::new(Mode::Ppm, samplerate);
            (0..samplerate / 2)
                .map(|sample| {
                    let time = sample as f64 / samplerate as f64;
                    let input = if time < burst_seconds {
                        0.5 * (std::f64::consts::TAU * 5000.0 * time).sin()
                    } else {
                        0.0
                    };
                    detector.sample([input, 0.0])[0]
                })
                .fold(0.0, f64::max)
        }
        for samplerate in [44100, 48000, 96000] {
            let sustained = peak(samplerate, 0.5);
            assert!((20.0 * (sustained / 0.5).log10()).abs() < 0.1);
            for (seconds, expected_db) in [(0.0015, -9.0), (0.005, -4.0), (0.010, -2.0), (0.1, 0.0)]
            {
                let actual_db = 20.0 * (peak(samplerate, seconds) / sustained).log10();
                assert!(
                    (actual_db - expected_db).abs() < 0.6,
                    "{samplerate}, {seconds}s detector burst: {actual_db} vs {expected_db}"
                );
            }
        }
    }

    #[test]
    fn sample_peak_retains_single_sample_and_then_returns_at_twenty_db_per_second() {
        let mut processor = Processor::new(1.0, Mode::SamplePeak);
        processor.set_samplerate(48000);
        processor.consume_buf(&[[2.0, 0.0]]);
        processor.consume_buf(&[[0.0; 2]; 4799]);
        assert_eq!(last(&mut processor, 0.1), [2.0, 0.0]);
        for _ in 0..10 {
            processor.consume_buf(&[[0.0; 2]; 4800]);
            processor.frame_drives(0.1);
        }
        assert!((processor.last_levels[0] - 0.2).abs() < 0.001);
    }

    #[test]
    fn preserves_a_short_vu_drive_between_display_frames_and_resets_on_mode_change() {
        let mut processor = Processor::new(1.0, Mode::Vu);
        processor.set_samplerate(48000);
        processor.consume_buf(&[[1.0, 0.0]; 48]);
        processor.consume_buf(&[[0.0; 2]; 4752]);
        let drives = processor.frame_drives(0.1);
        let area: f64 = drives
            .iter()
            .map(|drive| f64::from(drive.levels[0]) * drive.seconds)
            .sum();
        assert!((area - 0.001 * std::f64::consts::PI / (2.0 * 2.0_f64.sqrt())).abs() < 1e-9);
        processor.set_mode(Mode::SamplePeak);
        assert_eq!(last(&mut processor, 0.1), [0.0; 2]);
    }
}
