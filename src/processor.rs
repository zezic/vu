use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use crate::audio::timeline::{AudioClock, AudioInterval, AudioTime};

use crate::measurement::Mode;

/// A detector drive held for a span of audio time.
#[derive(Clone, Copy)]
pub struct Drive {
    pub levels: [f32; 2],
    pub interval: AudioInterval,
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
    drives: Vec<Drive>,
    samples: u64,
    clock: Option<AudioClock>,
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
            drives: Vec::new(),
            samples: 0,
            clock: None,
            last_levels: [0.0; 2],
            preamp,
        }
    }

    fn reset(&mut self) {
        self.detector = Detector::new(self.mode, self.samplerate);
        self.bin_sum = [0.0; 2];
        self.bin_count = 0;
        self.drives.clear();
        self.last_levels = [0.0; 2];
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.reset();
    }

    pub fn set_samplerate(&mut self, samplerate: usize) {
        assert!(samplerate > 0, "sample rate must be positive");
        self.samplerate = samplerate;
        self.samples = 0;
        self.clock = None;
        self.reset();
    }

    pub fn audio_time(&self) -> AudioTime {
        AudioTime::from_samples(self.samples, self.samplerate)
    }

    pub fn position_at(&self, now: Instant) -> Option<AudioTime> {
        self.clock.map(|clock| clock.position_at(now))
    }

    pub fn partial_drive(&self) -> Option<Drive> {
        (self.bin_count > 0).then(|| Drive {
            levels: self
                .bin_sum
                .map(|sum| (sum / self.bin_count as f64) as f32 * self.preamp),
            interval: AudioInterval::new(
                AudioTime::from_samples(self.samples - self.bin_count as u64, self.samplerate),
                self.audio_time(),
            ),
        })
    }

    pub fn latest_levels(&self) -> [f32; 2] {
        self.partial_drive().map_or_else(
            || self.last_levels.map(|level| level * self.preamp),
            |drive| drive.levels,
        )
    }

    /// Each sample advances the detector once. Bins stay on the sample grid across
    /// capture buffers; drawing never flushes or consumes a partial bin.
    pub fn consume_buf(&mut self, buf: &[[f32; 2]], captured_at: Instant) -> &[Drive] {
        self.drives.clear();
        if buf.is_empty() {
            return &self.drives;
        }
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
            self.samples += 1;
            if self.bin_count == bin_samples {
                let drive = self.partial_drive().expect("a full bin has a drive");
                self.last_levels = self.bin_sum.map(|sum| (sum / self.bin_count as f64) as f32);
                self.drives.push(drive);
                self.bin_sum = [0.0; 2];
                self.bin_count = 0;
            }
        }
        self.clock = Some(AudioClock {
            position: self.audio_time(),
            at: captured_at + Duration::from_secs_f64(buf.len() as f64 / self.samplerate as f64),
        });
        &self.drives
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consume(processor: &mut Processor, buf: &[[f32; 2]]) -> Vec<Drive> {
        let at = processor.clock.map_or_else(Instant::now, |clock| clock.at);
        processor.consume_buf(buf, at).to_vec()
    }

    #[test]
    fn meters_stereo_channels_independently_and_resets_on_sample_rate_change() {
        let mut processor = Processor::new(1.0, Mode::Rms);
        processor.set_samplerate(100);
        consume(&mut processor, &[[2.0, -1.5]; 60]);
        assert_eq!(processor.latest_levels(), [2.0, 1.5]);
        processor.set_samplerate(50);
        consume(&mut processor, &[[0.0, 0.5]; 15]);
        assert_eq!(processor.latest_levels(), [0.0, 0.5]);
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
            consume(processor, &samples);
        }
        let rms = rms.latest_levels();
        let vu = vu.latest_levels();
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
        consume(&mut processor, &[[2.0, 0.0]]);
        consume(&mut processor, &[[0.0; 2]; 4799]);
        assert_eq!(processor.latest_levels(), [2.0, 0.0]);
        for _ in 0..10 {
            consume(&mut processor, &[[0.0; 2]; 4800]);
        }
        assert!((processor.last_levels[0] - 0.2).abs() < 0.001);
    }

    #[test]
    fn preserves_a_short_vu_drive_between_display_frames_and_resets_on_mode_change() {
        let mut processor = Processor::new(1.0, Mode::Vu);
        processor.set_samplerate(48000);
        let mut drives = consume(&mut processor, &[[1.0, 0.0]; 48]);
        drives.extend(consume(&mut processor, &[[0.0; 2]; 4752]));
        let area: f64 = drives
            .iter()
            .map(|drive| f64::from(drive.levels[0]) * drive.interval.seconds())
            .sum();
        assert!((area - 0.001 * std::f64::consts::PI / (2.0 * 2.0_f64.sqrt())).abs() < 1e-9);
        processor.set_mode(Mode::SamplePeak);
        assert_eq!(processor.latest_levels(), [0.0; 2]);
    }

    #[test]
    fn bins_are_independent_of_buffer_boundaries_and_empty_deliveries() {
        let start = Instant::now();
        for rate in [44100, 48000, 96000] {
            let input: Vec<_> = (0..rate / 10)
                .map(|n| [(n as f32 * 0.123).sin(), 0.5])
                .collect();
            let mut whole = Processor::new(1.0, Mode::Rms);
            whole.set_samplerate(rate);
            let expected = whole.consume_buf(&input, start).to_vec();
            let mut split = Processor::new(1.0, Mode::Rms);
            split.set_samplerate(rate);
            let mut actual = Vec::new();
            let mut samples = 0;
            for chunk in input.chunks(137) {
                let at = start + Duration::from_secs_f64(samples as f64 / rate as f64);
                actual.extend_from_slice(split.consume_buf(chunk, at));
                assert!(split.consume_buf(&[], at).is_empty());
                samples += chunk.len();
            }
            assert_eq!(expected.len(), actual.len());
            for (expected, actual) in expected.iter().zip(&actual) {
                assert_eq!(expected.levels, actual.levels);
                assert_eq!(expected.interval.start(), actual.interval.start());
                assert_eq!(expected.interval.end(), actual.interval.end());
            }
            assert_eq!(whole.audio_time(), split.audio_time());
            assert_eq!(whole.position_at(start), split.position_at(start));
        }
    }
}
