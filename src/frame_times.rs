use std::collections::VecDeque;

pub struct FrameTimes {
    values: VecDeque<f32>,
}

impl FrameTimes {
    pub fn new() -> Self {
        Self {
            values: VecDeque::with_capacity(100),
        }
    }

    pub fn update(&mut self, frame_time: f32) {
        if self.values.len() == 100 {
            self.values.pop_front();
        }
        self.values.push_back(frame_time);
    }

    pub fn average(&self) -> f32 {
        assert!(
            !self.values.is_empty(),
            "record a frame time before averaging"
        );
        self.values.iter().sum::<f32>() / self.values.len() as f32
    }
}
