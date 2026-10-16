use std::{
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use audio::capture_stream;
use config::{Config, PreampStep};
use frame_schedule::{
    Adjustment, FrameKind, FrameSchedule, FrameTime, Occlusion, RedrawPermit, SurfaceArea,
};
use meter::{Meter, View};
use render::{Presentation, RenderWindow};
use saved::Saved;
use scales::ScaleSetting;
use vello::kurbo::Point;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::WindowId,
};

use vu::{audio, config, frame_schedule, measurement, meter, profile, render, scales, AudioEvent};
mod saved;

enum WindowState {
    Suspended,
    Active(Box<ActiveWindow>),
    Failed(anyhow::Error),
}

struct ActiveWindow {
    render: RenderWindow,
    view: View,
    meter_renderer: meter::Renderer,
}

impl ActiveWindow {
    fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let render = RenderWindow::new(event_loop, meter::Renderer::device_preferences())?;
        let view = View::fit(render.viewport());
        let meter_renderer = meter::Renderer::new(&render.device().device, render.format());
        Ok(Self {
            render,
            view,
            meter_renderer,
        })
    }
}

enum DragState {
    Idle,
    Pending(Point),
    Adjusting(ScaleSetting),
}

struct Click {
    at: Instant,
    position: Point,
}

impl Click {
    fn follows(&self, position: Point, now: Instant) -> bool {
        now.duration_since(self.at) <= Duration::from_millis(400)
            && self.position.distance(position) <= 6.0
    }
}

impl DragState {
    fn setting(&self) -> Option<ScaleSetting> {
        match self {
            Self::Adjusting(setting) => Some(*setting),
            _ => None,
        }
    }
}

enum Setting {
    Mode,
    PeakMovement,
    Theme,
    Preamp,
    FrameRate,
    Shutter,
}

struct App {
    state: WindowState,
    rx: Receiver<AudioEvent>,
    meter: Meter,
    settings: Saved<Config>,
    mouse: Point,
    drag: DragState,
    last_click: Option<Click>,
    modifiers: ModifiersState,
    frames: FrameSchedule,
    previous_frame: Instant,
    profile: Option<profile::Run>,
}

impl App {
    fn frame_rate_limit(&self) -> frame_schedule::FrameRateLimit {
        self.profile
            .as_ref()
            .map_or(self.settings.frame_rate_limit, |_| {
                frame_schedule::FrameRateLimit::Fps240
            })
    }

    fn adjust_setting(&mut self, setting: Setting, adjustment: Adjustment) {
        let settings = self.settings.edit();
        match setting {
            Setting::PeakMovement => {
                settings.peak_movement = settings.peak_movement.next();
                self.meter.set_peak_movement(settings.peak_movement);
            }
            Setting::Mode => {
                settings.mode = settings.mode.next();
                self.meter.set_mode(settings.mode);
                self.drag = DragState::Idle;
            }
            Setting::Shutter => {
                settings.shutter_frames = settings.shutter_frames.adjusted(adjustment);
                self.meter.set_shutter(settings.shutter_frames);
            }
            Setting::Theme => {
                settings.theme = settings.theme.adjusted(adjustment);
                self.meter.set_theme(settings.theme);
            }
            Setting::Preamp => {
                let step = if self.modifiers.shift_key() {
                    PreampStep::Fine
                } else {
                    PreampStep::Coarse
                };
                settings.preamp_db = settings.preamp_db.adjusted(adjustment, step);
                self.meter.set_preamp(settings.preamp_db);
            }
            Setting::FrameRate => {
                settings.frame_rate_limit = settings.frame_rate_limit.adjusted(adjustment);
                self.frames.limit_changed(Instant::now());
            }
        }
    }

    fn window_event(&mut self, event: WindowEvent) -> anyhow::Result<()> {
        let limit = self.frame_rate_limit();
        let WindowState::Active(window) = &mut self.state else {
            return Ok(());
        };
        let ActiveWindow {
            render,
            view,
            meter_renderer,
        } = &mut **window;
        if self.profile.is_some()
            && !matches!(
                event,
                WindowEvent::Resized(_) | WindowEvent::Occluded(_) | WindowEvent::RedrawRequested
            )
        {
            return Ok(());
        }
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
                let now = Instant::now();
                match state {
                    ElementState::Pressed => {
                        if self
                            .last_click
                            .take()
                            .is_some_and(|click| click.follows(self.mouse, now))
                        {
                            self.meter.reset_scale();
                        }
                        self.drag = DragState::Pending(self.mouse);
                    }
                    ElementState::Released => {
                        if matches!(self.drag, DragState::Pending(_)) {
                            self.meter.repair_needles();
                            self.last_click = Some(Click {
                                at: now,
                                position: self.mouse,
                            });
                        }
                        self.drag = DragState::Idle;
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let position = Point::new(position.x, position.y);
                let mut delta = position - self.mouse;
                if let DragState::Pending(origin) = self.drag {
                    let movement = position - origin;
                    if self.settings.mode != measurement::Mode::Vu && movement.hypot() >= 3.0 {
                        self.drag = DragState::Adjusting(if movement.x.abs() > movement.y.abs() {
                            ScaleSetting::DbRange
                        } else {
                            ScaleSetting::Bend
                        });
                        delta = movement;
                    }
                }
                match (self.settings.mode, self.drag.setting()) {
                    (measurement::Mode::Vu, _) => {}
                    (_, Some(ScaleSetting::DbRange)) => {
                        self.meter.scale.negative_db_range = (self.meter.scale.negative_db_range
                            + delta.x as f32 * 0.1)
                            .clamp(1.0, 100.0);
                    }
                    (_, Some(ScaleSetting::Bend)) => {
                        self.meter.scale.bend =
                            (self.meter.scale.bend * (1.0 + delta.y as f32 * 0.003)).max(0.1);
                    }
                    (_, None) => {}
                }
                self.mouse = position;
            }
            WindowEvent::Focused(false) => {
                self.modifiers = ModifiersState::empty();
                self.drag = DragState::Idle;
                self.last_click = None;
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
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
                        KeyCode::Equal if self.settings.mode != measurement::Mode::Vu => {
                            self.meter.scale.negative_db_range =
                                (self.meter.scale.negative_db_range + 0.1).min(100.0)
                        }
                        KeyCode::Minus if self.settings.mode != measurement::Mode::Vu => {
                            self.meter.scale.negative_db_range =
                                (self.meter.scale.negative_db_range - 0.1).max(1.0)
                        }
                        KeyCode::BracketLeft if self.settings.mode != measurement::Mode::Vu => {
                            self.meter.scale.bend = (self.meter.scale.bend - 0.1).max(0.1)
                        }
                        KeyCode::BracketRight if self.settings.mode != measurement::Mode::Vu => {
                            self.meter.scale.bend += 0.1
                        }
                        KeyCode::KeyC => {
                            self.adjust_setting(Setting::Shutter, Adjustment::Decrease)
                        }
                        KeyCode::KeyV => {
                            self.adjust_setting(Setting::Shutter, Adjustment::Increase)
                        }
                        KeyCode::KeyM if !event.repeat => {
                            self.adjust_setting(Setting::Mode, Adjustment::Increase)
                        }
                        KeyCode::KeyN
                            if !event.repeat && self.settings.mode.uses_peak_movement() =>
                        {
                            self.adjust_setting(Setting::PeakMovement, Adjustment::Increase)
                        }
                        KeyCode::KeyA => self.adjust_setting(Setting::Theme, Adjustment::Decrease),
                        KeyCode::KeyS => self.adjust_setting(Setting::Theme, Adjustment::Increase),
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
                while let Ok(event) = self.rx.try_recv() {
                    self.meter.audio_event(event);
                }
                let now = Instant::now();
                let frame_time = FrameTime::new(self.previous_frame, now);
                self.previous_frame = now;
                if let Some(profile) = &mut self.profile {
                    profile.feed(&mut self.meter, now)?;
                }
                let before_draw = Instant::now();
                render.scene.reset();
                let frame = self.meter.draw(
                    &mut render.scene,
                    view,
                    limit,
                    self.drag.setting(),
                    frame_time,
                );
                let after_draw = Instant::now();
                let changed = !matches!(frame.background, render::Frame::Unchanged(_));
                let viewport = render.viewport();
                let overlay = |device: &vello::util::DeviceHandle,
                               encoder: &mut vello::wgpu::CommandEncoder,
                               background: &vello::wgpu::TextureView,
                               target: &vello::wgpu::TextureView| {
                    meter_renderer.draw(
                        device,
                        encoder,
                        background,
                        target,
                        viewport,
                        &frame.overlay,
                    );
                };
                let presentation = if let Some(profile) = &mut self.profile {
                    profile.present(before_draw, after_draw, changed, |completed| {
                        render.present(frame.background, completed, overlay)
                    })?
                } else {
                    render.present(frame.background, |_| {}, overlay)?
                };
                if matches!(presentation, Presentation::Unavailable) {
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
        self.state = match ActiveWindow::new(event_loop) {
            Ok(window) => {
                let now = Instant::now();
                self.frames
                    .resized(window.render.window.inner_size().into(), now);
                self.frames.occlusion_changed(Occlusion::Visible, now);
                self.drag = DragState::Idle;
                self.last_click = None;
                WindowState::Active(Box::new(window))
            }
            Err(error) => {
                event_loop.exit();
                WindowState::Failed(error)
            }
        };
        self.previous_frame = Instant::now();
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.modifiers = ModifiersState::empty();
        if matches!(self.state, WindowState::Active(_)) {
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
        if self
            .profile
            .as_ref()
            .is_some_and(|profile| profile.finished(now))
        {
            event_loop.exit();
            return;
        }
        if let Err(error) = self.settings.flush_if_due(now) {
            log::error!("{error:#}");
        }
        if self.frames.is_due(now) {
            match self.frames.kind(now) {
                FrameKind::Visible => {
                    self.frames
                        .request_visible_frame(now, self.frame_rate_limit());
                    let WindowState::Active(window) = &self.state else {
                        unreachable!("a drawable surface owns its window")
                    };
                    window.render.window.request_redraw();
                }
                FrameKind::Background => self.frames.background_tick_finished(now),
            }
        } else if self.frames.redraw_is_pending() && self.frames.kind(now) == FrameKind::Visible {
            let WindowState::Active(window) = &self.state else {
                unreachable!("a pending redraw owns its window")
            };
            window.render.window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.frames.deadline(now)));
    }
}

fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();
    let profile = profile::Run::from_args()?;
    let settings = Saved::<Config>::load(config::config_file()?)?;
    let mut meter = Meter::new(&settings)?;
    let (tx, rx) = std::sync::mpsc::channel();
    let _stream = if profile.is_some() {
        meter.set_preamp(config::PreampDb::try_from(0.0)?);
        meter.audio_event(AudioEvent::Config {
            samplerate: profile::SAMPLE_RATE,
        });
        None
    } else {
        Some(capture_stream(tx)?)
    };
    let event_loop = EventLoop::new()?;
    let mut app = App {
        state: WindowState::Suspended,
        rx,
        meter,
        settings,
        mouse: Point::ORIGIN,
        drag: DragState::Idle,
        last_click: None,
        modifiers: ModifiersState::empty(),
        frames: FrameSchedule::new(Instant::now()),
        previous_frame: Instant::now(),
        profile,
    };
    let result = event_loop.run_app(&mut app);
    app.settings.flush()?;
    result?;
    if let WindowState::Failed(error) = app.state {
        return Err(error);
    }
    if let Some(profile) = &app.profile {
        if let WindowState::Active(window) = &app.state {
            let info = window.render.device().adapter().get_info();
            let size = window.render.viewport();
            eprintln!(
                "Target: {}x{} pixels; {} ({:?})",
                size.width, size.height, info.name, info.backend
            );
        }
        profile.report()?;
    }
    Ok(())
}

pub use vu::{db_to_multiplier, db_to_normalized, normalized_to_db};
