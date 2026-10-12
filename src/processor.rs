use std::{collections::VecDeque, ops::Mul};

use instant::Instant;

pub struct Processor {
    squares: VecDeque<[i64; 2]>,
    square_sums: VecDeque<[i64; 2]>,
    head_instant: Instant,
    samplerate: usize,
    pub preamp: f32,
}

impl Processor {
    pub fn new(preamp: f32) -> Self {
        Self {
            squares: VecDeque::new(),
            square_sums: VecDeque::new(),
            head_instant: Instant::now(),
            samplerate: 44100,
            preamp,
        }
    }

    pub fn set_samplerate(&mut self, samplerate: usize) {
        assert!(samplerate > 0, "sample rate must be positive");
        self.samplerate = samplerate;
        self.squares.clear();
        self.square_sums.clear();
        self.head_instant = Instant::now();
    }

    pub fn consume_buf(&mut self, buf: &[[f32; 2]]) {
        // 300ms window
        self.head_instant = Instant::now();
        let window_len = (self.samplerate as f32 * 0.3) as usize;

        if !self.square_sums.is_empty() {
            // Leave only last square_sum
            self.square_sums.drain(0..self.square_sums.len() - 1);
        }

        for incoming_pair in buf {
            let rms_head_squares = if self.squares.len() >= window_len {
                self.squares.pop_front().unwrap()
            } else {
                [0; 2]
            };

            let square_sums = self.square_sums.back().unwrap_or(&[0; 2]);

            let incoming_squares = [
                (incoming_pair[0].clamp(-1.0, 1.0).mul(16384.0) as i64).pow(2),
                (incoming_pair[1].clamp(-1.0, 1.0).mul(16384.0) as i64).pow(2),
            ];

            self.squares.push_back(incoming_squares);

            self.square_sums.push_back([
                square_sums[0] + incoming_squares[0] - rms_head_squares[0],
                square_sums[1] + incoming_squares[1] - rms_head_squares[1],
            ]);
        }

        if self.square_sums.len() > buf.len() {
            // Leave only buf size square_sums
            self.square_sums
                .drain(0..self.square_sums.len() - buf.len());
        }
    }

    pub fn get_hands_for_instant(&self, instant: Instant) -> [f32; 2] {
        let window_len = (self.samplerate as f32 * 0.3) as i64;

        let offset = (instant.duration_since(self.head_instant).as_secs_f32()
            * (self.samplerate as f32)) as usize;
        let square_sums = self
            .square_sums
            .get(offset)
            .unwrap_or(self.square_sums.back().unwrap_or(&[0; 2]));
        let sqrt = ((square_sums[0] / window_len) as f32).sqrt() / 16384.0;
        let sqrt_2 = ((square_sums[1] / window_len) as f32).sqrt() / 16384.0;
        [
            if sqrt.is_nan() { 0.0 } else { sqrt } * self.preamp,
            if sqrt_2.is_nan() { 0.0 } else { sqrt_2 } * self.preamp,
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::Processor;

    #[test]
    fn meters_stereo_channels_independently_and_resets_on_sample_rate_change() {
        let mut processor = Processor::new(1.0);
        processor.set_samplerate(100);
        processor.consume_buf(&[[0.5, 0.25]; 60]);
        assert_eq!(
            processor.get_hands_for_instant(processor.head_instant + Duration::from_secs(1)),
            [0.5, 0.25]
        );

        processor.set_samplerate(50);
        processor.consume_buf(&[[0.0, 0.5]; 15]);
        assert_eq!(
            processor.get_hands_for_instant(processor.head_instant + Duration::from_secs(1)),
            [0.0, 0.5]
        );
    }
}
