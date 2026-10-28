use std::collections::VecDeque;

use crate::{audio::timeline::AudioInterval, config::ShutterFrames};

const HISTORY_SECONDS: f64 = 2.0;

/// Display-frame shutter accounting shared by recorded motion sources.
#[derive(Default)]
pub struct ShutterWindow {
    frames: VecDeque<f64>,
}

impl ShutterWindow {
    /// Convert the frame being displayed into the audio-time exposure it covers.
    pub fn interval(&mut self, frame: AudioInterval, shutter: ShutterFrames) -> AudioInterval {
        let end = frame.end();
        if frame.seconds() > 0.0 {
            if self.frames.len() == ShutterFrames::MAX as usize {
                self.frames.pop_front();
            }
            self.frames.push_back(frame.seconds().min(HISTORY_SECONDS));
        }
        let mut frames = f64::from(shutter.frames());
        let seconds: f64 = self
            .frames
            .iter()
            .rev()
            .map(|seconds| {
                let fraction = frames.min(1.0);
                frames -= fraction;
                seconds * fraction
            })
            .sum();
        let start = if frame.seconds() == 0.0 {
            end
        } else {
            frame.end().shifted(-seconds.min(HISTORY_SECONDS))
        };
        AudioInterval::new(start, end)
    }

    pub(crate) fn clear(&mut self) {
        self.frames.clear();
    }
}
