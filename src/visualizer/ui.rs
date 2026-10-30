use std::{collections::HashMap, time::Instant};

use strum::{EnumCount, VariantArray};
use vello::{
    kurbo::{Affine, Point, Rect, Size},
    peniko::{Color, Fill},
    Scene,
};

use crate::{
    audio::AudioCursor,
    config::{Config, PreampDb, PresentationOpacity, ShutterFrames, VisualizerTextScale},
    frame_schedule::{FrameRateLimit, FrameTime},
    render::{Frame, PlateInstance, PlateMaskImage, RasterAsset},
    text::{Fonts, Text, UiFace},
    theme::{Theme, ThemeColors, ThemeId},
    AudioEvent,
};

use super::{
    debug, presentation::Presentation, Contribution, CreaseThreshold, DebugSelection,
    DebugSnapshot, DepthFade, Detector, DetectorSettings, DropKernel, Frame as VisualizerFrame,
    LineWidth, ModelComposite, ModelMotion, MotionFadePower, MotionOpacity, NearOpacity, Palette,
    Parameter, PatternKernel, PatternLayer, Property, TreeTarget, Visualizer, VisualizerTuning,
    BAND_COUNT,
};

pub(super) const HUD_MARGIN: f64 = 24.0;

/// Window-relative drawing state shared by the HUD and the inspector.
#[derive(Clone, Copy, PartialEq)]
pub struct View {
    pub(super) viewport: Size,
    zoom: f64,
    ui_scale: f64,
}

impl View {
    pub fn fit(viewport: Size) -> Self {
        assert!(
            viewport.width > 0.0 && viewport.height > 0.0,
            "a drawable viewport must have an area"
        );
        Self {
            viewport,
            zoom: 1.0,
            ui_scale: 1.0,
        }
    }

    pub fn zoom(&mut self, factor: f64) {
        assert!(factor > 0.0, "zoom must preserve a positive scale");
        self.zoom = (self.zoom * factor).clamp(0.35, 3.0);
    }

    /// Physical pixels per logical UI point, independent of window size.
    pub fn ui_scale(&self) -> f64 {
        self.ui_scale
    }

    pub fn set_ui_scale(&mut self, scale: f64) {
        assert!(
            scale.is_finite() && scale > 0.0,
            "the UI scale must be finite and positive"
        );
        self.ui_scale = scale;
    }

    pub fn zoom_factor(&self) -> f64 {
        self.zoom
    }
}

#[derive(PartialEq)]
enum HintOrder {
    BeforeValue,
    AfterValue,
}

struct Readout {
    caption: Option<Text>,
    hints: Vec<Text>,
}

impl Readout {
    fn value(fonts: &mut Fonts, text: &str) -> Text {
        fonts.layout(text, 12.0)
    }

    fn new(fonts: &mut Fonts, caption: &str, hints: [&str; 2]) -> Self {
        Self {
            caption: Some(fonts.layout_ui(caption, 8.0, UiFace::Light)),
            hints: hints
                .map(|hint| fonts.layout_ui(hint, 10.0, UiFace::Light))
                .into(),
        }
    }

    /// A caption-only readout for a value without a hotkey.
    fn label(fonts: &mut Fonts, caption: &str) -> Self {
        Self {
            caption: Some(fonts.layout_ui(caption, 8.0, UiFace::Light)),
            hints: Vec::new(),
        }
    }

    fn inline<const N: usize>(fonts: &mut Fonts, hints: [&str; N]) -> Self {
        Self {
            caption: None,
            hints: hints
                .map(|hint| fonts.layout_ui(hint, 10.0, UiFace::Light))
                .into(),
        }
    }

    fn inline_width(&self, value: &Text) -> f64 {
        assert!(
            self.caption.is_none(),
            "inline width belongs to a readout without a caption"
        );
        self.hints
            .iter()
            .map(|hint| hint.width() + 8.0)
            .sum::<f64>()
            + value.width()
    }

    /// Left-aligned caption, value and hints for the full-window HUD.
    fn draw_left_aligned(
        &self,
        scene: &mut Scene,
        transform: Affine,
        value: &Text,
        colors: &ThemeColors,
    ) {
        let baseline = transform * Affine::translate((0.0, 12.0));
        match &self.caption {
            Some(caption) => {
                caption.draw_left(scene, transform, colors.readout);
                value.draw_left(scene, baseline, colors.readout);
                let mut x = value.width() + 8.0;
                for hint in &self.hints {
                    hint.draw_left(scene, baseline * Affine::translate((x, 0.0)), colors.hint);
                    x += hint.width() + 8.0;
                }
            }
            None => self.draw_inline(scene, transform, value, colors, HintOrder::BeforeValue),
        }
    }

    fn draw_inline(
        &self,
        scene: &mut Scene,
        transform: Affine,
        value: &Text,
        colors: &ThemeColors,
        order: HintOrder,
    ) {
        assert!(self.caption.is_none(), "inline readouts have no caption");
        let hints = self.hints.iter().map(|hint| (hint, colors.hint));
        let value = (value, colors.readout);
        let items = (order == HintOrder::AfterValue)
            .then_some(value)
            .into_iter()
            .chain(hints)
            .chain((order == HintOrder::BeforeValue).then_some(value));
        let mut x = 0.0;
        for (text, color) in items {
            text.draw_left(scene, transform * Affine::translate((x, 0.0)), color);
            x += text.width() + 8.0;
        }
    }
}

/// How often the retained inspector face is refreshed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugRefresh {
    /// Once per completed FFT hop.
    #[default]
    PerHop,
    Hz20,
    Hz10,
    Hz5,
    Hz2,
    Hz1,
}

impl DebugRefresh {
    fn interval(self) -> Option<f32> {
        match self {
            Self::PerHop => None,
            Self::Hz20 => Some(0.05),
            Self::Hz10 => Some(0.1),
            Self::Hz5 => Some(0.2),
            Self::Hz2 => Some(0.5),
            Self::Hz1 => Some(1.0),
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::PerHop => Self::Hz20,
            Self::Hz20 => Self::Hz10,
            Self::Hz10 => Self::Hz5,
            Self::Hz5 => Self::Hz2,
            Self::Hz2 => Self::Hz1,
            Self::Hz1 => Self::PerHop,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::PerHop => "Hop",
            Self::Hz20 => "20Hz",
            Self::Hz10 => "10Hz",
            Self::Hz5 => "5Hz",
            Self::Hz2 => "2Hz",
            Self::Hz1 => "1Hz",
        }
    }
}

struct SliderDrag {
    parameter: Parameter,
    track: Rect,
}

#[derive(Clone, Copy)]
struct KernelDrag {
    kernel: DropKernel,
    rect: Rect,
    last: Point,
}

#[derive(Clone, Copy)]
struct PatternDrag {
    detector: Detector,
    canvas: Rect,
    kernel: PatternKernel,
    layer: PatternLayer,
    vertex: usize,
}

#[derive(Clone, Copy, PartialEq)]
struct FaceSettings {
    viewport: Size,
    ui_scale: f64,
    limit: FrameRateLimit,
    colors: ThemeColors,
    palette: Palette,
    debug: bool,
    presentation: bool,
    text_scale: VisualizerTextScale,
}

/// The interactive model visualizer: HUD, inspector and audio cursor.
pub struct Ui {
    scene_readout: Readout,
    scene_value: Text,
    scene_name: String,
    line_count: usize,
    line_count_value: Text,
    model_position: Option<(usize, usize)>,
    line_width: LineWidth,
    model_motion: ModelMotion,
    motion_opacity: MotionOpacity,
    motion_fade_power: MotionFadePower,
    line_status_value: Text,
    debug_readout: Readout,
    debug_value: Text,
    debug: bool,
    timeline_readout: Readout,
    timeline_value: Text,
    timeline: bool,
    timeline_labels: debug::TimelineLabels,
    debug_refresh: DebugRefresh,
    debug_snapshot: Option<DebugSnapshot>,
    debug_selection: Option<DebugSelection>,
    tree_target: Option<TreeTarget>,
    flux_band: usize,
    graph_max: [Option<debug::GraphMax>; Property::COUNT],
    debug_pointer: Option<Point>,
    slider_drag: Option<SliderDrag>,
    kernel_drag: Option<KernelDrag>,
    pattern_drag: Option<PatternDrag>,
    temporal_view: debug::TemporalView,
    offset_drag: Option<Rect>,
    pattern_click: Option<(Instant, Point)>,
    debug_clock: f32,
    debug_analysis_revision: u64,
    text_scale: VisualizerTextScale,
    presentation_opacity: PresentationOpacity,
    text_scale_readout: Readout,
    text_scale_value: Text,
    detectors: DetectorSettings,
    kick_gap_readout: Readout,
    kick_gap_value: Text,
    drop_gap_readout: Readout,
    drop_gap_value: Text,
    depth_fade: DepthFade,
    depth_fade_readout: Readout,
    depth_fade_value: Text,
    near_opacity: NearOpacity,
    near_opacity_readout: Readout,
    near_opacity_value: Text,
    crease_threshold: CreaseThreshold,
    crease_readout: Readout,
    crease_value: Text,
    diagnosis_text: Option<Text>,
    diagnosis_anchor: Point,
    diagnosis_color: Color,
    diagnosis_revision: u64,
    diagnosis_visible_last_frame: bool,
    visualizer: Visualizer,
    preamp: PreampDb,
    audio: AudioCursor,
    theme: Theme,
    theme_readout: Readout,
    theme_value: Text,
    shutter_frames: ShutterFrames,
    shutter_readout: Readout,
    shutter_value: Text,
    fonts: Fonts,
    preamp_readout: Readout,
    preamp_value: Text,
    frame_readout: Readout,
    frame_values: HashMap<FrameRateLimit, Text>,
    face: Scene,
    face_settings: Option<FaceSettings>,
    presentation: Presentation,
    plate_mask: Option<PlateMaskImage>,
}

impl Ui {
    fn line_status(
        position: Option<(usize, usize)>,
        motion: ModelMotion,
        width: LineWidth,
        opacity: MotionOpacity,
        power: MotionFadePower,
    ) -> String {
        let mut parts = Vec::new();
        if let Some((current, total)) = position {
            parts.push(format!("MODEL [\\] {current}/{total}"));
        }
        parts.push(format!("BLUR [T] {}", motion.name()));
        parts.push(format!("WIDTH [;] ['] {:>3}%", width.percent()));
        parts.push(format!("FLOOR [L] [R] {:>3}%", opacity.percent()));
        parts.push(format!("POWER [Y] {:.1}", power.power()));
        parts.join("   ")
    }

    fn layout_line_status(
        fonts: &mut Fonts,
        position: Option<(usize, usize)>,
        motion: ModelMotion,
        width: LineWidth,
        opacity: MotionOpacity,
        power: MotionFadePower,
    ) -> Text {
        let status = Self::line_status(position, motion, width, opacity, power);
        fonts.layout(&status, 11.0)
    }

    fn refresh_line_status(&mut self) {
        self.line_status_value = Self::layout_line_status(
            &mut self.fonts,
            self.model_position,
            self.model_motion,
            self.line_width,
            self.motion_opacity,
            self.motion_fade_power,
        );
        self.face_settings = None;
    }

    /// Unscaled width of the bottom-right hotkey line, from its left caption to
    /// the shutter readout. The light timeline uses the same width and scale.
    fn hotkey_line_width(&self) -> f64 {
        let shutter = self.shutter_readout.inline_width(&self.shutter_value);
        shutter + 16.0 + self.line_count_value.width() + 24.0 + self.line_status_value.width()
    }

    fn model_composite(theme: ThemeId) -> ModelComposite {
        if theme.is_light() {
            ModelComposite::Multiplicative
        } else {
            ModelComposite::Additive
        }
    }

    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let selected = config.theme;
        let mut fonts = Fonts::new()?;
        let scene_readout = Readout::inline(&mut fonts, ["MODEL"]);
        let debug_readout = Readout::inline(&mut fonts, ["[D]", "[B]"]);
        let debug_value = Readout::value(&mut fonts, "Debug Off");
        let timeline_readout = Readout::inline(&mut fonts, ["[G]"]);
        let timeline_value = Readout::value(&mut fonts, "Timeline Off");
        let timeline_labels = debug::TimelineLabels::new(&mut fonts);
        let text_scale = config.visualizer_text_scale;
        let presentation_opacity = config.presentation_opacity;
        let text_scale_readout = Readout::inline(&mut fonts, ["[-]", "[+]"]);
        let text_scale_value =
            Readout::value(&mut fonts, &format!("Text {:>3}%", text_scale.percent()));
        let detectors = config.detectors.clone();
        let mut visualizer = Visualizer::new();
        visualizer.set_model_composite(Self::model_composite(selected));
        visualizer.set_detector_settings(detectors.clone());
        let kick_gap_readout = Readout::label(&mut fonts, "Kick Gap");
        let kick_gap_value = Readout::value(
            &mut fonts,
            &format!("{:.0}ms", detectors.kick_onset_min_interval * 1000.0),
        );
        let drop_gap_readout = Readout::new(&mut fonts, "Drop Gate", ["[J]", "[K]"]);
        let drop_gap_value = Readout::value(
            &mut fonts,
            &format!("{:.0} dB", detectors.drop_gap_threshold),
        );
        let depth_fade = config.depth_fade;
        visualizer.set_depth_fade(depth_fade);
        let depth_fade_readout = Readout::new(&mut fonts, "Depth Fade", ["[O]", "[P]"]);
        let depth_fade_value = Readout::value(&mut fonts, &format!("{:>3}%", depth_fade.percent()));
        let near_opacity = config.near_opacity;
        visualizer.set_near_opacity(near_opacity);
        visualizer.set_tuning(config.tuning.clone());
        let near_opacity_readout = Readout::new(&mut fonts, "Near Opacity", ["[U]", "[I]"]);
        let near_opacity_value =
            Readout::value(&mut fonts, &format!("{:>3}%", near_opacity.percent()));
        let crease_threshold = config.crease_threshold;
        visualizer.set_crease_threshold(crease_threshold);
        let crease_readout = Readout::new(&mut fonts, "Crease", ["[,]", "[.]"]);
        let crease_value = Readout::value(
            &mut fonts,
            &format!("{:>3}", crease_threshold.whole_degrees()),
        );
        let line_width = config.line_width;
        visualizer.set_line_width(line_width);
        let model_motion = config.model_motion;
        visualizer.set_model_motion(model_motion);
        let motion_opacity = config.motion_opacity;
        visualizer.set_motion_opacity(motion_opacity);
        let motion_fade_power = config.motion_fade_power;
        visualizer.set_motion_fade_power(motion_fade_power);
        let scene_name = visualizer.scene_name().to_owned();
        let scene_value = fonts.layout_ui(&scene_name, 12.0, UiFace::SemiBold);
        let line_count = visualizer.line_count();
        let line_count_value = fonts.layout(&format!("LINES {line_count}"), 12.0);
        let model_position = visualizer.model_position();
        let line_status_value = Self::layout_line_status(
            &mut fonts,
            model_position,
            model_motion,
            line_width,
            motion_opacity,
            motion_fade_power,
        );
        let theme_readout = Readout::inline(&mut fonts, ["[A]", "[S]"]);
        let theme_value = Readout::value(&mut fonts, selected.name());
        let shutter_readout = Readout::inline(&mut fonts, ["[C]", "[V]"]);
        let shutter_value = Readout::value(
            &mut fonts,
            &format!("Shutter {:>4.1}f", config.shutter_frames.frames()),
        );
        let preamp_readout = Readout::new(&mut fonts, "Preamp", ["[Z]", "[X]"]);
        let frame_readout = Readout::new(&mut fonts, "FPS Limit", ["[Q]", "[W]"]);
        let frame_values = FrameRateLimit::VARIANTS
            .iter()
            .map(|&limit| (limit, Readout::value(&mut fonts, &format!("{limit:>5}"))))
            .collect();
        let preamp = config.preamp_db;
        let preamp_value = Readout::value(&mut fonts, &format!("{:.1}dB", preamp.db()));
        Ok(Self {
            scene_readout,
            scene_value,
            scene_name,
            line_count,
            line_count_value,
            model_position,
            line_width,
            model_motion,
            motion_opacity,
            motion_fade_power,
            line_status_value,
            debug_readout,
            debug_value,
            debug: false,
            timeline_readout,
            timeline_value,
            timeline: false,
            timeline_labels,
            debug_refresh: DebugRefresh::default(),
            debug_snapshot: None,
            debug_selection: None,
            tree_target: None,
            flux_band: 0,
            graph_max: [None; Property::COUNT],
            debug_pointer: None,
            slider_drag: None,
            kernel_drag: None,
            pattern_drag: None,
            temporal_view: debug::TemporalView::editing(),
            offset_drag: None,
            pattern_click: None,
            debug_clock: 0.0,
            debug_analysis_revision: 0,
            text_scale,
            presentation_opacity,
            text_scale_readout,
            text_scale_value,
            detectors,
            kick_gap_readout,
            kick_gap_value,
            drop_gap_readout,
            drop_gap_value,
            depth_fade,
            depth_fade_readout,
            depth_fade_value,
            near_opacity,
            near_opacity_readout,
            near_opacity_value,
            crease_threshold,
            crease_readout,
            crease_value,
            diagnosis_text: None,
            diagnosis_anchor: Point::ORIGIN,
            diagnosis_color: Color::new([1.0, 1.0, 1.0, 1.0]),
            diagnosis_revision: 0,
            diagnosis_visible_last_frame: false,
            visualizer,
            preamp,
            audio: AudioCursor::new(),
            theme: Theme::new(selected),
            theme_readout,
            theme_value,
            shutter_frames: config.shutter_frames,
            shutter_readout,
            shutter_value,
            fonts,
            preamp_readout,
            preamp_value,
            frame_readout,
            frame_values,
            face: Scene::new(),
            face_settings: None,
            presentation: Presentation::new(),
            plate_mask: None,
        })
    }

    pub fn audio_event(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Config { samplerate } => {
                self.audio.set_samplerate(samplerate);
                self.visualizer.set_samplerate(samplerate);
            }
            AudioEvent::Buffer { buf, captured_at } => self.consume_buf(&buf, captured_at),
        }
    }

    /// Borrowed output audio follows the standalone capture processing path.
    pub fn consume_buf(&mut self, buf: &[[f32; 2]], captured_at: Instant) {
        self.audio.push(buf.len(), captured_at);
        self.visualizer.consume_buf(buf, self.preamp.gain());
    }

    pub fn debug(&self) -> bool {
        self.debug
    }

    pub fn presentation(&self) -> bool {
        self.presentation.active()
    }

    pub fn set_presentation(&mut self, presentation: bool) {
        if self.presentation.active() == presentation {
            return;
        }
        self.presentation.set_active(presentation);
        if presentation {
            self.clear_drag();
        }
        self.face_settings = None;
    }

    /// Supplies the GPU coverage mask used by moving contrast plates.
    pub fn set_plate_mask(&mut self, mask: Option<PlateMaskImage>) {
        self.plate_mask = mask;
    }

    pub fn timeline(&self) -> bool {
        self.timeline
    }

    pub fn set_timeline(&mut self, timeline: bool) {
        if self.timeline == timeline {
            return;
        }
        self.timeline = timeline;
        let label = if timeline {
            "Timeline On"
        } else {
            "Timeline Off"
        };
        self.timeline_value = Readout::value(&mut self.fonts, label);
        self.face_settings = None;
    }

    pub fn set_debug(&mut self, debug: bool) {
        if self.debug == debug {
            return;
        }
        self.debug = debug;
        let label = self.debug_label();
        self.debug_value = Readout::value(&mut self.fonts, &label);
        self.debug_snapshot = None;
        self.debug_selection = None;
        self.tree_target = None;
        self.graph_max = [None; Property::COUNT];
        self.clear_drag();
        self.debug_clock = 0.0;
        self.face_settings = None;
    }

    /// Hover state for the analysis panel. The UI owns the layout, so the
    /// host only forwards the physical pointer position.
    pub fn set_pointer(&mut self, pointer: Option<Point>) {
        if self.debug_pointer == pointer {
            return;
        }
        self.debug_pointer = pointer;
        if self.presentation.active() {
            return;
        }
        self.update_sliders(pointer);
        self.update_kernels(pointer);
        self.update_pattern(pointer);
        self.update_offset(pointer);
        if self.debug {
            self.face_settings = None;
        }
    }

    /// Selects or clears an analysis item under the pointer. Returns true when
    /// the panel consumed the click.
    pub fn click_debug(&mut self, view: &View, position: Point) -> bool {
        if self.presentation.active() || !self.debug || self.debug_snapshot.is_none() {
            return false;
        }
        let property = match self.debug_selection {
            Some(DebugSelection::Property(property)) => Some(property),
            _ => None,
        };
        let scale = view.ui_scale() * f64::from(self.text_scale.factor());
        if let Some(property) = property {
            let tree = debug::tree_layout(
                view,
                self.text_scale,
                property,
                self.tree_target,
                self.visualizer.mixer(),
            );
            if tree.rect.width() >= debug::TREE_MIN_VISIBLE_WIDTH * scale
                && tree.rect.height() >= 200.0 * scale
                && tree.rect.contains(position)
            {
                if let Some(action) = tree.action_at(position) {
                    self.apply_tree_action(action);
                    return true;
                }
                if let Some(slider) = tree.slider_at(position) {
                    self.begin_slider(slider.parameter, slider.track, position);
                    return true;
                }
                if let Some((kernel, rect, _)) = tree.kernel_at(position) {
                    self.begin_kernel(kernel, rect, position);
                    return true;
                }
                if let Some(temporal) = tree.temporal {
                    if temporal.offset.contains(position) {
                        self.begin_offset(temporal.offset, position);
                        return true;
                    }
                    if temporal.canvas.contains(position) {
                        self.handle_pattern_click(
                            temporal.detector,
                            temporal.canvas,
                            position,
                            scale,
                        );
                        return true;
                    }
                }
                if let Some(row) = tree.row_at(position) {
                    if let Some(contribution) = row.contribution() {
                        let badge = Rect::new(
                            row.rect.x1 - 18.0 * scale,
                            row.rect.y0,
                            row.rect.x1 - 8.0 * scale,
                            row.rect.y1,
                        );
                        if badge.contains(position) {
                            self.visualizer.toggle_contribution_mix(contribution);
                            self.clear_drag();
                            self.face_settings = None;
                            return true;
                        }
                    }
                    self.tree_target = row.target();
                    self.clear_drag();
                    self.face_settings = None;
                }
                return true;
            }
        }
        let detector_focused = self.debug_selection.is_some_and(|selection| {
            debug::detector_focused(selection, self.tree_target, self.flux_band)
        });
        if !detector_focused {
            if let Some(selection) = self.debug_selection {
                if let Some((property, max)) = debug::graph_max_button_at(
                    view,
                    self.text_scale,
                    selection,
                    &self.visualizer,
                    position,
                ) {
                    let slot = &mut self.graph_max[property.index()];
                    *slot = if *slot == Some(max) { None } else { Some(max) };
                    self.clear_drag();
                    self.face_settings = None;
                    return true;
                }
            }
        }
        let layout = debug::Layout::new(view, self.text_scale);
        for (button, selection) in [
            (layout.hat_button, DebugSelection::Hat),
            (layout.scene_button, DebugSelection::Scene),
            (layout.flux_button, DebugSelection::Flux),
        ] {
            if button.contains(position) {
                self.debug_selection = Some(selection);
                self.tree_target = None;
                self.clear_drag();
                self.face_settings = None;
                return true;
            }
        }
        if self.debug_selection == Some(DebugSelection::Hat) {
            let panel = debug::hat_analyzer_layout(view, self.text_scale);
            let panel_scale = view.ui_scale() * f64::from(self.text_scale.factor());
            if panel.rect.width() >= debug::TREE_MIN_VISIBLE_WIDTH * panel_scale
                && panel.rect.height() >= 200.0 * panel_scale
                && panel.rect.contains(position)
            {
                if let Some(action) = panel.action_at(position) {
                    self.apply_tree_action(action);
                    return true;
                }
                if let Some(slider) = panel.slider_at(position) {
                    self.begin_slider(slider.parameter, slider.track, position);
                    return true;
                }
                if let Some(temporal) = panel.temporal {
                    if temporal.offset.contains(position) {
                        self.begin_offset(temporal.offset, position);
                        return true;
                    }
                    if temporal.canvas.contains(position) {
                        self.handle_pattern_click(
                            temporal.detector,
                            temporal.canvas,
                            position,
                            panel_scale,
                        );
                        return true;
                    }
                }
                return true;
            }
        }
        if self.debug_selection == Some(DebugSelection::Scene) {
            let panel = debug::scene_analyzer_layout(view, self.text_scale);
            let panel_scale = view.ui_scale() * f64::from(self.text_scale.factor());
            if panel.rect.width() >= debug::TREE_MIN_VISIBLE_WIDTH * panel_scale
                && panel.rect.height() >= 200.0 * panel_scale
                && panel.rect.contains(position)
            {
                if let Some(slider) = panel.slider_at(position) {
                    self.begin_slider(slider.parameter, slider.track, position);
                    return true;
                }
                return true;
            }
        }
        if self.debug_selection == Some(DebugSelection::Flux) {
            let panel = debug::flux_analyzer_layout(view, self.text_scale, self.flux_band);
            let panel_scale = view.ui_scale() * f64::from(self.text_scale.factor());
            if panel.rect.width() >= debug::TREE_MIN_VISIBLE_WIDTH * panel_scale
                && panel.rect.height() >= 200.0 * panel_scale
                && panel.rect.contains(position)
            {
                if let Some(action) = panel.action_at(position) {
                    self.apply_tree_action(action);
                    return true;
                }
                if let Some(slider) = panel.slider_at(position) {
                    self.begin_slider(slider.parameter, slider.track, position);
                    return true;
                }
                if let Some(band) = panel.flux_band_at(position) {
                    self.flux_band = band;
                    self.clear_drag();
                    self.face_settings = None;
                    return true;
                }
                return true;
            }
        }
        if !layout.panel.contains(position) {
            return false;
        }
        if let Some((property, button)) = layout.property_button_at(position) {
            match button {
                debug::PropertyButton::Mute => self.visualizer.toggle_property_mute(property),
                debug::PropertyButton::Solo => self.visualizer.toggle_property_solo(property),
            }
            self.clear_drag();
            self.face_settings = None;
            return true;
        }
        let selection = match layout.item_at(position) {
            Some(item) if self.debug_selection == Some(item) => None,
            Some(item) => Some(item),
            None => None,
        };
        if let Some(DebugSelection::Band(index)) = selection {
            self.flux_band = index;
        }
        if selection != self.debug_selection {
            self.tree_target = None;
            self.clear_drag();
        }
        self.debug_selection = selection;
        self.face_settings = None;
        true
    }

    fn apply_tree_action(&mut self, action: debug::TreeAction) {
        match action {
            debug::TreeAction::ToggleMix(contribution) => {
                self.visualizer.toggle_contribution_mix(contribution);
            }
            debug::TreeAction::AddExtra(point) => {
                let extra = self.visualizer.add_extra(point);
                self.tree_target = Some(TreeTarget::Contribution(Contribution::Extra(extra)));
            }
            debug::TreeAction::RemoveExtra(extra) => {
                self.visualizer.remove_extra(extra);
                let removed = TreeTarget::Contribution(Contribution::Extra(extra));
                if self.tree_target == Some(removed) {
                    self.tree_target = Some(TreeTarget::MixPoint(extra.point));
                }
            }
            debug::TreeAction::ToggleAdaptive(target) => self.visualizer.toggle_adaptive(target),
            debug::TreeAction::ClearPattern(detector) => {
                self.visualizer.clear_pattern(detector);
                self.set_detector_settings(self.visualizer.detector_settings());
            }
            debug::TreeAction::TogglePatternOverlay => {
                self.temporal_view.overlay = !self.temporal_view.overlay;
            }
            debug::TreeAction::TogglePatternPreview => {
                self.temporal_view.preview = !self.temporal_view.preview;
            }
            debug::TreeAction::TogglePatternPause => {
                self.temporal_view.paused = !self.temporal_view.paused;
                if self.temporal_view.paused {
                    self.temporal_view.frozen = Some(self.visualizer.onset_history().clone());
                } else {
                    self.temporal_view.frozen = None;
                }
                self.temporal_view.offset = 0.0;
            }
            debug::TreeAction::TogglePatternSuppression => {
                self.temporal_view.kernel = match self.temporal_view.kernel {
                    PatternKernel::Main => PatternKernel::Suppression,
                    PatternKernel::Suppression => PatternKernel::Main,
                };
            }
        }
        self.clear_drag();
        self.pattern_click = None;
        self.face_settings = None;
    }

    fn begin_slider(&mut self, parameter: Parameter, track: Rect, position: Point) {
        self.slider_drag = Some(SliderDrag { parameter, track });
        let value = Self::slider_value(parameter, track, position.x);
        self.set_parameter(parameter, value);
    }

    fn begin_kernel(&mut self, kernel: DropKernel, rect: Rect, position: Point) {
        self.slider_drag = None;
        self.kernel_drag = Some(KernelDrag {
            kernel,
            rect,
            last: position,
        });
        self.paint_kernel(kernel, rect, position, position);
        self.face_settings = None;
    }

    fn update_kernels(&mut self, pointer: Option<Point>) {
        let (Some(pointer), Some(drag)) = (pointer, self.kernel_drag) else {
            return;
        };
        self.paint_kernel(drag.kernel, drag.rect, drag.last, pointer);
        self.kernel_drag = Some(KernelDrag {
            last: pointer,
            ..drag
        });
    }

    fn paint_kernel(&mut self, kernel: DropKernel, rect: Rect, from: Point, to: Point) {
        let band_at = |x: f64| {
            (((x - rect.x0) / rect.width()).clamp(0.0, 0.999_999) * BAND_COUNT as f64) as usize
        };
        let value_at = |y: f64| ((rect.y1 - y) / rect.height()).clamp(0.0, 1.0) as f32;
        let from_band = band_at(from.x);
        let to_band = band_at(to.x);
        let from_value = value_at(from.y);
        let to_value = value_at(to.y);
        let (low, high) = if from_band <= to_band {
            (from_band, to_band)
        } else {
            (to_band, from_band)
        };
        for band in low..=high {
            let value = if from_band == to_band {
                to_value
            } else {
                let position =
                    (band as f64 - from_band as f64) / (to_band as f64 - from_band as f64);
                (f64::from(from_value) + (f64::from(to_value) - f64::from(from_value)) * position)
                    as f32
            };
            self.visualizer.set_drop_kernel_band(kernel, band, value);
        }
        self.detectors = self.visualizer.detector_settings();
        self.face_settings = None;
    }

    fn handle_pattern_click(
        &mut self,
        detector: Detector,
        canvas: Rect,
        position: Point,
        scale: f64,
    ) {
        let settings = self.visualizer.detector_settings();
        let pattern = match detector {
            Detector::Kick => &settings.kick_pattern,
            Detector::Snare => &settings.snare_pattern,
            Detector::Hat => &settings.hat_pattern,
            Detector::Drop | Detector::Flux => return,
        };
        let kernel = self.temporal_view.kernel;
        let hit = if self.temporal_view.overlay {
            debug::pattern_hit(pattern, kernel, canvas, position, scale)
        } else {
            None
        };
        let now = Instant::now();
        let double = self.pattern_click.take().is_some_and(|(at, previous)| {
            now.duration_since(at) <= std::time::Duration::from_millis(400)
                && previous.distance(position) <= 6.0
        });
        if double {
            match hit {
                Some(debug::PatternHit::Vertex { layer, index }) => {
                    let polygon = match (kernel, layer) {
                        (PatternKernel::Main, PatternLayer::Inner) => &pattern.inner,
                        (PatternKernel::Main, PatternLayer::Outer) => &pattern.outer,
                        (PatternKernel::Suppression, PatternLayer::Inner) => {
                            &pattern.suppression_inner
                        }
                        (PatternKernel::Suppression, PatternLayer::Outer) => {
                            &pattern.suppression_outer
                        }
                    };
                    if polygon.len() > 3 {
                        self.visualizer
                            .remove_pattern_vertex(detector, kernel, layer, index);
                        self.detectors = self.visualizer.detector_settings();
                    }
                }
                Some(debug::PatternHit::Edge {
                    layer,
                    index,
                    point,
                }) => {
                    let polygon = match (kernel, layer) {
                        (PatternKernel::Main, PatternLayer::Inner) => &pattern.inner,
                        (PatternKernel::Main, PatternLayer::Outer) => &pattern.outer,
                        (PatternKernel::Suppression, PatternLayer::Inner) => {
                            &pattern.suppression_inner
                        }
                        (PatternKernel::Suppression, PatternLayer::Outer) => {
                            &pattern.suppression_outer
                        }
                    };
                    if polygon.len() < 32 {
                        self.visualizer
                            .insert_pattern_vertex(detector, kernel, layer, index, point);
                        self.detectors = self.visualizer.detector_settings();
                    }
                }
                None => {}
            }
            self.clear_drag();
            self.face_settings = None;
            return;
        }
        self.pattern_click = Some((now, position));
        match hit {
            Some(debug::PatternHit::Vertex { layer, index }) => {
                self.begin_polygon(detector, canvas, kernel, layer, index);
            }
            _ => self.clear_drag(),
        }
        self.face_settings = None;
    }

    fn begin_polygon(
        &mut self,
        detector: Detector,
        canvas: Rect,
        kernel: PatternKernel,
        layer: PatternLayer,
        vertex: usize,
    ) {
        self.slider_drag = None;
        self.kernel_drag = None;
        self.pattern_drag = Some(PatternDrag {
            detector,
            canvas,
            kernel,
            layer,
            vertex,
        });
        self.face_settings = None;
    }

    fn update_pattern(&mut self, pointer: Option<Point>) {
        let (Some(pointer), Some(drag)) = (pointer, self.pattern_drag) else {
            return;
        };
        let point = [
            ((pointer.x - drag.canvas.x0) / drag.canvas.width()).clamp(0.0, 1.0) as f32,
            ((pointer.y - drag.canvas.y0) / drag.canvas.height()).clamp(0.0, 1.0) as f32,
        ];
        self.visualizer.set_pattern_vertex(
            drag.detector,
            drag.kernel,
            drag.layer,
            drag.vertex,
            point,
        );
        self.detectors = self.visualizer.detector_settings();
        self.face_settings = None;
    }

    fn begin_offset(&mut self, rect: Rect, position: Point) {
        self.slider_drag = None;
        self.kernel_drag = None;
        self.pattern_drag = None;
        self.pattern_click = None;
        self.offset_drag = Some(rect);
        if !self.temporal_view.paused {
            self.temporal_view.paused = true;
            self.temporal_view.frozen = Some(self.visualizer.onset_history().clone());
        }
        self.set_offset(rect, position.x);
        self.face_settings = None;
    }

    fn update_offset(&mut self, pointer: Option<Point>) {
        let (Some(pointer), Some(rect)) = (pointer, self.offset_drag) else {
            return;
        };
        self.set_offset(rect, pointer.x);
    }

    fn set_offset(&mut self, rect: Rect, x: f64) {
        let fraction = ((x - rect.x0) / rect.width()).clamp(0.0, 1.0);
        self.temporal_view.offset = ((fraction * 0.5) - 0.25) as f32;
        self.face_settings = None;
    }

    fn clear_drag(&mut self) {
        self.slider_drag = None;
        self.kernel_drag = None;
        self.pattern_drag = None;
        self.offset_drag = None;
    }

    /// Clears the inspector selection without changing the debug panel toggle.
    pub fn clear_debug_selection(&mut self) {
        if self.debug_selection.take().is_some() {
            self.tree_target = None;
            self.clear_drag();
            self.face_settings = None;
        }
    }

    /// Live detector settings, including slider edits, for config sync.
    pub fn detector_settings(&self) -> DetectorSettings {
        self.detectors.clone()
    }

    /// Live inspector tuning, including slider edits, for config sync.
    pub fn visualizer_tuning(&self) -> VisualizerTuning {
        self.visualizer.tuning()
    }

    fn set_parameter(&mut self, parameter: Parameter, value: f32) {
        self.visualizer.set_parameter(parameter, value);
        self.set_detector_settings(self.visualizer.detector_settings());
        self.face_settings = None;
    }

    pub fn end_slider_drag(&mut self) {
        self.clear_drag();
    }

    fn slider_value(parameter: Parameter, track: Rect, x: f64) -> f32 {
        let fraction = ((x - track.x0) / track.width().max(1.0)).clamp(0.0, 1.0) as f32;
        parameter.from_fraction(fraction)
    }

    fn update_sliders(&mut self, pointer: Option<Point>) {
        let Some(drag) = &self.slider_drag else {
            return;
        };
        let (parameter, track) = (drag.parameter, drag.track);
        let Some(pointer) = pointer else {
            return;
        };
        let value = Self::slider_value(parameter, track, pointer.x);
        self.set_parameter(parameter, value);
    }

    pub fn cycle_debug_refresh(&mut self) {
        self.debug_refresh = self.debug_refresh.next();
        let label = self.debug_label();
        self.debug_value = Readout::value(&mut self.fonts, &label);
        self.debug_snapshot = None;
        self.debug_clock = 0.0;
        self.face_settings = None;
    }

    fn debug_label(&self) -> String {
        if self.debug {
            format!("Debug {}", self.debug_refresh.name())
        } else {
            "Debug Off".to_owned()
        }
    }

    pub fn set_theme(&mut self, selected: ThemeId) {
        self.theme.select(selected);
        self.visualizer
            .set_model_composite(Self::model_composite(selected));
        self.theme_value = Readout::value(&mut self.fonts, selected.name());
        self.face_settings = None;
    }

    pub fn set_shutter(&mut self, shutter: ShutterFrames) {
        self.shutter_frames = shutter;
        self.shutter_value = Readout::value(
            &mut self.fonts,
            &format!("Shutter {:>4.1}f", shutter.frames()),
        );
        self.face_settings = None;
    }

    pub fn set_preamp(&mut self, preamp: PreampDb) {
        self.preamp = preamp;
        self.preamp_value = Readout::value(&mut self.fonts, &format!("{:.1}dB", preamp.db()));
        self.face_settings = None;
    }

    pub fn set_presentation_opacity(&mut self, opacity: PresentationOpacity) {
        self.presentation_opacity = opacity;
    }

    pub fn set_text_scale(&mut self, scale: VisualizerTextScale) {
        if self.text_scale == scale {
            return;
        }
        self.text_scale = scale;
        self.text_scale_value =
            Readout::value(&mut self.fonts, &format!("Text {:>3}%", scale.percent()));
        self.face_settings = None;
    }

    pub fn set_detector_settings(&mut self, settings: DetectorSettings) {
        if self.detectors == settings {
            return;
        }
        self.detectors = settings.clone();
        self.visualizer.set_detector_settings(settings.clone());
        self.kick_gap_value = Readout::value(
            &mut self.fonts,
            &format!("{:.0}ms", settings.kick_onset_min_interval * 1000.0),
        );
        self.drop_gap_value = Readout::value(
            &mut self.fonts,
            &format!("{:.0} dB", settings.drop_gap_threshold),
        );
        self.face_settings = None;
    }

    pub fn set_depth_fade(&mut self, depth_fade: DepthFade) {
        if self.depth_fade == depth_fade {
            return;
        }
        self.depth_fade = depth_fade;
        self.visualizer.set_depth_fade(depth_fade);
        self.depth_fade_value =
            Readout::value(&mut self.fonts, &format!("{:>3}%", depth_fade.percent()));
        self.face_settings = None;
    }

    pub fn set_near_opacity(&mut self, near_opacity: NearOpacity) {
        if self.near_opacity == near_opacity {
            return;
        }
        self.near_opacity = near_opacity;
        self.visualizer.set_near_opacity(near_opacity);
        self.near_opacity_value =
            Readout::value(&mut self.fonts, &format!("{:>3}%", near_opacity.percent()));
        self.face_settings = None;
    }

    pub fn set_crease_threshold(&mut self, crease: CreaseThreshold) {
        if self.crease_threshold == crease {
            return;
        }
        self.crease_threshold = crease;
        self.visualizer.set_crease_threshold(crease);
        self.crease_value =
            Readout::value(&mut self.fonts, &format!("{:>3}", crease.whole_degrees()));
        self.face_settings = None;
    }

    pub fn next_model(&mut self) {
        self.visualizer.next_model();
    }

    pub fn set_line_width(&mut self, width: LineWidth) {
        if self.line_width == width {
            return;
        }
        self.line_width = width;
        self.visualizer.set_line_width(width);
        self.refresh_line_status();
    }

    pub fn set_motion_opacity(&mut self, opacity: MotionOpacity) {
        if self.motion_opacity == opacity {
            return;
        }
        self.motion_opacity = opacity;
        self.visualizer.set_motion_opacity(opacity);
        self.refresh_line_status();
    }

    pub fn set_motion_fade_power(&mut self, power: MotionFadePower) {
        if self.motion_fade_power == power {
            return;
        }
        self.motion_fade_power = power;
        self.visualizer.set_motion_fade_power(power);
        self.refresh_line_status();
    }

    pub fn set_model_motion(&mut self, motion: ModelMotion) {
        if self.model_motion == motion {
            return;
        }
        self.model_motion = motion;
        self.visualizer.set_model_motion(motion);
        self.refresh_line_status();
    }

    fn update_hud(&mut self, dt: f32) {
        let name = self.visualizer.scene_name();
        if name != self.scene_name {
            self.scene_name = name.to_owned();
            self.scene_value = self.fonts.layout_ui(name, 12.0, UiFace::SemiBold);
            self.face_settings = None;
        }
        let line_count = self.visualizer.line_count();
        if line_count != self.line_count {
            self.line_count = line_count;
            self.line_count_value = self.fonts.layout(&format!("LINES {line_count}"), 12.0);
            self.face_settings = None;
        }
        let model_position = self.visualizer.model_position();
        if model_position != self.model_position {
            self.model_position = model_position;
            self.refresh_line_status();
        }
        if self.debug {
            self.debug_clock += dt;
            let revision = self.visualizer.analysis_revision();
            let new_analysis = revision != self.debug_analysis_revision;
            let due = self
                .debug_refresh
                .interval()
                .map_or(new_analysis, |interval| self.debug_clock >= interval);
            if self.debug_snapshot.is_none() || due {
                self.debug_clock = 0.0;
                self.debug_analysis_revision = revision;
                self.debug_snapshot = Some(self.visualizer.debug_snapshot());
                self.face_settings = None;
            }
        }
        if let Some(anchor) = self.visualizer.diagnosis_anchor() {
            self.diagnosis_anchor = Point::new(anchor[0] as f64, anchor[1] as f64);
        }
        let diagnosis = self
            .visualizer
            .diagnosis()
            .map(|readout| (readout.revision, readout.text.clone(), readout.color));
        match diagnosis {
            Some((revision, text, color)) if revision != self.diagnosis_revision => {
                self.diagnosis_revision = revision;
                self.diagnosis_text = Some(self.fonts.layout_ui(&text, 11.0, UiFace::SemiBold));
                self.diagnosis_color = color;
            }
            None if self.diagnosis_text.is_some() => {
                self.diagnosis_text = None;
                self.diagnosis_revision = 0;
            }
            _ => {}
        }
    }

    fn draw_diagnosis(
        scene: &mut Scene,
        text: &Text,
        anchor: Point,
        color: Color,
        colors: ThemeColors,
        ui_scale: f64,
    ) {
        let scale = ui_scale;
        let width = 340.0 * scale;
        let height = 13.0 * scale;
        let left = anchor.x;
        let top = anchor.y - height * 0.5;
        let box_rect = Rect::from_origin_size(
            Point::new(left - 2.0 * scale, top - 2.0 * scale),
            Size::new(width + 12.0 * scale, height + 4.0 * scale),
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            colors.background.with_alpha(0.84),
            None,
            &box_rect,
        );
        scene.stroke(
            &vello::kurbo::Stroke::new(scale),
            Affine::IDENTITY,
            color.with_alpha(0.65),
            None,
            &box_rect,
        );
        scene.stroke(
            &vello::kurbo::Stroke::new(1.4 * scale),
            Affine::IDENTITY,
            color,
            None,
            &vello::kurbo::Line::new(
                Point::new(left, top - 3.0 * scale),
                Point::new(left, top + height + 3.0 * scale),
            ),
        );
        scene.stroke(
            &vello::kurbo::Stroke::new(scale),
            Affine::IDENTITY,
            color.with_alpha(0.7),
            None,
            &vello::kurbo::Line::new(
                Point::new(left - 4.0 * scale, anchor.y),
                Point::new(left + 4.0 * scale, anchor.y),
            ),
        );
        let center = top + height * 0.5;
        let baseline = center + text.cap_height() * scale * 0.5;
        text.draw_left(
            scene,
            Affine::translate((left + 6.0 * scale, baseline)) * Affine::scale(scale),
            color,
        );
    }

    fn draw_face(&mut self, view: &View, limit: FrameRateLimit) {
        let scene = &mut self.face;
        let viewport = view.viewport;
        let ui_scale = view.ui_scale();
        scene.reset();
        let colors = self.theme.colors;
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            colors.background,
            None,
            &Rect::from_origin_size(Point::ORIGIN, viewport),
        );
        if self.presentation.active() {
            return;
        }
        // HUD chrome uses fixed logical sizes, scaled by DPI and the user's
        // text-size setting but never by window size.
        let ui = ui_scale * f64::from(self.text_scale.factor());
        let hud_left = Affine::translate((HUD_MARGIN * ui, viewport.height - HUD_MARGIN * ui))
            * Affine::scale(ui);
        let shutter_width = self.shutter_readout.inline_width(&self.shutter_value) * ui;
        let shutter_transform = Affine::translate((
            viewport.width - HUD_MARGIN * ui - shutter_width,
            viewport.height - HUD_MARGIN * ui,
        )) * Affine::scale(ui);
        let line_count_transform =
            shutter_transform * Affine::translate((-(self.line_count_value.width() + 16.0), 0.0));
        let line_status_transform = line_count_transform
            * Affine::translate((-(self.line_status_value.width() + 24.0), 0.0));
        let top_left = Affine::translate((HUD_MARGIN * ui, 32.0 * ui)) * Affine::scale(ui);
        let palette = self.visualizer.palette();
        let hud_colors = ThemeColors {
            readout: palette.neutral[0],
            hint: palette.neutral[2],
            ..colors
        };
        self.theme_readout
            .draw_left_aligned(scene, hud_left, &self.theme_value, &hud_colors);
        self.scene_readout.draw_inline(
            scene,
            hud_left * Affine::translate((0.0, -24.0)),
            &self.scene_value,
            &hud_colors,
            HintOrder::BeforeValue,
        );
        self.debug_readout.draw_inline(
            scene,
            hud_left * Affine::translate((0.0, -48.0)),
            &self.debug_value,
            &hud_colors,
            HintOrder::BeforeValue,
        );
        self.timeline_readout.draw_inline(
            scene,
            hud_left * Affine::translate((0.0, -72.0)),
            &self.timeline_value,
            &hud_colors,
            HintOrder::BeforeValue,
        );
        self.text_scale_readout.draw_inline(
            scene,
            hud_left * Affine::translate((0.0, -96.0)),
            &self.text_scale_value,
            &hud_colors,
            HintOrder::BeforeValue,
        );
        self.line_status_value
            .draw_left(scene, line_status_transform, hud_colors.readout);
        self.line_count_value
            .draw_left(scene, line_count_transform, hud_colors.readout);
        self.shutter_readout.draw_inline(
            scene,
            shutter_transform,
            &self.shutter_value,
            &hud_colors,
            HintOrder::AfterValue,
        );
        self.preamp_readout
            .draw_left_aligned(scene, top_left, &self.preamp_value, &hud_colors);
        let frame_value = self
            .frame_values
            .get(&limit)
            .expect("every frame limit has a prepared label");
        self.frame_readout.draw_left_aligned(
            scene,
            top_left * Affine::translate((0.0, 44.0)),
            frame_value,
            &hud_colors,
        );
        self.kick_gap_readout.draw_left_aligned(
            scene,
            top_left * Affine::translate((0.0, 88.0)),
            &self.kick_gap_value,
            &hud_colors,
        );
        self.drop_gap_readout.draw_left_aligned(
            scene,
            top_left * Affine::translate((0.0, 132.0)),
            &self.drop_gap_value,
            &hud_colors,
        );
        self.depth_fade_readout.draw_left_aligned(
            scene,
            top_left * Affine::translate((0.0, 176.0)),
            &self.depth_fade_value,
            &hud_colors,
        );
        self.near_opacity_readout.draw_left_aligned(
            scene,
            top_left * Affine::translate((0.0, 220.0)),
            &self.near_opacity_value,
            &hud_colors,
        );
        self.crease_readout.draw_left_aligned(
            scene,
            top_left * Affine::translate((0.0, 264.0)),
            &self.crease_value,
            &hud_colors,
        );
        if self.debug {
            if let Some(snapshot) = &self.debug_snapshot {
                let history = self.visualizer.debug_history();
                let palette = self.visualizer.palette();
                let visualizer = &self.visualizer;
                debug::draw(
                    &mut self.fonts,
                    scene,
                    view,
                    snapshot,
                    history,
                    visualizer,
                    palette,
                    colors,
                    self.debug_refresh,
                    self.text_scale,
                    self.debug_selection,
                    self.debug_pointer,
                    self.tree_target,
                    self.flux_band,
                    &self.temporal_view,
                    &self.graph_max,
                );
            }
        }
    }

    pub fn draw(
        &mut self,
        scene: &mut Scene,
        view: &View,
        limit: FrameRateLimit,
        frame_time: FrameTime,
    ) -> UiFrame<'_> {
        let dt = frame_time.seconds();
        self.theme.advance(dt);
        let presentation_active = self.presentation.active();
        let tree_open =
            self.debug && matches!(self.debug_selection, Some(DebugSelection::Property(_)));
        self.visualizer
            .set_diagnosis_suppressed(tree_open || presentation_active);
        let frame_interval = self.audio.frame(frame_time.start(), frame_time.end());
        let palette = Palette::from_theme(self.theme.selected());
        self.visualizer.update(
            frame_interval,
            self.shutter_frames,
            dt,
            &palette,
            (view.viewport.width as f32, view.viewport.height as f32),
            view.zoom_factor() as f32,
            view.ui_scale() as f32 * self.text_scale.factor(),
        );
        self.presentation.update(
            dt,
            (view.viewport.width as f32, view.viewport.height as f32),
            &self.visualizer,
            &mut self.fonts,
            &self.visualizer.palette(),
            self.theme.colors,
        );
        self.update_hud(dt);
        let settings = FaceSettings {
            viewport: view.viewport,
            ui_scale: view.ui_scale(),
            limit,
            colors: self.theme.colors,
            palette: self.visualizer.palette(),
            debug: self.debug,
            presentation: presentation_active,
            text_scale: self.text_scale,
        };
        let rebuild = self.face_settings != Some(settings);
        if rebuild {
            self.draw_face(view, limit);
            self.face_settings = Some(settings);
        }
        if self.timeline && !presentation_active {
            debug::draw_timeline(
                scene,
                view,
                self.hotkey_line_width(),
                self.visualizer.debug_history(),
                self.detectors.drop_gap_threshold,
                self.visualizer.palette(),
                self.theme.colors,
                self.text_scale,
                &self.timeline_labels,
            );
        }
        if !tree_open && !presentation_active {
            if let Some(text) = &self.diagnosis_text {
                Self::draw_diagnosis(
                    scene,
                    text,
                    self.diagnosis_anchor,
                    self.diagnosis_color,
                    self.theme.colors,
                    view.ui_scale() * f64::from(self.text_scale.factor()),
                );
            }
        }
        let mut raster_assets = Vec::new();
        if presentation_active {
            if let Some(stage) = self.presentation.stage() {
                self.presentation.draw(
                    scene,
                    &mut raster_assets,
                    &stage,
                    self.presentation.colors(),
                    self.plate_mask.as_ref(),
                );
            }
        }
        let overlay_composite =
            presentation_active.then_some(Self::model_composite(self.theme.selected()));
        let overlay_opacity = if presentation_active {
            self.presentation_opacity.factor()
        } else {
            1.0
        };
        let diagnosis_visible = !tree_open && !presentation_active && self.diagnosis_text.is_some();
        let foreground_changed = diagnosis_visible
            || self.diagnosis_visible_last_frame
            || (self.timeline && !presentation_active)
            || presentation_active;
        self.diagnosis_visible_last_frame = diagnosis_visible;
        let background = if rebuild {
            Frame::Rebuilt(&self.face)
        } else if foreground_changed {
            Frame::Retained(&self.face)
        } else {
            Frame::Unchanged(&self.face)
        };
        UiFrame {
            background,
            overlay: self.visualizer.frame(),
            foreground: diagnosis_visible
                || (self.timeline && !presentation_active)
                || presentation_active,
            overlay_composite,
            overlay_opacity,
            plate_instances: self.presentation.plate_instances(),
            plate_ink: self.presentation.colors().ink,
            raster_assets,
        }
    }
}

/// One retained background and its animated foreground overlay.
pub struct UiFrame<'a> {
    pub background: Frame<'a>,
    /// Retained vector sources referenced by the foreground scene.
    pub raster_assets: Vec<RasterAsset>,
    pub overlay: VisualizerFrame<'a>,
    /// Whether the foreground scene carries anything to rasterize.
    pub foreground: bool,
    /// Composition mode for presentation text; `None` keeps normal blending.
    pub overlay_composite: Option<ModelComposite>,
    /// Multiplier applied to the presentation text layer.
    pub overlay_opacity: f32,
    /// Moving plate coverage requests for the GPU mask pass.
    pub plate_instances: &'a [PlateInstance],
    /// Ink color of the moving plate masks.
    pub plate_ink: Color,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::RasterAssetCache;
    use crate::visualizer::{
        Contribution, EnvelopeTarget, Input, MixPoint, StyleInput, TreeTarget,
    };
    use std::time::Duration;

    fn write_preview(
        path: &std::path::Path,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> anyhow::Result<()> {
        use std::{fs::File, io::BufWriter};
        let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(pixels)?;
        Ok(())
    }

    fn presentation_pixels(
        ui: &mut Ui,
        view: &View,
        width: u32,
        height: u32,
        frames: usize,
        feed_audio: bool,
        mut before_frame: impl FnMut(usize, &mut Ui),
    ) -> anyhow::Result<Vec<u8>> {
        use vello::{kurbo::Size, util::RenderContext, wgpu, RenderParams};

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut context = RenderContext::new();
        let id = pollster::block_on(crate::render::prepare_device(
            &mut context,
            None,
            wgpu::DeviceDescriptor::default(),
        ))?;
        let device = &context.devices[id];
        let make_texture = |label: &str, usage: wgpu::TextureUsages| {
            device.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let background = make_texture(
            "Presentation background",
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let foreground = make_texture(
            "Presentation foreground",
            wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let target = make_texture(
            "Presentation target",
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let background_view = background.create_view(&wgpu::TextureViewDescriptor::default());
        let foreground_view = foreground.create_view(&wgpu::TextureViewDescriptor::default());
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut renderer = crate::render::renderer(&device.device)?;
        let mut plate_mask =
            crate::render::PlateMask::new(&device.device, &mut renderer, width, height);
        ui.set_plate_mask(Some(plate_mask.image()));

        let now = Instant::now();
        let mut scene = Scene::new();
        let mut last = None;
        for frame in 0..frames {
            let mut input = Vec::with_capacity(800);
            let start = frame * 800;
            for index in 0..800 {
                let sample = start + index;
                let time = sample as f32 / 48_000.0;
                let kick = if (0..10).contains(&(sample % 9_600)) {
                    0.8
                } else {
                    0.0
                };
                let tone = 0.2 * (std::f32::consts::TAU * 220.0 * time).sin()
                    + 0.1 * (std::f32::consts::TAU * 900.0 * time).sin();
                input.push([tone + kick * (std::f32::consts::TAU * 64.0 * time).sin(); 2]);
            }
            before_frame(frame, ui);
            if feed_audio {
                ui.consume_buf(&input, now + Duration::from_millis((frame as u64) * 16));
            }
            scene.reset();
            let frame_time = FrameTime::new(
                now + Duration::from_millis((frame as u64) * 16),
                now + Duration::from_millis(((frame + 1) as u64) * 16),
            );
            last = Some(ui.draw(&mut scene, view, FrameRateLimit::Fps120, frame_time));
        }
        let frame = last.expect("the presentation produced a frame");
        let mut raster_assets = RasterAssetCache::new();
        raster_assets.prepare(&mut renderer, device, &frame.raster_assets)?;
        let background_scene = match frame.background {
            Frame::Rebuilt(scene) | Frame::Retained(scene) | Frame::Unchanged(scene) => scene,
        };
        let params = RenderParams {
            base_color: vello::peniko::Color::TRANSPARENT,
            width,
            height,
            antialiasing_method: crate::render::ANTIALIASING,
        };
        renderer.render_to_texture(
            &device.device,
            &device.queue,
            background_scene,
            &background_view,
            &params,
        )?;
        plate_mask.render(
            &device.device,
            &device.queue,
            frame.plate_instances,
            frame.plate_ink,
        );
        renderer.mark_override_image_dirty(&plate_mask.image().image);
        renderer.render_to_texture(
            &device.device,
            &device.queue,
            &scene,
            &foreground_view,
            &params,
        )?;
        let mut visualizer_renderer = crate::visualizer::Renderer::new(&device.device, format);
        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        visualizer_renderer.draw(
            device,
            &mut encoder,
            &background_view,
            &target_view,
            Size::new(f64::from(width), f64::from(height)),
            frame.overlay,
            Some(&foreground_view),
            frame.overlay_composite,
            frame.overlay_opacity,
        );
        let row_bytes = width * 4;
        let readback = device.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Presentation readback"),
            size: u64::from(row_bytes * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(height),
                },
            },
            target.size(),
        );
        device.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let pixels = readback.slice(..).get_mapped_range()?.to_vec();
        readback.unmap();
        Ok(pixels)
    }

    fn drawn_ui() -> (Ui, View) {
        let mut ui = Ui::new(&Config::default()).expect("the bundled fonts and models load");
        ui.set_debug(true);
        let view = View::fit(Size::new(1280.0, 440.0));
        let now = Instant::now();
        let mut scene = Scene::new();
        let _ = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        (ui, view)
    }

    #[test]
    fn line_status_reports_model_motion_and_width() {
        assert_eq!(
            Ui::line_status(
                Some((2, 3)),
                ModelMotion::Swept,
                LineWidth::default(),
                MotionOpacity::default(),
                MotionFadePower::default(),
            ),
            "MODEL [\\] 2/3   BLUR [T] ON   WIDTH [;] [']  80%   FLOOR [L] [R]   0%   POWER [Y] 0.5"
        );
    }

    #[test]
    fn light_timeline_redraws_the_foreground_each_frame() {
        let mut ui = Ui::new(&Config::default()).expect("the bundled fonts and models load");
        let view = View::fit(Size::new(1280.0, 440.0));
        let now = Instant::now();
        let frame_time = FrameTime::new(now, now + Duration::from_millis(16));
        let mut scene = Scene::new();

        ui.set_timeline(true);
        {
            let frame = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);
            assert!(matches!(frame.background, Frame::Rebuilt(_)));
        }
        {
            let frame = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);
            assert!(matches!(frame.background, Frame::Retained(_)));
        }

        ui.set_timeline(false);
        {
            let frame = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);
            assert!(matches!(frame.background, Frame::Rebuilt(_)));
        }
        {
            let frame = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);
            assert!(matches!(frame.background, Frame::Unchanged(_)));
        }
    }

    #[test]
    fn presentation_draws_an_animated_foreground_without_the_hud() {
        let mut ui = Ui::new(&Config::default()).expect("the bundled fonts and models load");
        let view = View::fit(Size::new(1280.0, 720.0));
        let now = Instant::now();
        let frame_time = FrameTime::new(now, now + Duration::from_millis(16));
        let mut scene = Scene::new();

        ui.set_presentation(true);
        let expected_composite = Some(Ui::model_composite(ui.theme.selected()));
        let first = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);
        assert!(matches!(first.background, Frame::Rebuilt(_)));
        assert!(first.foreground);
        assert_eq!(first.overlay_composite, expected_composite);
        assert!((first.overlay_opacity - 1.0).abs() < 1e-6);

        let second = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);
        assert!(second.foreground);
        assert!(matches!(
            second.background,
            Frame::Retained(_) | Frame::Unchanged(_)
        ));

        ui.set_presentation(false);
        let third = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);
        assert!(matches!(third.background, Frame::Rebuilt(_)));
        assert!(!third.foreground || ui.timeline());
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_presentation_at_sixteen_by_nine() -> anyhow::Result<()> {
        let width = 1920;
        let height = 1080;
        let mut ui = Ui::new(&Config::default())?;
        ui.set_presentation(true);
        let view = View::fit(Size::new(f64::from(width), f64::from(height)));
        let now = Instant::now();
        let mut scene = Scene::new();
        let _ = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        for _ in 0..64 {
            if ui.visualizer.model_id() == Some("rif_ak47") {
                break;
            }
            ui.visualizer.next_model();
        }
        let pixels = presentation_pixels(&mut ui, &view, width, height, 150, false, |_, _| {})?;
        if let Some(path) = std::env::var_os("VU_PRESENTATION_PREVIEW_PATH") {
            write_preview(&std::path::PathBuf::from(path), width, height, &pixels)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_presentation_entrance() -> anyhow::Result<()> {
        let width = 1920;
        let height = 1080;
        let mut ui = Ui::new(&Config::default())?;
        ui.set_presentation(true);
        let view = View::fit(Size::new(f64::from(width), f64::from(height)));
        let now = Instant::now();
        let mut scene = Scene::new();
        let _ = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        for _ in 0..64 {
            if ui.visualizer.model_id() == Some("rif_ak47") {
                break;
            }
            ui.visualizer.next_model();
        }
        let pixels = presentation_pixels(&mut ui, &view, width, height, 22, false, |_, _| {})?;
        if let Some(path) = std::env::var_os("VU_ENTRANCE_PREVIEW_PATH") {
            write_preview(&std::path::PathBuf::from(path), width, height, &pixels)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_presentation_low_opacity() -> anyhow::Result<()> {
        let width = 1920;
        let height = 1080;
        let mut config = Config::default();
        config.presentation_opacity = PresentationOpacity::try_from(0.5)?;
        let mut ui = Ui::new(&config)?;
        ui.set_presentation(true);
        let view = View::fit(Size::new(f64::from(width), f64::from(height)));
        let now = Instant::now();
        let mut scene = Scene::new();
        let _ = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        for _ in 0..64 {
            if ui.visualizer.model_id() == Some("rif_ak47") {
                break;
            }
            ui.visualizer.next_model();
        }
        let pixels = presentation_pixels(&mut ui, &view, width, height, 80, false, |_, _| {})?;
        if let Some(path) = std::env::var_os("VU_OPACITY_PREVIEW_PATH") {
            write_preview(&std::path::PathBuf::from(path), width, height, &pixels)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_presentation_light_theme() -> anyhow::Result<()> {
        let width = 1920;
        let height = 1080;
        let mut ui = Ui::new(&Config::default())?;
        let theme: ThemeId = ron::from_str("\"Tomorrow\"")?;
        ui.set_theme(theme);
        ui.set_presentation(true);
        let view = View::fit(Size::new(f64::from(width), f64::from(height)));
        let now = Instant::now();
        let mut scene = Scene::new();
        let _ = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        for _ in 0..64 {
            if ui.visualizer.model_id() == Some("rif_ak47") {
                break;
            }
            ui.visualizer.next_model();
        }
        let pixels = presentation_pixels(&mut ui, &view, width, height, 80, false, |_, _| {})?;
        if let Some(path) = std::env::var_os("VU_LIGHT_PREVIEW_PATH") {
            write_preview(&std::path::PathBuf::from(path), width, height, &pixels)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_presentation_aspects() -> anyhow::Result<()> {
        let Some(directory) = std::env::var_os("VU_ASPECT_PREVIEW_DIR") else {
            return Ok(());
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        let mut ui = Ui::new(&Config::default())?;
        ui.set_presentation(true);
        let now = Instant::now();
        let mut scene = Scene::new();
        let load_view = View::fit(Size::new(1920.0, 1080.0));
        let _ = ui.draw(
            &mut scene,
            &load_view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        for _ in 0..64 {
            if ui.visualizer.model_id() == Some("rif_ak47") {
                break;
            }
            ui.visualizer.next_model();
        }
        for (width, height) in [(1280, 720), (1280, 440), (1024, 768)] {
            let view = View::fit(Size::new(f64::from(width), f64::from(height)));
            let pixels = presentation_pixels(&mut ui, &view, width, height, 70, false, |_, _| {})?;
            write_preview(
                &directory.join(format!("presentation-{width}x{height}.png")),
                width,
                height,
                &pixels,
            )?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_selector_presentation() -> anyhow::Result<()> {
        use crate::visualizer::Action;

        let width = 1920;
        let height = 1080;
        let mut ui = Ui::new(&Config::default())?;
        ui.set_presentation(true);
        let view = View::fit(Size::new(f64::from(width), f64::from(height)));
        let now = Instant::now();
        let mut scene = Scene::new();
        let _ = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        for _ in 0..64 {
            if ui.visualizer.model_id() == Some("rif_ak47") {
                break;
            }
            ui.visualizer.next_model();
        }
        let pixels = presentation_pixels(&mut ui, &view, width, height, 90, false, |frame, ui| {
            if frame == 0 {
                ui.visualizer
                    .set_parameter(Parameter::SelectorEntryKicks, 1.0);
                ui.visualizer.handle_action(Action::Kick(0.6), 0.0, 0.75);
            }
            if frame == 30 || frame == 55 {
                ui.visualizer.handle_action(Action::Hat, 0.0, 0.75);
            }
        })?;
        if let Some(path) = std::env::var_os("VU_SELECTOR_PREVIEW_PATH") {
            write_preview(&std::path::PathBuf::from(path), width, height, &pixels)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_selector_exit_presentation() -> anyhow::Result<()> {
        use crate::visualizer::Action;

        let width = 1920;
        let height = 1080;
        let mut ui = Ui::new(&Config::default())?;
        ui.set_presentation(true);
        let view = View::fit(Size::new(f64::from(width), f64::from(height)));
        let now = Instant::now();
        let mut scene = Scene::new();
        let _ = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        for _ in 0..64 {
            if ui.visualizer.model_id() == Some("rif_ak47") {
                break;
            }
            ui.visualizer.next_model();
        }
        let pixels = presentation_pixels(&mut ui, &view, width, height, 38, false, |frame, ui| {
            if frame == 0 {
                ui.visualizer
                    .set_parameter(Parameter::SelectorEntryKicks, 1.0);
                ui.visualizer
                    .set_parameter(Parameter::SelectorExitTriggers, 1.0);
                ui.visualizer.handle_action(Action::Kick(0.6), 0.0, 0.75);
            }
            if frame == 30 {
                ui.visualizer.handle_action(Action::Kick(0.6), 0.0, 0.75);
            }
        })?;
        if let Some(path) = std::env::var_os("VU_SELECTOR_EXIT_PREVIEW_PATH") {
            write_preview(&std::path::PathBuf::from(path), width, height, &pixels)?;
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a compute-capable GPU"]
    fn renders_poi_presentation() -> anyhow::Result<()> {
        use crate::visualizer::{Action, Rng};

        let width = 1920;
        let height = 1080;
        let mut ui = Ui::new(&Config::default())?;
        ui.set_presentation(true);
        let view = View::fit(Size::new(f64::from(width), f64::from(height)));
        let now = Instant::now();
        let mut scene = Scene::new();
        let _ = ui.draw(
            &mut scene,
            &view,
            FrameRateLimit::Fps120,
            FrameTime::new(now, now + Duration::from_millis(16)),
        );
        for _ in 0..64 {
            if ui.visualizer.model_id() == Some("rif_ak47") {
                break;
            }
            ui.visualizer.next_model();
        }
        ui.visualizer
            .set_parameter(Parameter::SnarePoiSwitches, 10.0);
        let mut rng = Rng::new(7);
        ui.visualizer.showcase.enter_focus(&mut rng, 1.1);
        let pixels = presentation_pixels(&mut ui, &view, width, height, 80, false, |frame, ui| {
            if frame == 35 {
                ui.visualizer.handle_action(Action::Snare, 0.0, 1.1);
            }
            if frame == 74 {
                ui.visualizer.handle_action(Action::Hat, 0.0, 1.1);
                ui.presentation.debug_hit_marker();
            }
        })?;
        if let Some(path) = std::env::var_os("VU_POI_PREVIEW_PATH") {
            write_preview(&std::path::PathBuf::from(path), width, height, &pixels)?;
        }
        Ok(())
    }

    #[test]
    fn presentation_overlay_tracks_theme_composition_and_opacity() {
        let mut ui = Ui::new(&Config::default()).expect("the bundled fonts and models load");
        ui.set_presentation(true);
        let view = View::fit(Size::new(1280.0, 720.0));
        let now = Instant::now();
        let mut scene = Scene::new();
        let frame_time = FrameTime::new(now, now + Duration::from_millis(16));
        let dark = Ui::model_composite(ui.theme.selected());
        let _ = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);

        let light: ThemeId = ron::from_str("\"Tomorrow\"").expect("the bundled light theme loads");
        ui.set_theme(light);
        ui.set_presentation_opacity(
            PresentationOpacity::try_from(0.5).expect("the opacity is inside its bounds"),
        );
        let frame = ui.draw(&mut scene, &view, FrameRateLimit::Fps120, frame_time);
        assert_ne!(dark, Ui::model_composite(light));
        assert_eq!(frame.overlay_composite, Some(Ui::model_composite(light)));
        assert!((frame.overlay_opacity - 0.5).abs() < 1e-6);
    }

    #[test]
    fn debug_panel_selects_bands_and_properties() {
        let (mut ui, view) = drawn_ui();
        let layout = debug::Layout::new(&view, ui.text_scale);
        assert!(ui.click_debug(&view, layout.band_center(3)));
        assert_eq!(ui.debug_selection, Some(DebugSelection::Band(3)));
        assert!(ui.click_debug(&view, layout.property_center(Property::Pulse)));
        assert_eq!(
            ui.debug_selection,
            Some(DebugSelection::Property(Property::Pulse))
        );
        assert!(!ui.click_debug(&view, Point::new(1.0, 1.0)));
    }

    #[test]
    fn header_buttons_open_hat_and_scene_panels() {
        let (mut ui, view) = drawn_ui();
        let layout = debug::Layout::new(&view, ui.text_scale);
        assert!(ui.click_debug(&view, layout.hat_button.center()));
        assert_eq!(ui.debug_selection, Some(DebugSelection::Hat));
        let hat = debug::hat_analyzer_layout(&view, ui.text_scale);
        assert!(!hat.sliders.is_empty());
        assert!(hat.temporal.is_some(), "the hat pattern editor is shown");

        assert!(ui.click_debug(&view, layout.scene_button.center()));
        assert_eq!(ui.debug_selection, Some(DebugSelection::Scene));
        let scene = debug::scene_analyzer_layout(&view, ui.text_scale);
        assert_eq!(scene.sliders.len(), 26);
        let slider = scene.sliders[0];
        let before = ui.visualizer.parameter_value(slider.parameter);
        assert!(ui.click_debug(&view, Point::new(slider.track.x0, slider.track.center().y)));
        let after = ui.visualizer.parameter_value(slider.parameter);
        assert!(after < before, "the scene slider edits the live setting");
    }

    #[test]
    fn tree_sliders_edit_the_live_parameter() {
        let (mut ui, view) = drawn_ui();
        ui.debug_selection = Some(DebugSelection::Property(Property::Pulse));
        let target = TreeTarget::Contribution(Contribution::Fixed(MixPoint::PulseEnergy, 0));
        let tree = debug::tree_layout(
            &view,
            ui.text_scale,
            Property::Pulse,
            None,
            ui.visualizer.mixer(),
        );
        let row = tree
            .rows
            .iter()
            .find(|row| row.target() == Some(target))
            .expect("the pulse energy term is listed");
        assert!(ui.click_debug(&view, row.rect.center()));
        assert_eq!(ui.tree_target, Some(target));

        let tree = debug::tree_layout(
            &view,
            ui.text_scale,
            Property::Pulse,
            ui.tree_target,
            ui.visualizer.mixer(),
        );
        assert_eq!(tree.sliders.len(), 7);
        let attack = tree
            .sliders
            .iter()
            .find(|slider| slider.parameter == Parameter::Attack(EnvelopeTarget::Power))
            .expect("the attack slider is exposed");
        let before = ui
            .visualizer
            .parameter_value(Parameter::Attack(EnvelopeTarget::Power));
        assert!(ui.click_debug(&view, attack.track.center()));
        let after = ui
            .visualizer
            .parameter_value(Parameter::Attack(EnvelopeTarget::Power));
        assert_ne!(before, after);
        ui.set_pointer(Some(Point::new(attack.track.x1, attack.track.center().y)));
        let dragged = ui
            .visualizer
            .parameter_value(Parameter::Attack(EnvelopeTarget::Power));
        assert!(dragged > after);
        ui.end_slider_drag();

        ui.tree_target = None;
        let style_target = TreeTarget::Input(Input::Style(StyleInput::ZoomBias));
        let tree = debug::tree_layout(
            &view,
            ui.text_scale,
            Property::Pulse,
            None,
            ui.visualizer.mixer(),
        );
        let row = tree
            .rows
            .iter()
            .find(|row| row.target() == Some(style_target))
            .expect("the zoom bias input is listed");
        assert!(ui.click_debug(&view, row.rect.center()));
        let tree = debug::tree_layout(
            &view,
            ui.text_scale,
            Property::Pulse,
            ui.tree_target,
            ui.visualizer.mixer(),
        );
        let zoom = tree
            .sliders
            .iter()
            .find(|slider| slider.parameter == Parameter::Style(StyleInput::ZoomBias))
            .expect("the style slider is exposed");
        let before = ui
            .visualizer
            .parameter_value(Parameter::Style(StyleInput::ZoomBias));
        assert!(ui.click_debug(
            &view,
            Point::new(zoom.track.x1 - 0.5, zoom.track.center().y)
        ));
        let after = ui
            .visualizer
            .parameter_value(Parameter::Style(StyleInput::ZoomBias));
        assert_ne!(before, after);
    }
}
