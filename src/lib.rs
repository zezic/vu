//! Reusable VU measurement, vector faces, themes and GPU needle overlays.
use std::time::Instant;

pub mod audio;
pub mod config;
pub mod frame_schedule;
pub mod measurement;
pub mod meter;
pub mod processor;
pub mod render;
pub mod scales;
pub mod shutter;
pub mod text;
pub mod theme;
pub mod visualizer;

pub enum AudioEvent {
    Config {
        samplerate: usize,
    },
    Buffer {
        buf: Vec<[f32; 2]>,
        captured_at: Instant,
    },
}

pub fn db_to_multiplier(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
pub fn normalized_to_db(value: f32, negative_db_range: f32) -> f32 {
    if value <= 0.0 {
        -negative_db_range
    } else {
        20.0 * value.log10()
    }
}
pub fn db_to_normalized(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub mod profile;
