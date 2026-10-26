use std::sync::Arc;

use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use serde::{Deserialize, Serialize};

pub const BAND_COUNT: usize = 16;
/// Analysis window. Longer windows resolve the lowest log bands; the hop keeps
/// transient detection at display rates.
const FFT_SIZE: usize = 4096;
const HOP_SIZE: usize = 512;
const MIN_FREQUENCY: f32 = 32.0;
const MAX_FREQUENCY: f32 = 16_000.0;
const BASS_BANDS: usize = 4;
const TREBLE_BANDS: usize = 4;
const BIN_COUNT: usize = FFT_SIZE / 2;

/// Parameters of the per-bin relative spectral excess detector.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RelativeExcessSettings {
    /// Background envelope attack and release time constants in seconds.
    pub background_attack: f32,
    pub background_release: f32,
    /// Floor added to the background before the relative excess division.
    pub floor: f32,
    /// Upper bound of one bin's relative excess.
    pub cap: f32,
}

impl Default for RelativeExcessSettings {
    fn default() -> Self {
        Self {
            background_attack: 0.320,
            background_release: 0.200,
            floor: 0.010,
            cap: 1.0,
        }
    }
}

/// One analysis hop, normalized to 0..1 for the visible scene parameters.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spectrum {
    pub bands: [f32; BAND_COUNT],
    /// Broadband level of the analysis window.
    pub power: f32,
    pub bass: f32,
    pub treble: f32,
    /// Mean capped relative excess of each band's FFT bins.
    pub band_excess: [f32; BAND_COUNT],
}

pub struct SpectrumAnalyzer {
    samplerate: usize,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    window_sum: f32,
    ring: Vec<f32>,
    ring_position: usize,
    filled: usize,
    since_hop: usize,
    fft_buffer: Vec<Complex32>,
    band_bins: [(usize, usize); BAND_COUNT],
    frequency_edges: [f32; BAND_COUNT + 1],
    bin_background: Vec<f32>,
    excess: RelativeExcessSettings,
}

impl SpectrumAnalyzer {
    pub fn new(samplerate: usize) -> Self {
        assert!(samplerate > 0, "the spectrum needs a positive sample rate");
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|index| {
                let phase = std::f32::consts::TAU * index as f32 / FFT_SIZE as f32;
                0.5 - 0.5 * phase.cos()
            })
            .collect();
        let window_sum = window.iter().sum::<f32>();
        let (band_bins, frequency_edges) = Self::bands(samplerate);
        Self {
            samplerate,
            fft,
            window,
            window_sum,
            ring: vec![0.0; FFT_SIZE],
            ring_position: 0,
            filled: 0,
            since_hop: 0,
            fft_buffer: vec![Complex32::ZERO; FFT_SIZE],
            band_bins,
            frequency_edges,
            bin_background: vec![0.0; BIN_COUNT],
            excess: RelativeExcessSettings::default(),
        }
    }

    fn bands(samplerate: usize) -> ([(usize, usize); BAND_COUNT], [f32; BAND_COUNT + 1]) {
        let minimum = MIN_FREQUENCY;
        let maximum = MAX_FREQUENCY
            .min(samplerate as f32 * 0.45)
            .max(minimum * 2.0);
        let ratio = maximum / minimum;
        let mut edges = [0.0; BAND_COUNT + 1];
        for (index, edge) in edges.iter_mut().enumerate() {
            *edge = minimum * ratio.powf(index as f32 / BAND_COUNT as f32);
        }
        let bin_hz = samplerate as f32 / FFT_SIZE as f32;
        let nyquist_bin = FFT_SIZE / 2;
        let mut bins = [(0, 1); BAND_COUNT];
        for (band, range) in bins.iter_mut().enumerate() {
            let start = (edges[band] / bin_hz).ceil() as usize;
            let end = (edges[band + 1] / bin_hz).floor() as usize;
            let start = start.clamp(1, nyquist_bin - 1);
            let end = end.max(start + 1).min(nyquist_bin);
            *range = (start, end);
        }
        (bins, edges)
    }

    pub fn set_samplerate(&mut self, samplerate: usize) {
        assert!(samplerate > 0, "the spectrum needs a positive sample rate");
        if self.samplerate == samplerate {
            return;
        }
        self.samplerate = samplerate;
        let (band_bins, frequency_edges) = Self::bands(samplerate);
        self.band_bins = band_bins;
        self.frequency_edges = frequency_edges;
        self.reset();
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.ring_position = 0;
        self.filled = 0;
        self.since_hop = 0;
        self.bin_background.fill(0.0);
    }

    pub fn set_relative_excess(&mut self, settings: RelativeExcessSettings) {
        assert!(
            settings.background_attack.is_finite()
                && settings.background_attack > 0.0
                && settings.background_release.is_finite()
                && settings.background_release > 0.0
                && settings.floor.is_finite()
                && settings.floor >= 0.0
                && settings.cap.is_finite()
                && settings.cap > 0.0,
            "relative excess settings must be positive and finite"
        );
        self.excess = settings;
    }

    pub fn hop_seconds(&self) -> f32 {
        HOP_SIZE as f32 / self.samplerate as f32
    }

    pub fn band_range(&self, index: usize) -> (f32, f32) {
        let index = index.min(BAND_COUNT - 1);
        (self.frequency_edges[index], self.frequency_edges[index + 1])
    }

    /// Feed one post-preamp mono sample. A hop returns the analyzed spectrum.
    pub fn push(&mut self, sample: f32) -> Option<Spectrum> {
        self.ring[self.ring_position] = sample;
        self.ring_position = (self.ring_position + 1) % FFT_SIZE;
        self.filled = (self.filled + 1).min(FFT_SIZE);
        self.since_hop += 1;
        if self.since_hop < HOP_SIZE {
            return None;
        }
        self.since_hop = 0;
        (self.filled == FFT_SIZE).then(|| self.analyze())
    }

    fn analyze(&mut self) -> Spectrum {
        let mut energy = 0.0;
        for index in 0..FFT_SIZE {
            let sample = self.ring[(self.ring_position + index) % FFT_SIZE];
            energy += sample * sample;
            self.fft_buffer[index] = Complex32::new(sample * self.window[index], 0.0);
        }
        self.fft.process(&mut self.fft_buffer);
        let norm = 2.0 / self.window_sum;
        let hop = self.hop_seconds();
        let attack_alpha = (-hop / self.excess.background_attack).exp();
        let release_alpha = (-hop / self.excess.background_release).exp();
        let mut bands = [0.0; BAND_COUNT];
        let mut band_excess = [0.0; BAND_COUNT];
        for (band, &(start, end)) in self.band_bins.iter().enumerate() {
            let mut power = 0.0;
            let mut excess = 0.0;
            for bin in start..end {
                let magnitude = self.fft_buffer[bin].norm() * norm;
                power += magnitude * magnitude;
                let background = self.bin_background[bin];
                let denominator = (background + self.excess.floor).max(1e-9);
                excess += ((magnitude - background) / denominator)
                    .max(0.0)
                    .min(self.excess.cap);
                let alpha = if magnitude > background {
                    attack_alpha
                } else {
                    release_alpha
                };
                self.bin_background[bin] = alpha * background + (1.0 - alpha) * magnitude;
            }
            bands[band] = normalize_db(power.sqrt(), -72.0, 24.0);
            band_excess[band] = excess / (end - start) as f32;
        }
        let rms = (energy / FFT_SIZE as f32).sqrt();
        let power = normalize_db(rms, -60.0, 12.0);
        let bass = bass_energy(&bands);
        let treble = bands[BAND_COUNT - TREBLE_BANDS..].iter().sum::<f32>() / TREBLE_BANDS as f32;
        Spectrum {
            bands,
            power,
            bass,
            treble,
            band_excess,
        }
    }

    #[cfg(test)]
    pub(crate) fn frequency_edges(&self) -> &[f32; BAND_COUNT + 1] {
        &self.frequency_edges
    }

    #[cfg(test)]
    pub(crate) fn band_bins(&self) -> &[(usize, usize); BAND_COUNT] {
        &self.band_bins
    }
}

fn bass_energy(bands: &[f32; BAND_COUNT]) -> f32 {
    bands[..BASS_BANDS]
        .iter()
        .copied()
        .fold(0.0, f32::max)
        .clamp(0.0, 1.0)
}

fn normalize_db(amplitude: f32, floor_db: f32, ceiling_db: f32) -> f32 {
    let db = 20.0 * amplitude.max(1e-9).log10();
    ((db - floor_db) / (ceiling_db - floor_db)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_tone(analyzer: &mut SpectrumAnalyzer, frequency: f32, samples: usize) -> Spectrum {
        let mut last = Spectrum::default();
        for sample in 0..samples {
            let phase = std::f32::consts::TAU * frequency * sample as f32 / 48_000.0;
            if let Some(spectrum) = analyzer.push(phase.sin()) {
                last = spectrum;
            }
        }
        last
    }

    fn expected_band(analyzer: &SpectrumAnalyzer, frequency: f32) -> usize {
        analyzer
            .frequency_edges()
            .windows(2)
            .position(|edge| edge[0] <= frequency && frequency < edge[1])
            .expect("the tone lies inside the analyzed range")
    }

    #[test]
    fn log_bands_place_tones_in_the_matching_area() {
        for frequency in [80.0, 440.0, 1_000.0, 6_000.0, 12_000.0] {
            let mut analyzer = SpectrumAnalyzer::new(48_000);
            let spectrum = feed_tone(&mut analyzer, frequency, 24_000);
            let expected = expected_band(&analyzer, frequency);
            let strongest = spectrum
                .bands
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.total_cmp(b))
                .map(|(index, _)| index)
                .unwrap();
            assert_eq!(
                strongest, expected,
                "{frequency} Hz landed in band {strongest} instead of {expected}"
            );
            assert!(spectrum.bands[expected] > 0.6);
            assert!(spectrum.power > 0.7);
        }
    }

    #[test]
    fn relative_excess_caps_on_a_rise_and_decays_into_the_background() {
        let mut analyzer = SpectrumAnalyzer::new(48_000);
        analyzer.set_relative_excess(RelativeExcessSettings {
            background_attack: 0.05,
            background_release: 0.05,
            cap: 0.5,
            ..RelativeExcessSettings::default()
        });
        for _ in 0..FFT_SIZE {
            analyzer.push(0.0);
        }
        let band = expected_band(&analyzer, 40.0);
        let mut peak = 0.0_f32;
        let mut last = 0.0_f32;
        for sample in 0..48_000 {
            let phase = std::f32::consts::TAU * 40.0 * sample as f32 / 48_000.0;
            if let Some(spectrum) = analyzer.push(phase.sin()) {
                peak = peak.max(spectrum.band_excess[band]);
                last = spectrum.band_excess[band];
            }
        }
        assert!((peak - 0.5).abs() < 1e-5, "the ratio cap bounds the rise");
        assert!(last < 0.05, "a held tone sinks into the background: {last}");
    }

    #[test]
    fn log_bands_are_ordered_and_cover_the_audio_range() {
        let analyzer = SpectrumAnalyzer::new(48_000);
        let edges = analyzer.frequency_edges();
        assert_eq!(edges[0], MIN_FREQUENCY);
        assert!(edges[BAND_COUNT] <= MAX_FREQUENCY);
        for pair in edges.windows(2) {
            assert!(pair[0] < pair[1]);
        }
        for (band, &(start, end)) in analyzer.band_bins().iter().enumerate() {
            assert!(start >= 1, "band {band} includes DC");
            assert!(start < end, "band {band} needs at least one bin");
            if band > 0 {
                assert!(
                    start >= analyzer.band_bins()[band - 1].0,
                    "band {band} overlaps its lower neighbour"
                );
            }
        }
    }

    #[test]
    fn silence_after_a_tone_returns_bands_to_rest() {
        let mut analyzer = SpectrumAnalyzer::new(48_000);
        let tone = feed_tone(&mut analyzer, 1_000.0, 24_000);
        assert!(tone.bands.iter().any(|&band| band > 0.6));
        let mut silence = Spectrum::default();
        for _ in 0..12_000 {
            if let Some(spectrum) = analyzer.push(0.0) {
                silence = spectrum;
            }
        }
        assert!(silence.bands.iter().all(|&band| band < 0.01));
        assert!(silence.power < 0.01);
        assert_eq!(silence.bass, 0.0);
    }
}
