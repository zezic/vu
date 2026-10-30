use std::{sync::mpsc::Receiver, time::Instant};

use audio::capture_stream;
use config::{Config, CursorVisibility, PreampDb, PreampStep};
use frame_schedule::{
    Adjustment, FrameKind, FrameSchedule, FrameTime, Occlusion, RedrawPermit, SurfaceArea,
};
use render::RenderWindow;
use saved::Saved;
use vello::{kurbo::Point, wgpu};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Fullscreen, WindowId},
};

use visualizer::{ControlAction, Dial, Ui, View};
use vu::{audio, config, frame_schedule, profile, render, visualizer, AudioEvent};
mod saved;

enum WindowState {
    Suspended,
    Active(Box<ActiveWindow>),
    Failed(anyhow::Error),
}

struct ActiveWindow {
    render: RenderWindow,
    view: View,
    visualizer_renderer: visualizer::Renderer,
}

impl ActiveWindow {
    fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let render = RenderWindow::new(event_loop, wgpu::DeviceDescriptor::default())?;
        let mut view = View::fit(render.viewport());
        view.set_ui_scale(render.window.scale_factor());
        let visualizer_renderer =
            visualizer::Renderer::new(&render.device().device, render.format());
        Ok(Self {
            render,
            view,
            visualizer_renderer,
        })
    }
}

enum Setting {
    Theme,
    PresentationOpacity,
    Preamp,
    FrameRate,
    Shutter,
    DropGapThreshold,
    DepthFade,
    NearOpacity,
    CreaseThreshold,
    LineWidth,
    Zoom,
    MotionOpacity,
    MotionFadePower,
}

struct App {
    state: WindowState,
    rx: Receiver<AudioEvent>,
    ui: Ui,
    settings: Saved<Config>,
    mouse: Point,
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
            Setting::Shutter => {
                settings.shutter_frames = settings.shutter_frames.adjusted(adjustment);
                self.ui.set_shutter(settings.shutter_frames);
            }
            Setting::Theme => self.ui.navigate_theme(adjustment),
            Setting::PresentationOpacity => {
                settings.presentation_opacity = settings.presentation_opacity.adjusted(adjustment);
                self.ui
                    .set_presentation_opacity(settings.presentation_opacity);
            }
            Setting::Preamp => {
                let step = if self.modifiers.shift_key() {
                    PreampStep::Fine
                } else {
                    PreampStep::Coarse
                };
                settings.preamp_db = settings.preamp_db.adjusted(adjustment, step);
                self.ui.set_preamp(settings.preamp_db);
            }
            Setting::FrameRate => {
                settings.frame_rate_limit = settings.frame_rate_limit.adjusted(adjustment);
                self.frames.limit_changed(Instant::now());
            }
            Setting::DropGapThreshold => {
                let mut detectors = self.ui.detector_settings();
                let step = visualizer::Parameter::DropGapThreshold.step();
                let delta = match adjustment {
                    Adjustment::Decrease => -step,
                    Adjustment::Increase => step,
                };
                detectors.drop_gap_threshold = (detectors.drop_gap_threshold + delta).clamp(
                    visualizer::DROP_GAP_THRESHOLD_MIN,
                    visualizer::DROP_GAP_THRESHOLD_MAX,
                );
                settings.detectors = detectors.clone();
                self.ui.set_detector_settings(detectors);
            }
            Setting::DepthFade => {
                settings.depth_fade = settings.depth_fade.adjusted(adjustment);
                self.ui.set_depth_fade(settings.depth_fade);
            }
            Setting::NearOpacity => {
                settings.near_opacity = settings.near_opacity.adjusted(adjustment);
                self.ui.set_near_opacity(settings.near_opacity);
            }
            Setting::CreaseThreshold => {
                settings.crease_threshold = settings.crease_threshold.adjusted(adjustment);
                self.ui.set_crease_threshold(settings.crease_threshold);
            }
            Setting::Zoom => {
                settings.zoom = settings.zoom.adjusted(adjustment);
                let WindowState::Active(window) = &mut self.state else {
                    unreachable!("zoom requires an active window");
                };
                window.view.set_zoom(settings.zoom);
            }
            Setting::LineWidth => {
                settings.line_width = settings.line_width.adjusted(adjustment);
                self.ui.set_line_width(settings.line_width);
            }
            Setting::MotionOpacity => {
                settings.motion_opacity = settings.motion_opacity.adjusted(adjustment);
                self.ui.set_motion_opacity(settings.motion_opacity);
            }
            Setting::MotionFadePower => {
                settings.motion_fade_power = settings.motion_fade_power.adjusted(adjustment);
                self.ui.set_motion_fade_power(settings.motion_fade_power);
            }
        }
    }

    fn set_fullscreen(&mut self, fullscreen: bool) {
        self.ui.reset_presentation_clicks();
        let WindowState::Active(window) = &self.state else {
            unreachable!("fullscreen requires an active window");
        };
        window
            .render
            .window
            .set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
        self.settings.edit().fullscreen = fullscreen;
    }

    fn apply_control(&mut self, action: ControlAction) {
        match action {
            ControlAction::Fullscreen => {
                let WindowState::Active(window) = &self.state else {
                    unreachable!("a control requires an active window");
                };
                self.set_fullscreen(window.render.window.fullscreen().is_none());
            }
            ControlAction::Adjust(dial, adjustment) => {
                let setting = match dial {
                    Dial::Theme => Setting::Theme,
                    Dial::Model => {
                        self.ui.navigate_model(adjustment);
                        return;
                    }
                    Dial::LineWidth => Setting::LineWidth,
                    Dial::Zoom => Setting::Zoom,
                    Dial::FrameRate => Setting::FrameRate,
                    Dial::Shutter => Setting::Shutter,
                    Dial::TextOpacity => Setting::PresentationOpacity,
                };
                self.adjust_setting(setting, adjustment);
            }
            ControlAction::AutoColors => {
                let preferences = self.ui.theme_preferences_mut();
                preferences.auto_colors = !preferences.auto_colors;
            }
            ControlAction::Contrast(contrast) => {
                self.ui.theme_preferences_mut().contrast = contrast
            }
            ControlAction::Tone(tone) => self.ui.choose_tone(tone),
            ControlAction::ModelLock => {
                let rotation = self.settings.model_rotation.toggled();
                self.settings.edit().model_rotation = rotation;
                self.ui.set_model_rotation(rotation);
            }
            ControlAction::Boost(boost) => {
                self.settings.edit().preamp_db = boost;
                self.ui.set_preamp(boost);
            }
            ControlAction::MotionBlur => {
                let motion = self.settings.model_motion.toggled();
                self.settings.edit().model_motion = motion;
                self.ui.set_model_motion(motion);
            }
            ControlAction::HideCursor => {
                let visibility = match self.settings.cursor_visibility {
                    CursorVisibility::Always => CursorVisibility::AutoHide,
                    CursorVisibility::AutoHide => CursorVisibility::Always,
                };
                self.settings.edit().cursor_visibility = visibility;
            }
            ControlAction::Dislike => self.ui.dislike_theme(),
            ControlAction::SaveDefault => {
                let theme = self.ui.selected_theme();
                self.settings.edit().theme = theme;
                self.ui.choose_theme(theme);
            }
            ControlAction::Restore(theme) => {
                self.ui.theme_preferences_mut().restore(theme);
            }
            ControlAction::DeveloperMode => {
                self.ui.set_presentation(false);
                self.ui.set_debug(true);
            }
            ControlAction::Manage => {}
        }
        self.sync_theme_preferences();
    }

    fn sync_theme_preferences(&mut self) {
        if &self.settings.theme_preferences != self.ui.theme_preferences() {
            self.settings.edit().theme_preferences = self.ui.theme_preferences().clone();
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
            visualizer_renderer,
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
                let fullscreen = render.window.fullscreen().is_some();
                if self.settings.fullscreen != fullscreen {
                    self.settings.edit().fullscreen = fullscreen;
                }
                render.resize(size);
                *view = View::fit(render.viewport());
                view.set_zoom(self.settings.zoom);
                view.set_ui_scale(render.window.scale_factor());
                self.frames.resized(size.into(), Instant::now());
                self.ui.set_plate_mask(Some(render.plate_mask_image()));
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                view.set_ui_scale(scale_factor);
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
            } => match state {
                ElementState::Pressed => {
                    if self.ui.presentation() {
                        if let Some(action) = self.ui.click_controls(view, self.mouse) {
                            self.apply_control(action);
                        }
                    } else {
                        self.ui.click_debug(view, self.mouse);
                    }
                }
                ElementState::Released => self.ui.end_slider_drag(),
            },
            WindowEvent::CursorMoved { position, .. } => {
                let position = Point::new(position.x, position.y);
                self.ui.set_pointer(Some(position));
                render.window.set_cursor_visible(true);
                self.mouse = position;
            }
            WindowEvent::CursorLeft { .. } => {
                self.ui.set_pointer(None);
                render.window.set_cursor_visible(true);
                self.ui.end_slider_drag();
            }
            WindowEvent::Focused(false) => {
                self.modifiers = ModifiersState::empty();
                self.ui.set_control_modifiers(false);
                self.ui.set_pointer(None);
                render.window.set_cursor_visible(true);
                self.ui.end_slider_drag();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                self.ui.set_control_modifiers(!self.modifiers.is_empty());
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    MouseScrollDelta::PixelDelta(position) => position.y / 30.0,
                };
                if self.ui.presentation() {
                    self.ui.scroll_controls(lines);
                } else {
                    let zoom = self.settings.zoom.scaled((1.0 + lines / 10.0).max(0.1));
                    self.settings.edit().zoom = zoom;
                    view.set_zoom(zoom);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    if self.ui.presentation() {
                        if key == KeyCode::Escape && !event.repeat {
                            self.set_fullscreen(false);
                        }
                        return Ok(());
                    }
                    match key {
                        KeyCode::Escape => self.ui.clear_debug_selection(),
                        KeyCode::KeyC => {
                            self.adjust_setting(Setting::Shutter, Adjustment::Decrease)
                        }
                        KeyCode::KeyV => {
                            self.adjust_setting(Setting::Shutter, Adjustment::Increase)
                        }
                        KeyCode::Minus | KeyCode::NumpadSubtract if !event.repeat => {
                            let scale = self
                                .settings
                                .visualizer_text_scale
                                .adjusted(Adjustment::Decrease);
                            self.settings.edit().visualizer_text_scale = scale;
                            self.ui.set_text_scale(scale);
                        }
                        KeyCode::Equal | KeyCode::NumpadAdd if !event.repeat => {
                            let scale = self
                                .settings
                                .visualizer_text_scale
                                .adjusted(Adjustment::Increase);
                            self.settings.edit().visualizer_text_scale = scale;
                            self.ui.set_text_scale(scale);
                        }
                        KeyCode::KeyB if !event.repeat => {
                            self.ui.cycle_debug_refresh();
                        }
                        KeyCode::KeyD if !event.repeat => {
                            self.ui.set_debug(!self.ui.debug());
                        }
                        KeyCode::KeyG if !event.repeat => {
                            self.ui.set_timeline(!self.ui.timeline());
                        }
                        KeyCode::KeyJ => {
                            self.adjust_setting(Setting::DropGapThreshold, Adjustment::Decrease)
                        }
                        KeyCode::KeyK => {
                            self.adjust_setting(Setting::DropGapThreshold, Adjustment::Increase)
                        }
                        KeyCode::KeyO => {
                            self.adjust_setting(Setting::DepthFade, Adjustment::Decrease)
                        }
                        KeyCode::KeyP => {
                            self.adjust_setting(Setting::DepthFade, Adjustment::Increase)
                        }
                        KeyCode::KeyU => {
                            self.adjust_setting(Setting::NearOpacity, Adjustment::Decrease)
                        }
                        KeyCode::KeyI => {
                            self.adjust_setting(Setting::NearOpacity, Adjustment::Increase)
                        }
                        KeyCode::Comma => {
                            self.adjust_setting(Setting::CreaseThreshold, Adjustment::Decrease)
                        }
                        KeyCode::Period => {
                            self.adjust_setting(Setting::CreaseThreshold, Adjustment::Increase)
                        }
                        KeyCode::Backslash if !event.repeat => self.ui.next_model(),
                        KeyCode::KeyT if !event.repeat => {
                            let model_motion = self.settings.model_motion.toggled();
                            self.settings.edit().model_motion = model_motion;
                            self.ui.set_model_motion(model_motion);
                        }
                        KeyCode::Semicolon => {
                            self.adjust_setting(Setting::LineWidth, Adjustment::Decrease)
                        }
                        KeyCode::Quote => {
                            self.adjust_setting(Setting::LineWidth, Adjustment::Increase)
                        }
                        KeyCode::KeyL => {
                            self.adjust_setting(Setting::MotionOpacity, Adjustment::Decrease)
                        }
                        KeyCode::KeyR => {
                            self.adjust_setting(Setting::MotionOpacity, Adjustment::Increase)
                        }
                        KeyCode::KeyY => {
                            let adjustment = if self.modifiers.shift_key() {
                                Adjustment::Decrease
                            } else {
                                Adjustment::Increase
                            };
                            self.adjust_setting(Setting::MotionFadePower, adjustment)
                        }
                        KeyCode::KeyF if !event.repeat => self.ui.set_presentation(true),
                        KeyCode::KeyA => self.adjust_setting(Setting::Theme, Adjustment::Decrease),
                        KeyCode::KeyS => self.adjust_setting(Setting::Theme, Adjustment::Increase),
                        KeyCode::BracketLeft => {
                            self.adjust_setting(Setting::PresentationOpacity, Adjustment::Decrease)
                        }
                        KeyCode::BracketRight => {
                            self.adjust_setting(Setting::PresentationOpacity, Adjustment::Increase)
                        }
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
                    self.ui.audio_event(event);
                }
                let detectors = self.ui.detector_settings();
                if self.settings.detectors != detectors {
                    self.settings.edit().detectors = detectors;
                }
                let tuning = self.ui.visualizer_tuning();
                if self.settings.tuning != tuning {
                    self.settings.edit().tuning = tuning;
                }
                if &self.settings.theme_preferences != self.ui.theme_preferences() {
                    self.settings.edit().theme_preferences = self.ui.theme_preferences().clone();
                }
                let now = Instant::now();
                let top_overlay = self.profile.is_none()
                    && self.ui.draw_controls(
                        &mut render.overlay_scene,
                        view,
                        &self.settings,
                        render.window.fullscreen().is_some(),
                        now,
                    );
                render
                    .window
                    .set_cursor_visible(self.ui.cursor_visible(self.settings.cursor_visibility));
                let frame_time = FrameTime::new(self.previous_frame, now);
                self.previous_frame = now;
                if let Some(profile) = &mut self.profile {
                    profile.feed(&mut self.ui, now)?;
                }
                let before_draw = Instant::now();
                render.scene.reset();
                let frame = self.ui.draw(&mut render.scene, view, limit, frame_time);
                let after_draw = Instant::now();
                let changed =
                    matches!(frame.background, render::Frame::Rebuilt(_)) || frame.foreground;
                let viewport = render.viewport();
                let content = render::FrameContent {
                    background: frame.background,
                    foreground: frame.foreground,
                    top_overlay,
                    raster_assets: &frame.raster_assets,
                    plates: frame.plate_instances,
                    plate_ink: frame.plate_ink,
                };
                let overlay =
                    |device: &vello::util::DeviceHandle,
                     encoder: &mut vello::wgpu::CommandEncoder,
                     background: &vello::wgpu::TextureView,
                     target: &vello::wgpu::TextureView,
                     foreground: Option<&vello::wgpu::TextureView>,
                     top: Option<&vello::wgpu::TextureView>| {
                        visualizer_renderer.draw(
                            device,
                            encoder,
                            background,
                            target,
                            viewport,
                            frame.overlay,
                            foreground,
                            frame.overlay_composite,
                            frame.overlay_opacity,
                            top,
                        );
                    };
                let presentation = if let Some(profile) = &mut self.profile {
                    profile.present(before_draw, after_draw, changed, |completed| {
                        render.present(content, completed, overlay)
                    })?
                } else {
                    render.present(content, |_| {}, overlay)?
                };
                if matches!(presentation, render::Presentation::Unavailable) {
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
        self.sync_theme_preferences();
        if let Err(error) = self.settings.flush() {
            log::error!("{error:#}");
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !matches!(self.state, WindowState::Suspended) {
            return;
        }
        self.state = match ActiveWindow::new(event_loop) {
            Ok(mut window) => {
                window.view.set_zoom(self.settings.zoom);
                window.render.window.set_fullscreen(
                    self.settings
                        .fullscreen
                        .then_some(Fullscreen::Borderless(None)),
                );
                let now = Instant::now();
                self.frames
                    .resized(window.render.window.inner_size().into(), now);
                self.frames.occlusion_changed(Occlusion::Visible, now);
                self.ui
                    .set_plate_mask(Some(window.render.plate_mask_image()));
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
            self.ui.audio_event(event);
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
    let mut ui = Ui::new(&settings)?;
    ui.set_presentation(true);
    let (tx, rx) = std::sync::mpsc::channel();
    let _stream = if profile.is_some() {
        ui.set_preamp(PreampDb::try_from(0.0)?);
        ui.audio_event(AudioEvent::Config {
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
        ui,
        settings,
        mouse: Point::ORIGIN,
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
