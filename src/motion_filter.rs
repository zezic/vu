use std::f32::consts::PI;

/// A simple first-order low-pass filter
pub struct LowPassFilter {
    cutoff: f32,   // Cutoff frequency in Hz
    sample_rate: f32, // Sample rate in Hz
    alpha: f32,    // Filter coefficient
    prev_output: f32, // Previous output for feedback
}

impl LowPassFilter {
    /// Creates a new LowPassFilter with a given cutoff frequency and sample rate
    pub fn new(cutoff: f32, sample_rate: f32) -> Self {
        let mut filter = LowPassFilter {
            cutoff,
            sample_rate,
            alpha: 0.0,
            prev_output: 0.0,
        };
        filter.update_alpha();
        filter
    }

    /// Update the alpha value based on the cutoff frequency and sample rate
    fn update_alpha(&mut self) {
        let rc = 1.0 / (2.0 * PI * self.cutoff);
        let dt = 1.0 / self.sample_rate;
        self.alpha = dt / (rc + dt);
    }

    /// Set the cutoff frequency and update the alpha coefficient
    pub fn set_cutoff(&mut self, cutoff: f32) {
        self.cutoff = cutoff;
        self.update_alpha();
    }

    /// Apply the filter to a single sample
    pub fn process(&mut self, input: f32) -> f32 {
        let output = self.alpha * input + (1.0 - self.alpha) * self.prev_output;
        self.prev_output = output;
        output
    }
}

/// A second-order low-pass filter implemented by cascading two first-order filters
pub struct SecondOrderLowPassFilter {
    filter1: LowPassFilter,
    filter2: LowPassFilter,
}

impl SecondOrderLowPassFilter {
    pub fn new(cutoff: f32, sample_rate: f32) -> Self {
        SecondOrderLowPassFilter {
            filter1: LowPassFilter::new(cutoff, sample_rate),
            filter2: LowPassFilter::new(cutoff, sample_rate),
        }
    }

    pub fn set_samplerate(&mut self, samplerate: f32) {
        self.filter1.sample_rate = samplerate;
        self.filter2.sample_rate = samplerate;
        self.filter1.update_alpha();
        self.filter2.update_alpha();
    }

    pub fn set_cutoff(&mut self, cutoff: f32) {
        self.filter1.set_cutoff(cutoff);
        self.filter2.set_cutoff(cutoff);
    }

    pub fn process(&mut self, input: f32) -> f32 {
        let temp = self.filter1.process(input);
        self.filter2.process(temp)
    }
}