use std::{sync::mpsc::Receiver, time::Instant};

use audio::capture_stream;
use config::Config;
use frame_schedule::{Adjustment, FrameKind, FrameSchedule, Occlusion, RedrawPermit, SurfaceArea};
use meter::{Meter, View};
use render::{Presentation, RenderWindow};
use saved::Saved;
use vello::kurbo::Point;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::WindowId,
};

mod audio;
mod config;
mod frame_schedule;
mod frame_times;
mod meter;
mod processor;
mod render;
mod saved;
mod scales;
mod text;

enum AudioEvent {
    Config { samplerate: usize },
    Buffer { buf: Vec<[f32; 2]> },
}

enum WindowState {
    Suspended,
    Active {
        render: Box<RenderWindow>,
        view: View,
    },
    Failed(anyhow::Error),
}

enum DragState {
    Idle,
    Adjusting,
}

enum Setting {
    Preamp,
    FrameRate,
}

struct App {
    state: WindowState,
    rx: Receiver<AudioEvent>,
    meter: Meter,
    settings: Saved<Config>,
    mouse: Point,
    drag: DragState,
    frames: FrameSchedule,
    previous_frame: Instant,
}

impl App {
    fn adjust_setting(&mut self, setting: Setting, adjustment: Adjustment) {
        let settings = self.settings.edit();
        match setting {
            Setting::Preamp => {
                settings.preamp_db = settings.preamp_db.adjusted(adjustment);
                self.meter.set_preamp(settings.preamp_db);
            }
            Setting::FrameRate => {
                settings.frame_rate_limit = settings.frame_rate_limit.adjusted(adjustment);
                self.frames.limit_changed(Instant::now());
            }
        }
    }

    fn window_event(&mut self, event: WindowEvent) -> anyhow::Result<()> {
        let WindowState::Active { render, view } = &mut self.state else {
            return Ok(());
        };
        match event {
            WindowEvent::Resized(size) => {
                render.resize(size);
                *view = View::fit(render.viewport());
                self.frames.resized(size.into(), Instant::now());
            }
            WindowEvent::Occluded(occluded) => {
                let occlusion = if occluded {
                    Occlusion::Occluded
                } else {
                    Occlusion::Visible
                };
                self.frames.occlusion_changed(occlusion, Instant::now());
                self.previous_frame = Instant::now();
            }
            WindowEvent::MouseInput {
                button: MouseButton::Left,
                state,
                ..
            } => {
                self.drag = match state {
                    ElementState::Pressed => DragState::Adjusting,
                    ElementState::Released => DragState::Idle,
                };
            }
            WindowEvent::CursorMoved { position, .. } => {
                let position = Point::new(position.x, position.y);
                if matches!(self.drag, DragState::Adjusting) {
                    let delta = position - self.mouse;
                    if delta.x.abs() > delta.y.abs() {
                        self.meter.scale.negative_db_range = (self.meter.scale.negative_db_range
                            + delta.x as f32 * 0.1)
                            .clamp(1.0, 100.0);
                    } else {
                        self.meter.scale.bend =
                            (self.meter.scale.bend * (1.0 + delta.y as f32 * 0.003)).max(0.1);
                    }
                }
                self.mouse = position;
            }
            WindowEvent::MouseWheel {
                delta: MouseScrollDelta::LineDelta(_, y),
                ..
            } => {
                let scale = f64::from((1.0 + y / 10.0).max(0.1));
                view.zoom(self.mouse, scale);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    match key {
                        KeyCode::Equal => {
                            self.meter.scale.negative_db_range =
                                (self.meter.scale.negative_db_range + 0.1).min(100.0)
                        }
                        KeyCode::Minus => {
                            self.meter.scale.negative_db_range =
                                (self.meter.scale.negative_db_range - 0.1).max(1.0)
                        }
                        KeyCode::BracketLeft => {
                            self.meter.scale.bend = (self.meter.scale.bend - 0.1).max(0.1)
                        }
                        KeyCode::BracketRight => self.meter.scale.bend += 0.1,
                        KeyCode::KeyZ => self.adjust_setting(Setting::Preamp, Adjustment::Decrease),
                        KeyCode::KeyX => self.adjust_setting(Setting::Preamp, Adjustment::Increase),
                        KeyCode::KeyQ => {
                            self.adjust_setting(Setting::FrameRate, Adjustment::Decrease)
                        }
                        KeyCode::KeyW => {
                            self.adjust_setting(Setting::FrameRate, Adjustment::Increase)
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.previous_frame).as_secs_f32();
                self.previous_frame = now;
                render.scene.reset();
                self.meter
                    .draw(&mut render.scene, view, self.settings.frame_rate_limit, dt);
                if matches!(render.present()?, Presentation::Unavailable) {
                    self.frames.surface_unavailable(Instant::now());
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Err(error) = self.settings.flush() {
            log::error!("{error:#}");
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !matches!(self.state, WindowState::Suspended) {
            return;
        }
        self.state = match RenderWindow::new(event_loop) {
            Ok(render) => {
                let view = View::fit(render.viewport());
                let now = Instant::now();
                self.frames.resized(render.window.inner_size().into(), now);
                self.frames.occlusion_changed(Occlusion::Visible, now);
                self.drag = DragState::Idle;
                WindowState::Active {
                    render: Box::new(render),
                    view,
                }
            }
            Err(error) => {
                event_loop.exit();
                WindowState::Failed(error)
            }
        };
        self.previous_frame = Instant::now();
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if matches!(self.state, WindowState::Active { .. }) {
            self.state = WindowState::Suspended;
            self.frames.resized(SurfaceArea::Empty, Instant::now());
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::RedrawRequested)
            && self.frames.take_redraw_permit() == RedrawPermit::Idle
        {
            return;
        }
        if matches!(event, WindowEvent::CloseRequested) {
            event_loop.exit();
            return;
        }
        if let Err(error) = self.window_event(event) {
            self.state = WindowState::Failed(error);
            self.frames.resized(SurfaceArea::Empty, Instant::now());
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(event) = self.rx.try_recv() {
            self.meter.audio_event(event);
        }
        let now = Instant::now();
        if let Err(error) = self.settings.flush_if_due(now) {
            log::error!("{error:#}");
        }
        if self.frames.is_due(now) {
            match self.frames.kind(now) {
                FrameKind::Visible => {
                    self.frames
                        .request_visible_frame(now, self.settings.frame_rate_limit);
                    let WindowState::Active { render, .. } = &self.state else {
                        unreachable!("a drawable surface owns its window")
                    };
                    render.window.request_redraw();
                }
                FrameKind::Background => self.frames.background_tick_finished(now),
            }
        } else if self.frames.redraw_is_pending() && self.frames.kind(now) == FrameKind::Visible {
            let WindowState::Active { render, .. } = &self.state else {
                unreachable!("a pending redraw owns its window")
            };
            render.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.frames.deadline(now)));
    }
}

fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();
    let settings = Saved::<Config>::load(config::config_file()?)?;
    let meter = Meter::new(settings.preamp_db)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let _stream = capture_stream(tx)?;
    let event_loop = EventLoop::new()?;
    let mut app = App {
        state: WindowState::Suspended,
        rx,
        meter,
        settings,
        mouse: Point::ORIGIN,
        drag: DragState::Idle,
        frames: FrameSchedule::new(Instant::now()),
        previous_frame: Instant::now(),
    };
    let result = event_loop.run_app(&mut app);
    app.settings.flush()?;
    result?;
    match app.state {
        WindowState::Failed(error) => Err(error),
        _ => Ok(()),
    }
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
