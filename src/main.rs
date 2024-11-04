use std::sync::mpsc::Receiver;

use audio::audio_thread;
use biquad::{Biquad, Coefficients, DirectForm2Transposed, ToHertz, Q_BUTTERWORTH_F32};
use femtovg::{renderer::OpenGl, Align, Canvas, Color, FontId, Paint, Path};
use glutin::{
    context::PossiblyCurrentContext,
    surface::{Surface, WindowSurface},
};
use instant::Instant;
use log::info;
use processor::Processor;
use resource::resource;
use scales::{draw_scale, generate_din_scale, Mark};
use usvg::{
    tiny_skia_path::{PathSegment, Point},
    Node,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::KeyCode,
    window::Window,
};

const MOTION_FILTER_CUTOFF: f32 = 4.0;

mod audio;
mod helpers;
mod motion_filter;
mod processor;
mod scales;

use helpers::PerfGraph;

enum AudioEvent {
    Config { samplerate: usize },
    Buffer { buf: Vec<f32> },
}

const VU_WIDTH: f32 = 320.0;

fn main() {
    pretty_env_logger::init();
    info!("Hi");
    let (tx, rx) = std::sync::mpsc::channel();
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    std::thread::spawn(|| audio_thread(tx, shutdown_rx));
    helpers::start((VU_WIDTH * 2.0) as u32 * 2, 220 * 2, "VU", true, rx);

    shutdown_tx.send(()).unwrap();
}

use glutin::prelude::*;

struct App {
    dragging: bool,
    mouse: (f32, f32),
    prevt: Instant,
    canvas: Canvas<OpenGl>,
    context: PossiblyCurrentContext,
    surface: Surface<WindowSurface>,
    perf: PerfGraph,
    window: Window,
    _paths: Vec<(Path, Option<Paint>, Option<Paint>)>,
    rx: Receiver<AudioEvent>,
    processor: Processor,
    last_hand_pos: [(f32, f32); 2],
    last_last_hand_pos: [(f32, f32); 2],
    negative_db_range: f32,
    bend: f32,
    font_ids: Vec<FontId>,
    marks: Vec<Mark>,
    overload: [f32; 2],
    filter: [DirectForm2Transposed<f32>; 2],
    last_fps: u32,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        info!("resumed... and what?")
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        // info!("{:?}", event);
        event_loop.set_control_flow(ControlFlow::Poll);

        while let Ok(data) = self.rx.try_recv() {
            match data {
                AudioEvent::Config { samplerate } => {
                    self.processor.set_samplerate(samplerate);
                }
                AudioEvent::Buffer { buf } => {
                    self.processor.consume_buf(buf);
                }
            }
        }

        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if !event.state.is_pressed() {
                    return;
                }
                if let winit::keyboard::PhysicalKey::Code(key_code) = event.physical_key {
                    let step = 0.1;
                    match key_code {
                        KeyCode::Equal => {
                            self.negative_db_range += step;
                            info!("negative_db_range: {}", self.negative_db_range);
                        }
                        KeyCode::Minus => {
                            self.negative_db_range -= step;
                            info!("negative_db_range: {}", self.negative_db_range);
                        }
                        KeyCode::BracketLeft => {
                            self.bend -= step;
                            info!("bend: {}", self.bend);
                        }
                        KeyCode::BracketRight => {
                            self.bend += step;
                            info!("bend: {}", self.bend);
                        }
                        KeyCode::KeyZ => {
                            let preamp = multiplier_to_db(self.processor.preamp);
                            let db = (preamp - 6.0).clamp(-96.0, 96.0);
                            self.processor.preamp = db_to_multiplier(db);
                            info!("preamp: {}", db);
                        }
                        KeyCode::KeyX => {
                            let preamp = multiplier_to_db(self.processor.preamp);
                            let db = (preamp + 6.0).clamp(-96.0, 96.0);
                            self.processor.preamp = db_to_multiplier(db);
                            info!("preamp: {}", db);
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::Resized(physical_size) => {
                self.surface.resize(
                    &self.context,
                    physical_size.width.try_into().unwrap(),
                    physical_size.height.try_into().unwrap(),
                );
            }
            WindowEvent::MouseInput {
                button: MouseButton::Left,
                state,
                ..
            } => match state {
                ElementState::Pressed => self.dragging = true,
                ElementState::Released => self.dragging = false,
            },
            WindowEvent::CursorMoved {
                device_id: _,
                position,
                ..
            } => {
                if self.dragging {
                    // let p0 = self.canvas.transform().inversed().transform_point(self.mouse.0, self.mouse.1);
                    // let p1 = self.canvas
                    //     .transform()
                    //     .inversed()
                    //     .transform_point(position.x as f32, position.y as f32);
                    // self.canvas.translate(p1.0 - p0.0, p1.1 - p0.1);
                    let dx = position.x as f32 - self.mouse.0;
                    let dy = position.y as f32 - self.mouse.1;

                    if dx.abs() > dy.abs() {
                        self.negative_db_range += dx * 0.1;
                        self.negative_db_range = self.negative_db_range.min(100.0);
                        info!("negative_db_range: {}", self.negative_db_range);
                    } else {
                        self.bend *= 1.0 + dy * 0.003;
                        info!("bend: {}", self.bend);
                    }
                }

                self.mouse.0 = position.x as f32;
                self.mouse.1 = position.y as f32;
            }
            WindowEvent::MouseWheel {
                device_id: _,
                delta: winit::event::MouseScrollDelta::LineDelta(_, y),
                ..
            } => {
                let pt = self
                    .canvas
                    .transform()
                    .inversed()
                    .transform_point(self.mouse.0, self.mouse.1);
                self.canvas.translate(pt.0, pt.1);
                self.canvas.scale(1.0 + (y / 10.0), 1.0 + (y / 10.0));
                self.canvas.translate(-pt.0, -pt.1);
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.prevt).as_secs_f32();
                self.prevt = now;

                self.perf.update(dt);

                let dpi_factor = self.window.scale_factor();
                let size = self.window.inner_size();

                self.canvas
                    .set_size(size.width, size.height, dpi_factor as f32);
                self.canvas
                    .clear_rect(0, 0, size.width, size.height, Color::rgb(40, 36, 36));

                let rms = self.processor.get_hands_for_instant(Instant::now());

                // Stats
                let mut paint = Paint::color(Color::rgb(80, 72, 72));
                paint.set_text_align(Align::Center);

                let stats_start = 150.0;
                for (idx, (label, value)) in [(
                    "PREAMP",
                    format!("{:.1}dB", multiplier_to_db(self.processor.preamp)),
                )]
                .into_iter()
                .enumerate()
                {
                    let stat_y = stats_start + idx as f32 * 30.0;
                    paint.set_font_size(8.0);
                    self.canvas
                        .fill_text(VU_WIDTH, stat_y, label, &paint)
                        .unwrap();
                    paint.set_font_size(12.0);
                    self.canvas
                        .fill_text(VU_WIDTH, stat_y + 12.0, value, &paint)
                        .unwrap();
                }

                // Filters
                // paint.set_text_align(Align::Left);
                // let lines = format!("{:#?}", self.filter);
                // for (idx, line) in lines.lines().enumerate() {
                //     self.canvas
                //         .fill_text(10.0, 10.0 + idx as f32 * 10.0, line, &paint)
                //         .unwrap();
                // }

                const MAX_ANGLE: f32 = 47.0;
                let center_y = 207.0;

                // Scales
                {
                    self.canvas.save();
                    self.canvas.translate(VU_WIDTH / 2.0, center_y);
                    draw_scale(
                        &mut self.canvas,
                        self.font_ids[0],
                        &self.marks,
                        MAX_ANGLE,
                        self.negative_db_range,
                        6.0,
                        self.bend,
                    );
                    self.canvas.translate(VU_WIDTH, 0.0);
                    draw_scale(
                        &mut self.canvas,
                        self.font_ids[0],
                        &self.marks,
                        MAX_ANGLE,
                        self.negative_db_range,
                        6.0,
                        self.bend,
                    );
                    self.canvas.restore();
                }

                {
                    // Overload
                    for (idx, rms) in rms.into_iter().enumerate() {
                        let overload = rms >= 1.0;
                        if overload {
                            self.overload[idx] += dt * 25.0;
                            self.overload[idx] = self.overload[idx].clamp(0.0, 1.0);
                        } else {
                            self.overload[idx] -= dt * 5.0;
                            self.overload[idx] = self.overload[idx].clamp(0.0, 1.0);
                        }
                        let x_base = VU_WIDTH * idx as f32 + VU_WIDTH / 2.0;

                        // Hole
                        let mut path = Path::new();
                        path.circle(x_base, center_y * 0.6, 7.0);
                        let paint = Paint::color(Color::rgbaf(0.0, 0.0, 0.0, 0.25));
                        self.canvas.fill_path(&path, &paint);

                        // Glow
                        let mut path = Path::new();
                        const GLOW_SIZE: f32 = 40.0;
                        path.circle(x_base, center_y * 0.6, GLOW_SIZE);
                        let paint = Paint::radial_gradient(
                            x_base,
                            center_y * 0.6,
                            5.0,
                            GLOW_SIZE,
                            Color::rgbaf(
                                1.0,
                                self.overload[idx].powf(2.0) * 0.2,
                                self.overload[idx].powf(4.0) * 0.1,
                                self.overload[idx].powf(2.0) * 0.2,
                            ),
                            Color::rgbaf(1.0, 0.0, 0.0, 0.0),
                        );
                        self.canvas.fill_path(&path, &paint);

                        // Light
                        let mut path = Path::new();
                        path.circle(x_base, center_y * 0.6, 5.0);
                        let paint = Paint::color(Color::rgbaf(
                            1.0,
                            self.overload[idx].powf(2.0) * 0.9,
                            self.overload[idx].powf(4.0) * 0.8,
                            self.overload[idx],
                        ));
                        self.canvas.fill_path(&path, &paint);
                    }

                    let fps = (1.0 / self.perf.get_average()).max(MOTION_FILTER_CUTOFF * 2.0) as u32;

                    if fps != self.last_fps {
                        for filter in &mut self.filter {
                            filter.update_coefficients(
                                Coefficients::<f32>::from_params(
                                    biquad::Type::LowPass,
                                    fps.hz(),
                                    MOTION_FILTER_CUTOFF.hz(),
                                    Q_BUTTERWORTH_F32,
                                )
                                .unwrap(),
                            );
                        }
                    }
                    self.last_fps = fps;

                    // Hands
                    for (idx, rms) in rms.into_iter().enumerate() {
                        let rms = normalized_to_db(rms, self.negative_db_range);
                        let rms = (rms + self.negative_db_range).max(0.0);
                        let rms = rms / (self.negative_db_range + 6.0);
                        let rms = rms.powf(self.bend);

                        let filter = &mut self.filter[idx];

                        let rms = filter.run(rms);
                        if rms.is_nan() {
                            info!("rms 5 is nan");
                        }

                        let x_base = VU_WIDTH * idx as f32 + VU_WIDTH / 2.0;

                        // Convert value from [0.0, 1.0] to angle range [-45°, 45°] in radians
                        let angle = (rms * MAX_ANGLE * 2.0 - 90.0 - MAX_ANGLE)
                            * (std::f32::consts::PI / 180.0); // Convert degrees to radians

                        // Radius (distance from center)
                        let radius = 174.0;

                        // Calculate x and y coordinates using polar to Cartesian conversion
                        let x = radius * angle.cos();
                        let y = radius * angle.sin();

                        let center_y = 207.0;

                        let mut path_0 = Path::new();
                        path_0.move_to(x_base, center_y);
                        path_0.line_to(x_base + x, center_y + y);
                        let mut paint_0 = Paint::color(Color::rgb(255, 200, 160));
                        paint_0.set_line_width(1.0);

                        let mut path_1 = Path::new();
                        path_1.move_to(x_base, center_y);
                        path_1.line_to(x_base + x, center_y + y);
                        path_1.line_to(
                            self.last_last_hand_pos[idx].0,
                            self.last_last_hand_pos[idx].1,
                        );
                        let paint_1 = Paint::color(Color::rgba(255, 48, 0, 100));
                        self.canvas.fill_path(&path_1, &paint_1);

                        let mut path_1 = Path::new();
                        path_1.move_to(x_base, center_y);
                        path_1.line_to(x_base + x, center_y + y);
                        path_1.line_to(self.last_hand_pos[idx].0, self.last_hand_pos[idx].1);
                        let paint_1 = Paint::color(Color::rgba(255, 48, 0, 100));
                        self.canvas.fill_path(&path_1, &paint_1);

                        self.canvas.stroke_path(&path_0, &paint_0);

                        self.last_last_hand_pos = self.last_hand_pos;
                        self.last_hand_pos[idx] = (x_base + x, center_y + y);
                    }
                }

                // self.canvas.save();
                // self.canvas.reset();
                // self.perf.render(&mut self.canvas, 5.0, 215.0);
                // self.canvas.restore();

                self.canvas.flush();
                self.surface.swap_buffers(&self.context).unwrap();
            }
            _ => (),
        }

        self.window.request_redraw();

        // match event {
        //     Event::MainEventsCleared => window.request_redraw(),
        //     _ => (),
        // }
    }
}

pub fn multiplier_to_db(multiplier: f32) -> f32 {
    20.0 * multiplier.log10()
}

pub fn db_to_multiplier(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

pub fn normalized_to_db(value: f32, negative_db_range: f32) -> f32 {
    if value <= 0.0 {
        -negative_db_range // Return -100 dB for a value of 0.0
    } else {
        20.0 * value.log10()
    }
}

pub fn db_to_normalized(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

fn run(
    mut canvas: Canvas<OpenGl>,
    el: EventLoop<()>,
    context: glutin::context::PossiblyCurrentContext,
    surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
    window: Window,
    rx: Receiver<AudioEvent>,
) {
    let mut font_ids = vec![];

    font_ids.push(
        canvas
            .add_font_mem(&resource!("assets/D-DINExp.ttf"))
            .expect("Cannot add font"),
    );

    font_ids.push(
        canvas
            .add_font_mem(&resource!("assets/D-DINExp-Bold.ttf"))
            .expect("Cannot add font"),
    );

    let start = Instant::now();
    let prevt = start;

    let mousex = 0.0;
    let mousey = 0.0;
    let dragging = false;

    let perf = PerfGraph::new();

    let svg_data = include_str!("assets/Sifam_Type_32A_DIN_scale_PPM_curves.svg").as_bytes();
    let tree = usvg::Tree::from_data(svg_data, &usvg::Options::default()).unwrap();

    let paths = render_svg(tree);

    // print memory usage
    let mut total_sisze_bytes = 0;

    for path in &paths {
        total_sisze_bytes += path.0.size();
    }

    log::info!("Path mem usage: {}kb", total_sisze_bytes / 1024);

    canvas.scale(2.0, 2.0);

    let mut app = App {
        canvas,
        context,
        surface,
        window,
        dragging,
        mouse: (mousex, mousey),
        prevt,
        perf,
        _paths: paths,
        rx,
        processor: Processor::new(),
        last_hand_pos: Default::default(),
        last_last_hand_pos: Default::default(),
        negative_db_range: 53.4,
        bend: 2.0,
        font_ids,
        marks: generate_din_scale(),
        overload: Default::default(),
        filter: [
            DirectForm2Transposed::<f32>::new(
                Coefficients::<f32>::from_params(
                    biquad::Type::LowPass,
                    60.hz(),
                    MOTION_FILTER_CUTOFF.hz(),
                    Q_BUTTERWORTH_F32,
                )
                .unwrap(),
            ),
            DirectForm2Transposed::<f32>::new(
                Coefficients::<f32>::from_params(
                    biquad::Type::LowPass,
                    60.hz(),
                    MOTION_FILTER_CUTOFF.hz(),
                    Q_BUTTERWORTH_F32,
                )
                .unwrap(),
            ),
        ],
        last_fps: 60,
    };

    el.run_app(&mut app).unwrap();
}

fn render_svg(svg: usvg::Tree) -> Vec<(Path, Option<Paint>, Option<Paint>)> {
    // use usvg::NodeKind;
    // use usvg::PathSegment;

    let mut paths = Vec::new();

    for node in svg.root().children() {
        handle_node(node, &mut paths);
    }

    paths
}

fn handle_node(node: &Node, paths: &mut Vec<(Path, Option<Paint>, Option<Paint>)>) {
    match node {
        Node::Path(svg_path) => {
            handle_path(svg_path, paths);
        }
        Node::Group(svg_group) => {
            for child in svg_group.children() {
                handle_node(child, paths);
            }
        }
        _ => {}
    }
}

fn handle_path(svg_path: &Box<usvg::Path>, paths: &mut Vec<(Path, Option<Paint>, Option<Paint>)>) {
    let mut path = Path::new();

    for command in svg_path.data().segments() {
        match command {
            PathSegment::MoveTo(Point { x, y }) => path.move_to(x as f32, y as f32),
            PathSegment::LineTo(Point { x, y }) => path.line_to(x as f32, y as f32),
            PathSegment::CubicTo(
                Point { x: x1, y: y1 },
                Point { x: x2, y: y2 },
                Point { x, y },
            ) => path.bezier_to(
                x1 as f32, y1 as f32, x2 as f32, y2 as f32, x as f32, y as f32,
            ),
            PathSegment::QuadTo(Point { x: x1, y: y1 }, Point { x: x2, y: y2 }) => {
                path.quad_to(x1 as f32, y1 as f32, x2 as f32, y2 as f32)
            }
            PathSegment::Close => path.close(),
        }
    }

    let to_femto_color = |usvg_paint: &usvg::Paint| match usvg_paint {
        usvg::Paint::Color(usvg::Color { red, green, blue }) => {
            Some(Color::rgb(*red, *green, *blue))
        }
        _ => None,
    };

    let fill = svg_path
        .fill()
        .as_ref()
        .and_then(|fill| to_femto_color(&fill.paint()))
        .map(|col| Paint::color(col).with_anti_alias(true));

    let stroke = svg_path.stroke().as_ref().and_then(|stroke| {
        to_femto_color(&stroke.paint()).map(|paint| {
            let mut stroke_paint = Paint::color(paint);
            stroke_paint.set_line_width(stroke.width().get() as f32);
            stroke_paint.set_anti_alias(true);
            stroke_paint
        })
    });

    paths.push((path, fill, stroke))
}
