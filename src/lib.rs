//! Audio capture, a model-driven wireframe visualizer, themes and HUD.
use std::time::Instant;

pub mod audio;
pub mod config;
pub mod frame_schedule;
pub mod render;
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

pub mod profile;
