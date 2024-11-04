use std::collections::VecDeque;

use instant::Instant;

use crate::db_to_multiplier;

pub struct Processor {
    squares: VecDeque<[f32; 2]>,
    square_sums: VecDeque<[f32; 2]>,
    head_instant: Instant,
    samplerate: usize,
    pub preamp: f32,
}

impl Processor {
    pub fn new() -> Self {
        Self {
            squares: VecDeque::new(),
            square_sums: VecDeque::new(),
            head_instant: Instant::now(),
            samplerate: 44100,
            preamp: db_to_multiplier(12.0),
        }
    }

    pub fn set_samplerate(&mut self, samplerate: usize) {
        self.samplerate = samplerate;
    }

    pub fn consume_buf(&mut self, buf: Vec<f32>) {
        // 300ms window
        self.head_instant = Instant::now();
        let window_len = (self.samplerate as f32 * 0.3) as usize;

        if !self.square_sums.is_empty() {
            // Leave only last square_sum
            self.square_sums.drain(0..self.square_sums.len() - 1);
        }

        if self.squares.len() > window_len {
            let excess = window_len - self.squares.len();
            // Leave only window requred for managing square_sums
            self.squares.drain(0..excess);
        }

        for incoming_pair in buf.chunks(2) {
            let rms_head_squares = if self.squares.len() >= window_len {
                self.squares.pop_front().unwrap()
            } else {
                [0.0; 2]
            };

            let square_sums = self.square_sums.back().unwrap_or(&[0.0; 2]);

            let incoming_squares = [
                (incoming_pair[0] * self.preamp).powf(2.0),
                (incoming_pair[1] * self.preamp).powf(2.0),
            ];

            self.squares.push_back(incoming_squares);

            self.square_sums.push_back([
                square_sums[0] + incoming_squares[0] - rms_head_squares[0],
                square_sums[1] + incoming_squares[1] - rms_head_squares[1],
            ]);
        }

        if self.square_sums.len() > buf.len() / 2 {
            // Leave only buf size square_sums
            self.square_sums.drain(0..self.square_sums.len() - buf.len() / 2);
        }
    }

    pub fn get_hands_for_instant(&self, instant: Instant) -> [f32; 2] {
        let window_len = self.samplerate as f32 * 0.3;

        let offset = (instant.duration_since(self.head_instant).as_secs_f32() * (self.samplerate as f32)) as usize;
        let square_sums = self.square_sums.get(offset).unwrap_or(self.square_sums.back().unwrap_or(&[0.0; 2]));
        [
            (square_sums[0] / window_len).sqrt(),
            (square_sums[1] / window_len).sqrt(),
        ]
    }
}