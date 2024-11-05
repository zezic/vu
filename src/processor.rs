use std::{collections::VecDeque, ops::Mul};

use instant::Instant;

use crate::db_to_multiplier;

pub struct Processor {
    squares: VecDeque<[i64; 2]>,
    square_sums: VecDeque<[i64; 2]>,
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
            preamp: db_to_multiplier(18.0),
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

        if self.square_sums.len() > buf.len() / 2 {
            // Leave only buf size square_sums
            self.square_sums.drain(0..self.square_sums.len() - buf.len() / 2);
        }
    }

    pub fn get_hands_for_instant(&self, instant: Instant) -> [f32; 2] {
        let window_len = (self.samplerate as f32 * 0.3) as i64;

        let offset = (instant.duration_since(self.head_instant).as_secs_f32() * (self.samplerate as f32)) as usize;
        let square_sums = self.square_sums.get(offset).unwrap_or(self.square_sums.back().unwrap_or(&[0; 2]));
        let sqrt = ((square_sums[0] / window_len) as f32).sqrt() / 16384.0;
        let sqrt_2 = ((square_sums[1] / window_len) as f32).sqrt() / 16384.0;
        [
            if sqrt.is_nan() { 0.0 } else { sqrt } * self.preamp,
            if sqrt_2.is_nan() { 0.0 } else { sqrt } * self.preamp,
        ]
    }
}