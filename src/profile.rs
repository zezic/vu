use std::time::{Duration, Instant};

use anyhow::{bail, ensure};
use cpu_time::{ProcessTime, ThreadTime};

use crate::{
    meter::Meter,
    render::{Presentation, Stage},
    AudioEvent,
};

pub const SAMPLE_RATE: usize = 48000;
const WARMUP: Duration = Duration::from_secs(1);
const MEASURE: Duration = Duration::from_secs(10);

enum Phase {
    Pending,
    Warmup(Instant),
    Measuring {
        run_start: Instant,
        wall_start: Instant,
        process_start: ProcessTime,
        thread_start: ThreadTime,
    },
}

#[derive(Default)]
struct Stages {
    acquire: Duration,
    rasterize: Duration,
    submit: Duration,
}

/// A bounded synthetic workload through the normal window and presentation loop.
pub struct Run {
    phase: Phase,
    audio_frames: usize,
    frames: u32,
    base_rasterizations: u32,
    draw: Duration,
    stages: Stages,
}

impl Run {
    pub fn from_args() -> anyhow::Result<Option<Self>> {
        let mut args = std::env::args_os().skip(1);
        match (args.next(), args.next()) {
            (None, None) => Ok(None),
            (Some(arg), None) if arg == "--profile" => Ok(Some(Self {
                phase: Phase::Pending,
                audio_frames: 0,
                frames: 0,
                base_rasterizations: 0,
                draw: Duration::ZERO,
                stages: Stages::default(),
            })),
            _ => bail!("Usage: vu [--profile]"),
        }
    }

    pub fn feed(&mut self, meter: &mut Meter, now: Instant) -> anyhow::Result<()> {
        let start = match &self.phase {
            Phase::Pending => {
                self.phase = Phase::Warmup(now);
                now
            }
            Phase::Warmup(start) => {
                let start = *start;
                if now.duration_since(start) >= WARMUP {
                    self.phase = Phase::Measuring {
                        run_start: start,
                        wall_start: now,
                        process_start: ProcessTime::try_now()?,
                        thread_start: ThreadTime::try_now()?,
                    };
                }
                start
            }
            Phase::Measuring { run_start, .. } => *run_start,
        };
        let elapsed = now.duration_since(start).as_secs_f64();
        let due = (elapsed * SAMPLE_RATE as f64) as usize;
        // Deliver only new samples, including when presentation limits the cadence.
        meter.audio_event(AudioEvent::Buffer {
            buf: vec![levels(elapsed); due - self.audio_frames],
        });
        self.audio_frames = due;
        Ok(())
    }

    pub fn present(
        &mut self,
        draw: Instant,
        present: Instant,
        changed: bool,
        render: impl FnOnce(&mut dyn FnMut(Stage)) -> anyhow::Result<Presentation>,
    ) -> anyhow::Result<Presentation> {
        let mut stages = Stages::default();
        let mut previous = present;
        let result = render(&mut |stage| {
            let now = Instant::now();
            let elapsed = now - previous;
            previous = now;
            match stage {
                Stage::Acquire => stages.acquire = elapsed,
                Stage::Rasterize => stages.rasterize = elapsed,
                Stage::Submit => stages.submit = elapsed,
            }
        })?;
        if matches!(self.phase, Phase::Measuring { .. })
            && matches!(result, Presentation::Presented)
        {
            self.frames += 1;
            self.base_rasterizations += u32::from(changed);
            self.draw += present - draw;
            self.stages.acquire += stages.acquire;
            self.stages.rasterize += stages.rasterize;
            self.stages.submit += stages.submit;
        }
        Ok(result)
    }

    pub fn finished(&self, now: Instant) -> bool {
        match &self.phase {
            Phase::Pending => false,
            Phase::Warmup(start)
            | Phase::Measuring {
                run_start: start, ..
            } => now.duration_since(*start) >= WARMUP + MEASURE,
        }
    }

    pub fn report(&self) -> anyhow::Result<()> {
        ensure!(
            self.frames > 0,
            "the profiling window presented no measured frames"
        );
        let Phase::Measuring {
            wall_start,
            process_start,
            thread_start,
            ..
        } = &self.phase
        else {
            unreachable!("measured frames have CPU and wall-clock origins")
        };
        let elapsed = wall_start.elapsed().as_secs_f64();
        let process_cpu = process_start.try_elapsed()?.as_secs_f64();
        let main_cpu = thread_start.try_elapsed()?.as_secs_f64();
        let frames = f64::from(self.frames);
        eprintln!(
            "VU profile: {:.1} FPS; {}/{} base rasterizations; draw {:.1} us/frame; present {:.1} us/frame",
            frames / elapsed,
            self.base_rasterizations,
            self.frames,
            self.draw.as_secs_f64() * 1e6 / frames,
            (self.stages.acquire + self.stages.rasterize + self.stages.submit).as_secs_f64() * 1e6
                / frames,
        );
        eprintln!(
            "CPU: process {:.1}%; main thread {:.1}%; acquire {:.1} us/frame; rasterize {:.1} us/frame; submit {:.1} us/frame",
            100.0 * process_cpu / elapsed,
            100.0 * main_cpu / elapsed,
            self.stages.acquire.as_secs_f64() * 1e6 / frames,
            self.stages.rasterize.as_secs_f64() * 1e6 / frames,
            self.stages.submit.as_secs_f64() * 1e6 / frames,
        );
        Ok(())
    }
}

pub fn levels(elapsed: f64) -> [f32; 2] {
    let phase = elapsed * std::f64::consts::TAU;
    [
        (0.02 + 0.9 * (phase.sin() * 0.5 + 0.5)) as f32,
        (0.02 + 0.9 * (phase.cos() * 0.5 + 0.5)) as f32,
    ]
}
