//! Deterministic CPU workload without a window or GPU.
use std::time::{Duration, Instant};

use vello::{kurbo::Size, Scene};
use vu::{
    config::Config,
    frame_schedule::{FrameRateLimit, FrameTime},
    visualizer::{Ui, View},
    AudioEvent,
};

fn main() -> anyhow::Result<()> {
    let mut ui = Ui::new(&Config::default())?;
    ui.set_presentation(true);
    ui.audio_event(AudioEvent::Config { samplerate: 48_000 });
    let view = View::fit(Size::new(1920.0, 1080.0));
    let mut scene = Scene::new();
    let origin = Instant::now();
    let mut measured = Duration::ZERO;
    let mut rebuilt = 0;
    let mut ribbons = 0;
    for index in 0..660 {
        let start = origin + Duration::from_secs_f64(index as f64 / 60.0);
        let end = origin + Duration::from_secs_f64((index + 1) as f64 / 60.0);
        let samples = (index * 800..(index + 1) * 800)
            .map(|sample| {
                let time = sample as f32 / 48_000.0;
                let tone = 0.25 * (std::f32::consts::TAU * 220.0 * time).sin();
                [tone; 2]
            })
            .collect();
        ui.audio_event(AudioEvent::Buffer {
            buf: samples,
            captured_at: start,
        });
        scene.reset();
        let before = Instant::now();
        let frame = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps60,
            FrameTime::new(start, end),
        );
        if index >= 60 {
            measured += before.elapsed();
            rebuilt += usize::from(matches!(frame.background, vu::render::Frame::Rebuilt(_)));
            ribbons += frame.overlay.ribbons.len();
        }
    }
    println!(
        "CPU draw: {:.1} us/frame; {rebuilt}/600 background rebuilds; {} ribbons/frame",
        measured.as_secs_f64() * 1e6 / 600.0,
        ribbons / 600
    );
    Ok(())
}
