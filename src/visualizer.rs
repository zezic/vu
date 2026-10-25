//! Spectrum-driven pseudo-3D wireframe presentation.

mod geometry;
mod renderer;
mod spectrum;

use std::{collections::VecDeque, f32::consts::TAU};

use serde::{Deserialize, Serialize};
use vello::peniko::Color;

use crate::{
    audio::timeline::{AudioInterval, AudioTime},
    config::ShutterFrames,
    shutter::ShutterWindow,
    theme::{mix, Accent, MeterColors, ThemeId},
};

use geometry::{
    soft_saturate, Camera, Deformation, FigureKind, Pose, Projected, Vec2, Vec3, Wireframe,
};
pub use spectrum::BAND_COUNT;
use spectrum::{Spectrum, SpectrumAnalyzer};

pub use renderer::Renderer;

const MAX_LINES: usize = 2048;
const MAX_RIBBONS: usize = 8192;
const MAX_PENDING_HITS: usize = 64;
const MAX_HISTORY: usize = 17;
const HISTORY_SECONDS: f64 = 2.0;
/// Keep the animation phase bounded without breaking the many non-integer
/// sine/cosine frequencies derived from it. All current multipliers are
/// hundredths, so one hundred turns is a whole period for every one of them.
const PHASE_PERIOD: f32 = TAU * 100.0;

/// The top-level display selected by the presentation hotkey.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presentation {
    #[default]
    Gauges,
    Visualizer,
}

impl Presentation {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Gauges => Self::Visualizer,
            Self::Visualizer => Self::Gauges,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Gauges => "Gauges",
            Self::Visualizer => "Wireframe",
        }
    }
}

/// One exposure-swept line, in physical pixels, uploaded as a vertex instance.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    /// Start and end in physical pixels: a.xy, b.xy.
    pub endpoints: [f32; 4],
    /// Tint and opacity; the shader premultiplies before blending.
    pub color: [f32; 4],
    /// Stroke width, triangle fallback flag, static-center flag, padding.
    pub style: [f32; 4],
}

impl LineInstance {
    fn new(start: Vec2, end: Vec2, color: [f32; 4], width: f32) -> Self {
        Self {
            endpoints: [start.x, start.y, end.x, end.y],
            color,
            style: [width, 0.0, 0.0, 0.0],
        }
    }
}

/// The selected Gogh palette split into its four neutral and twelve vivid slots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub neutral: [Color; 4],
    pub vivid: [Color; 12],
}

impl Palette {
    pub fn from_theme(theme: ThemeId) -> Self {
        Self {
            neutral: [
                theme.color(Accent::Foreground),
                theme.color(Accent::White),
                theme.color(Accent::BrightBlack),
                theme.color(Accent::Black),
            ],
            vivid: [
                theme.color(Accent::Red),
                theme.color(Accent::Green),
                theme.color(Accent::Yellow),
                theme.color(Accent::Blue),
                theme.color(Accent::Magenta),
                theme.color(Accent::Cyan),
                theme.color(Accent::BrightRed),
                theme.color(Accent::BrightGreen),
                theme.color(Accent::BrightYellow),
                theme.color(Accent::BrightBlue),
                theme.color(Accent::BrightMagenta),
                theme.color(Accent::BrightCyan),
            ],
        }
    }

    fn approach(&mut self, target: Self, dt: f32, rate: f32) {
        let blend = 1.0 - (-rate * dt).exp();
        for (current, target) in self.neutral.iter_mut().zip(target.neutral) {
            *current = mix(*current, target, blend);
        }
        for (current, target) in self.vivid.iter_mut().zip(target.vivid) {
            *current = mix(*current, target, blend);
        }
    }

    pub fn vivid(&self, index: usize) -> Color {
        self.vivid[index % self.vivid.len()]
    }

    /// Continuous sample of the vivid ring, so palette drift never snaps between
    /// neighbouring hues.
    pub fn vivid_blend(&self, angle: f32) -> Color {
        let scaled = ((angle.sin() * 0.5 + 0.5) * 12.0).rem_euclid(12.0);
        let base = (scaled.floor() as usize).min(11);
        let next = (base + 1) % 12;
        let fraction = scaled.fract();
        mix(self.vivid[base], self.vivid[next], fraction)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandRole {
    Sub,
    Bass,
    Low,
    Kick,
    Spin,
    Pitch,
    Roll,
    Twist,
    Bulge,
    Ripple,
    Shear,
    Zoom,
    Accent,
    Line,
    Jitter,
    Phase,
}

impl BandRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sub => "SUB",
            Self::Bass => "BASS",
            Self::Low => "LOW",
            Self::Kick => "KICK",
            Self::Spin => "SPIN",
            Self::Pitch => "PITCH",
            Self::Roll => "ROLL",
            Self::Twist => "TWIST",
            Self::Bulge => "BULGE",
            Self::Ripple => "RIPPLE",
            Self::Shear => "SHEAR",
            Self::Zoom => "ZOOM",
            Self::Accent => "ACCENT",
            Self::Line => "LINE",
            Self::Jitter => "JITTER",
            Self::Phase => "PHASE",
        }
    }

    fn for_index(index: usize) -> Self {
        match index {
            0 => Self::Sub,
            1 => Self::Bass,
            2 => Self::Low,
            3 => Self::Kick,
            4 => Self::Spin,
            5 => Self::Pitch,
            6 => Self::Roll,
            7 => Self::Twist,
            8 => Self::Bulge,
            9 => Self::Ripple,
            10 => Self::Shear,
            11 => Self::Zoom,
            12 => Self::Accent,
            13 => Self::Line,
            14 => Self::Jitter,
            _ => Self::Phase,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BandDebug {
    pub value: f32,
    pub raw: f32,
    pub low_hz: f32,
    pub high_hz: f32,
    pub role: BandRole,
}

#[derive(Clone, Copy, Debug)]
pub struct EnvelopeDebug {
    pub smooth: f32,
    pub raw: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct OnsetDebug {
    pub level: f32,
    pub flux: f32,
    pub baseline: f32,
    pub threshold: f32,
    pub cooldown: f32,
    pub cooldown_time: f32,
    pub flash: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct StyleDebug {
    pub axis: [f32; 3],
    pub twist_sign: f32,
    pub shear_sign: f32,
    pub deform_bias: f32,
    pub accent_bias: f32,
    pub zoom_bias: f32,
    pub phase_rate: f32,
    pub palette_rate: f32,
    pub jitter_bias: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct HitDebug {
    pub kind: &'static str,
    pub strength: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct DebugSnapshot {
    pub bands: [BandDebug; BAND_COUNT],
    pub power: EnvelopeDebug,
    pub bass: EnvelopeDebug,
    pub treble: EnvelopeDebug,
    pub flux: f32,
    pub bass_flux: f32,
    pub bass_onset: OnsetDebug,
    pub broadband_onset: OnsetDebug,
    pub kick: f32,
    pub transition_cooldown: f32,
    pub hits_last_frame: u32,
    pub bass_hits: u32,
    pub broadband_hits: u32,
    pub last_hit: Option<HitDebug>,
    pub scene: &'static str,
    pub outgoing: Option<&'static str>,
    pub transition: f32,
    pub style: StyleDebug,
    pub phase: f32,
    pub entrance: f32,
    pub lines: usize,
    pub ribbons: usize,
}

/// One gradient section of a three-pose motion blur. Ordinary sweeps use a swept
/// quad between poses; crossing sweeps use per-vertex triangle instances with an
/// independent opacity at each corner.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RibbonInstance {
    /// Older segment pose: a.xy, b.xy.
    pub start: [f32; 4],
    /// Newer segment pose: a.xy, b.xy.
    pub end: [f32; 4],
    /// Tint and opacity at the older pose.
    pub start_color: [f32; 4],
    /// Tint and opacity at the newer pose.
    pub end_color: [f32; 4],
    /// Tint and opacity of the third vertex for triangle fallback instances.
    pub third_color: [f32; 4],
    /// Stroke width, triangle fallback flag, padding, padding.
    pub style: [f32; 4],
}

impl RibbonInstance {
    fn new(
        start: [Vec2; 2],
        end: [Vec2; 2],
        start_color: [f32; 4],
        end_color: [f32; 4],
        width: f32,
    ) -> Self {
        Self {
            start: [start[0].x, start[0].y, start[1].x, start[1].y],
            end: [end[0].x, end[0].y, end[1].x, end[1].y],
            start_color,
            end_color,
            third_color: end_color,
            style: [width, 0.0, 0.0, 0.0],
        }
    }

    /// A crossing-fan fill triangle. The first vertex is the static X, the
    /// second is AM/BM, and the third is the transparent endpoint.
    fn fan_triangle(points: [Vec2; 3], colors: [[f32; 4]; 3], width: f32) -> Self {
        let mut ribbon = Self::triangle(points, colors, width);
        ribbon.style[2] = 1.0;
        ribbon
    }

    /// A single alpha-interpolated triangle, used for non-convex sweeps and the
    /// four crossing-fan regions. The three vertices carry independent colors.
    fn triangle(points: [Vec2; 3], colors: [[f32; 4]; 3], width: f32) -> Self {
        Self {
            start: [points[0].x, points[0].y, points[1].x, points[1].y],
            end: [points[2].x, points[2].y, points[2].x, points[2].y],
            start_color: colors[0],
            end_color: colors[1],
            third_color: colors[2],
            style: [width, 1.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiagnosisReadout {
    pub revision: u64,
    pub text: String,
    pub color: Color,
}

#[derive(Clone, Copy, Debug)]
enum DiagnosisMode {
    Position,
    World,
    Velocity,
    Acceleration,
}

impl DiagnosisMode {
    fn next(self) -> Self {
        match self {
            Self::Position => Self::World,
            Self::World => Self::Velocity,
            Self::Velocity => Self::Acceleration,
            Self::Acceleration => Self::Position,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Position => 0,
            Self::World => 1,
            Self::Velocity => 2,
            Self::Acceleration => 3,
        }
    }

    fn random(rng: &mut Rng) -> Self {
        match rng.range(4) {
            0 => Self::Position,
            1 => Self::World,
            2 => Self::Velocity,
            _ => Self::Acceleration,
        }
    }

    fn format(self, data: VertexDiagnosis) -> String {
        match self {
            Self::Position => format!("POS X{:7.1} Y{:7.1}", data.screen.x, data.screen.y),
            Self::World => format!(
                "XYZ {:+.2} {:+.2} {:+.2}",
                data.world.x, data.world.y, data.world.z
            ),
            Self::Velocity => format!(
                "VEL {:6.1}px/s  DX{:+.1} DY{:+.1}",
                data.velocity.length(),
                data.velocity.x,
                data.velocity.y
            ),
            Self::Acceleration => format!("ACC {:6.1}px/s2", data.acceleration.length()),
        }
    }
}

struct Diagnosis {
    active: bool,
    vertex: usize,
    mode: DiagnosisMode,
    next_in: f32,
    remaining: f32,
    mode_timer: f32,
    refresh_timer: f32,
    anchor: Vec2,
    readout: Option<DiagnosisReadout>,
    revision: u64,
    scene: &'static str,
}

impl Diagnosis {
    fn new() -> Self {
        Self {
            active: false,
            vertex: 0,
            mode: DiagnosisMode::Position,
            next_in: 2.5,
            remaining: 0.0,
            mode_timer: 0.0,
            refresh_timer: 0.0,
            anchor: Vec2::new(0.0, 0.0),
            readout: None,
            revision: 0,
            scene: "",
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.readout = None;
        self.next_in = 2.0;
        self.revision += 1;
    }
}

/// The animated layers for one displayed visualizer frame.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub lines: &'a [LineInstance],
    pub ribbons: &'a [RibbonInstance],
}

pub struct Visualizer {
    analyzer: SpectrumAnalyzer,
    bands: [Envelope; BAND_COUNT],
    power: Envelope,
    bass: Envelope,
    treble: Envelope,
    bass_onset: Onset,
    broadband_onset: Onset,
    pending_hits: VecDeque<Hit>,
    rng: Rng,
    scene: Scene,
    frames: ShutterWindow,
    kick: f32,
    time: f32,
    transition_cooldown: f32,
    palette: Palette,
    last_spectrum: Spectrum,
    bass_hit_flash: f32,
    broadband_hit_flash: f32,
    last_hit: Option<Hit>,
    analysis_revision: u64,
    hits_last_frame: u32,
    bass_hits: u32,
    broadband_hits: u32,
    diagnosis: Diagnosis,
    lines: Vec<LineInstance>,
    ribbons: Vec<RibbonInstance>,
}

impl Visualizer {
    pub fn new() -> Self {
        let bands =
            std::array::from_fn(|index| Envelope::new(14.0, 5.0 + 5.0 * index as f32 / 15.0));
        Self {
            analyzer: SpectrumAnalyzer::new(48_000),
            bands,
            power: Envelope::new(18.0, 4.5),
            bass: Envelope::new(90.0, 7.0),
            treble: Envelope::new(16.0, 3.5),
            bass_onset: Onset::bass(),
            broadband_onset: Onset::broadband(),
            pending_hits: VecDeque::with_capacity(MAX_PENDING_HITS),
            rng: Rng::new(0x005e_ed0f_7e5e_1e55u64),
            scene: Scene::new(),
            frames: ShutterWindow::default(),
            kick: 0.0,
            time: 0.0,
            transition_cooldown: 0.0,
            palette: Palette::from_theme(ThemeId::default()),
            last_spectrum: Spectrum::default(),
            bass_hit_flash: 0.0,
            broadband_hit_flash: 0.0,
            last_hit: None,
            analysis_revision: 0,
            hits_last_frame: 0,
            bass_hits: 0,
            broadband_hits: 0,
            diagnosis: Diagnosis::new(),
            lines: Vec::with_capacity(MAX_LINES),
            ribbons: Vec::with_capacity(MAX_RIBBONS),
        }
    }

    pub fn set_samplerate(&mut self, samplerate: usize) {
        self.analyzer.set_samplerate(samplerate);
        self.reset_audio();
    }

    /// Discard temporal history after capture restarts or the mode becomes visible.
    pub fn reset_audio(&mut self) {
        self.analyzer.reset();
        self.reset_analysis();
        self.pending_hits.clear();
        self.scene.reset_temporal();
        self.frames.clear();
        self.kick = 0.0;
        self.transition_cooldown = 0.0;
        self.last_spectrum = Spectrum::default();
        self.bass_hit_flash = 0.0;
        self.broadband_hit_flash = 0.0;
        self.last_hit = None;
        self.analysis_revision = 0;
        self.hits_last_frame = 0;
        self.bass_hits = 0;
        self.broadband_hits = 0;
        self.diagnosis.reset();
    }

    fn reset_analysis(&mut self) {
        for band in &mut self.bands {
            band.reset();
        }
        self.power.reset();
        self.bass.reset();
        self.treble.reset();
        self.bass_onset.reset();
        self.broadband_onset.reset();
    }

    /// Raw stereo frames feed the mono spectrum analyzer at the sample rate.
    pub fn consume_buf(&mut self, buf: &[[f32; 2]], gain: f32) {
        let hop = self.analyzer.hop_seconds();
        for frame in buf {
            let mono = (frame[0] + frame[1]) * 0.5 * gain;
            if let Some(spectrum) = self.analyzer.push(mono) {
                self.observe(spectrum, hop);
            }
        }
    }

    fn observe(&mut self, spectrum: Spectrum, hop: f32) {
        self.analysis_revision = self.analysis_revision.wrapping_add(1);
        self.last_spectrum = spectrum;
        self.bass_hit_flash *= (-hop * 7.0).exp();
        self.broadband_hit_flash *= (-hop * 7.0).exp();
        for (band, target) in self.bands.iter_mut().zip(spectrum.bands) {
            band.advance(target, hop);
        }
        self.power.advance(spectrum.power, hop);
        self.bass.advance(spectrum.bass, hop);
        self.treble.advance(spectrum.treble, hop);
        let bass_hit = self
            .bass_onset
            .observe(spectrum.bass, spectrum.bass_flux, hop);
        let broadband_hit = self
            .broadband_onset
            .observe(spectrum.power, spectrum.flux, hop);
        if let Some(strength) = bass_hit {
            self.bass_hits = self.bass_hits.saturating_add(1);
            self.bass_hit_flash = 1.0;
            self.last_hit = Some(Hit {
                kind: HitKind::Bass,
                strength,
            });
            self.push_hit(HitKind::Bass, strength);
        } else if let Some(strength) = broadband_hit {
            self.broadband_hits = self.broadband_hits.saturating_add(1);
            self.broadband_hit_flash = 1.0;
            self.last_hit = Some(Hit {
                kind: HitKind::Broadband,
                strength,
            });
            self.push_hit(HitKind::Broadband, strength);
        }
    }

    fn push_hit(&mut self, kind: HitKind, strength: f32) {
        if self.pending_hits.len() == MAX_PENDING_HITS {
            self.pending_hits.pop_front();
        }
        self.pending_hits.push_back(Hit { kind, strength });
    }

    pub fn frame(&self) -> Frame<'_> {
        Frame {
            lines: &self.lines,
            ribbons: &self.ribbons,
        }
    }

    pub fn scene_name(&self) -> &'static str {
        self.scene.current.kind.name()
    }

    pub fn analysis_revision(&self) -> u64 {
        self.analysis_revision
    }

    pub fn palette(&self) -> Palette {
        self.palette
    }

    pub fn diagnosis(&self) -> Option<&DiagnosisReadout> {
        self.diagnosis.readout.as_ref()
    }

    pub fn diagnosis_anchor(&self) -> Option<[f32; 2]> {
        self.diagnosis
            .active
            .then_some([self.diagnosis.anchor.x, self.diagnosis.anchor.y])
    }

    pub fn debug_snapshot(&self) -> DebugSnapshot {
        let bands = std::array::from_fn(|index| {
            let (low_hz, high_hz) = self.analyzer.band_range(index);
            BandDebug {
                value: self.bands[index].value,
                raw: self.last_spectrum.bands[index],
                low_hz,
                high_hz,
                role: BandRole::for_index(index),
            }
        });
        let bass_onset = OnsetDebug {
            level: self.last_spectrum.bass,
            flux: self.last_spectrum.bass_flux,
            baseline: self.bass_onset.baseline,
            threshold: self.bass_onset.threshold,
            cooldown: self.bass_onset.cooldown,
            cooldown_time: self.bass_onset.cooldown_time,
            flash: self.bass_hit_flash,
        };
        let broadband_onset = OnsetDebug {
            level: self.last_spectrum.power,
            flux: self.last_spectrum.flux,
            baseline: self.broadband_onset.baseline,
            threshold: self.broadband_onset.threshold,
            cooldown: self.broadband_onset.cooldown,
            cooldown_time: self.broadband_onset.cooldown_time,
            flash: self.broadband_hit_flash,
        };
        let style = self.scene.current.style;
        DebugSnapshot {
            bands,
            power: EnvelopeDebug {
                smooth: self.power.value,
                raw: self.last_spectrum.power,
            },
            bass: EnvelopeDebug {
                smooth: self.bass.value,
                raw: self.last_spectrum.bass,
            },
            treble: EnvelopeDebug {
                smooth: self.treble.value,
                raw: self.last_spectrum.treble,
            },
            flux: self.last_spectrum.flux,
            bass_flux: self.last_spectrum.bass_flux,
            bass_onset,
            broadband_onset,
            kick: self.kick,
            transition_cooldown: self.transition_cooldown,
            hits_last_frame: self.hits_last_frame,
            bass_hits: self.bass_hits,
            broadband_hits: self.broadband_hits,
            last_hit: self.last_hit.map(|hit| HitDebug {
                kind: hit.kind.name(),
                strength: hit.strength,
            }),
            scene: self.scene.current.kind.name(),
            outgoing: self.scene.outgoing.as_ref().map(|f| f.kind.name()),
            transition: self.scene.transition,
            style: StyleDebug {
                axis: [style.axis.x, style.axis.y, style.axis.z],
                twist_sign: style.twist_sign,
                shear_sign: style.shear_sign,
                deform_bias: style.deform_bias,
                accent_bias: style.accent_bias,
                zoom_bias: style.zoom_bias,
                phase_rate: style.phase_rate,
                palette_rate: style.palette_rate,
                jitter_bias: style.jitter_bias,
            },
            phase: self.scene.current.phase.rem_euclid(TAU),
            entrance: self.scene.current.entrance,
            lines: self.lines.len(),
            ribbons: self.ribbons.len(),
        }
    }

    /// Advance the scene from the latest analysis and rebuild the line overlay.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        frame: AudioInterval,
        shutter: ShutterFrames,
        dt: f32,
        colors: &MeterColors,
        palette: &Palette,
        viewport: (f32, f32),
        zoom: f32,
        ui_scale: f32,
    ) {
        let dt = dt.clamp(0.0, 0.1);
        self.palette.approach(*palette, dt, 10.0);
        self.time += dt;
        self.transition_cooldown = (self.transition_cooldown - dt).max(0.0);
        self.hits_last_frame = self.pending_hits.len() as u32;
        let mut strongest = None;
        while let Some(hit) = self.pending_hits.pop_front() {
            if strongest.is_none_or(|best: Hit| hit.strength > best.strength) {
                strongest = Some(hit);
            }
        }
        if let Some(hit) = strongest {
            self.handle_hit(hit);
        }
        self.kick *= (-dt * 3.2).exp();
        self.bass_hit_flash *= (-dt * 5.0).exp();
        self.broadband_hit_flash *= (-dt * 5.0).exp();
        let exposure = self.frames.interval(frame, shutter);
        let camera = Camera::new(viewport.0, viewport.1, zoom);
        let music = Music {
            bands: self.bands.map(|band| band.value),
            power: self.power.value,
            bass: self.bass.value.max(self.bass_onset.previous),
            treble: self.treble.value,
            kick: self.kick,
        };
        let width = (viewport.0.min(viewport.1) / 440.0).clamp(0.62, 2.6) * 1.4;
        self.lines.clear();
        self.ribbons.clear();
        self.scene.update(
            dt,
            &music,
            exposure.start(),
            frame.end(),
            camera,
            colors,
            &self.palette,
            width,
            self.time,
            &mut self.lines,
            &mut self.ribbons,
        );
        debug_assert!(
            self.lines.len() <= MAX_LINES,
            "the wireframe renderer line budget is exhausted"
        );
        debug_assert!(
            self.ribbons.len() <= MAX_RIBBONS,
            "the wireframe renderer ribbon budget is exhausted: {} ribbons",
            self.ribbons.len()
        );
        self.lines.truncate(MAX_LINES);
        self.ribbons.truncate(MAX_RIBBONS);
        let ui_scale = ui_scale.max(0.25);
        self.update_diagnosis(dt, viewport, self.palette, ui_scale);
        self.append_diagnosis(viewport, ui_scale);
    }

    fn diagnosis_target(&self, viewport: (f32, f32), ui_scale: f32) -> Vec2 {
        let vertex = self
            .scene
            .current
            .vertex_diagnosis(self.diagnosis.vertex)
            .map(|data| data.screen)
            .unwrap_or(Vec2::new(viewport.0 * 0.5, viewport.1 * 0.5));
        let center = Vec2::new(viewport.0 * 0.5, viewport.1 * 0.5);
        let side = if vertex.x < center.x { 1.0 } else { -1.0 };
        let target = Vec2::new(
            vertex.x + side * 150.0 * ui_scale,
            vertex.y - 70.0 * ui_scale,
        );
        Vec2::new(
            target
                .x
                .clamp(36.0 * ui_scale, (viewport.0 * 0.36).max(36.0 * ui_scale)),
            target.y.clamp(
                36.0 * ui_scale,
                (viewport.1 - 36.0 * ui_scale).max(36.0 * ui_scale),
            ),
        )
    }

    fn activate_diagnosis(&mut self, viewport: (f32, f32), ui_scale: f32) {
        let count = self.scene.current.vertices.len();
        if count == 0 {
            return;
        }
        self.diagnosis.active = true;
        self.diagnosis.vertex = self.rng.range(count);
        self.diagnosis.mode = DiagnosisMode::random(&mut self.rng);
        self.diagnosis.remaining = 2.5 + self.rng.unit() * 3.0;
        self.diagnosis.mode_timer = 0.0;
        self.diagnosis.refresh_timer = 0.1;
        self.diagnosis.scene = self.scene.current.kind.name();
        let target = self.diagnosis_target(viewport, ui_scale);
        self.diagnosis.anchor = target;
        self.diagnosis.revision += 1;
    }

    fn update_diagnosis(&mut self, dt: f32, viewport: (f32, f32), palette: Palette, ui_scale: f32) {
        if !self.diagnosis.active {
            self.diagnosis.next_in -= dt;
            if self.diagnosis.next_in <= 0.0 {
                self.activate_diagnosis(viewport, ui_scale);
            }
            return;
        }
        self.diagnosis.remaining -= dt;
        if self.diagnosis.remaining <= 0.0 {
            self.diagnosis.active = false;
            self.diagnosis.readout = None;
            self.diagnosis.revision += 1;
            self.diagnosis.next_in = 3.0 + self.rng.unit() * 5.0;
            return;
        }
        let scene = self.scene.current.kind.name();
        if scene != self.diagnosis.scene {
            self.diagnosis.scene = scene;
            self.diagnosis.vertex = self.rng.range(self.scene.current.vertices.len().max(1));
            self.diagnosis.mode = DiagnosisMode::random(&mut self.rng);
            self.diagnosis.mode_timer = 0.0;
            self.diagnosis.refresh_timer = 0.1;
            let target = self.diagnosis_target(viewport, ui_scale);
            self.diagnosis.anchor = target;
            self.diagnosis.readout = None;
            self.diagnosis.revision += 1;
        }
        if self.diagnosis.vertex >= self.scene.current.vertices.len() {
            self.diagnosis.vertex = self.scene.current.vertices.len().saturating_sub(1);
        }
        self.diagnosis.mode_timer += dt;
        if self.diagnosis.mode_timer >= 1.8 {
            self.diagnosis.mode_timer = 0.0;
            self.diagnosis.mode = self.diagnosis.mode.next();
            self.diagnosis.refresh_timer = 0.1;
            self.diagnosis.revision += 1;
        }
        let target = self.diagnosis_target(viewport, ui_scale);
        self.diagnosis.anchor =
            self.diagnosis.anchor + (target - self.diagnosis.anchor) * (1.0 - (-dt * 0.35).exp());
        self.diagnosis.refresh_timer += dt;
        if self.diagnosis.refresh_timer >= 0.1 {
            self.diagnosis.refresh_timer = 0.0;
            if let Some(data) = self.scene.current.vertex_diagnosis(self.diagnosis.vertex) {
                let color = palette.vivid_blend(
                    self.diagnosis.vertex as f32 * 1.7 + self.diagnosis.mode.index() as f32 * 2.1,
                );
                self.diagnosis.revision += 1;
                self.diagnosis.readout = Some(DiagnosisReadout {
                    revision: self.diagnosis.revision,
                    text: self.diagnosis.mode.format(data),
                    color,
                });
            }
        }
    }

    fn append_diagnosis(&mut self, viewport: (f32, f32), ui_scale: f32) {
        if !self.diagnosis.active {
            return;
        }
        let Some(data) = self.scene.current.vertex_diagnosis(self.diagnosis.vertex) else {
            return;
        };
        let Some(readout) = &self.diagnosis.readout else {
            return;
        };
        let color = readout.color.components;
        let vertex = data.screen;
        let anchor = self.diagnosis.anchor;
        let size = (viewport.0.min(viewport.1) / 80.0).clamp(7.0, 16.0) * ui_scale;
        let half = size * 0.5;
        let arm = size * 0.34;
        let width = 1.15 * ui_scale;
        let alpha = 0.78;
        let mut push = |a: Vec2, b: Vec2| {
            self.lines.push(LineInstance::new(
                a,
                b,
                [color[0], color[1], color[2], alpha],
                width,
            ));
        };
        // Four sci-fi corner brackets.
        push(
            Vec2::new(vertex.x - half, vertex.y - half),
            Vec2::new(vertex.x - half + arm, vertex.y - half),
        );
        push(
            Vec2::new(vertex.x - half, vertex.y - half),
            Vec2::new(vertex.x - half, vertex.y - half + arm),
        );
        push(
            Vec2::new(vertex.x + half, vertex.y - half),
            Vec2::new(vertex.x + half - arm, vertex.y - half),
        );
        push(
            Vec2::new(vertex.x + half, vertex.y - half),
            Vec2::new(vertex.x + half, vertex.y - half + arm),
        );
        push(
            Vec2::new(vertex.x - half, vertex.y + half),
            Vec2::new(vertex.x - half + arm, vertex.y + half),
        );
        push(
            Vec2::new(vertex.x - half, vertex.y + half),
            Vec2::new(vertex.x - half, vertex.y + half - arm),
        );
        push(
            Vec2::new(vertex.x + half, vertex.y + half),
            Vec2::new(vertex.x + half - arm, vertex.y + half),
        );
        push(
            Vec2::new(vertex.x + half, vertex.y + half),
            Vec2::new(vertex.x + half, vertex.y + half - arm),
        );
        // Small crosshair ticks.
        push(
            Vec2::new(vertex.x, vertex.y - half * 0.55),
            Vec2::new(vertex.x, vertex.y - half * 1.55),
        );
        push(
            Vec2::new(vertex.x, vertex.y + half * 0.55),
            Vec2::new(vertex.x, vertex.y + half * 1.55),
        );
        push(
            Vec2::new(vertex.x - half * 0.55, vertex.y),
            Vec2::new(vertex.x - half * 1.55, vertex.y),
        );
        push(
            Vec2::new(vertex.x + half * 0.55, vertex.y),
            Vec2::new(vertex.x + half * 1.55, vertex.y),
        );
        // Stepped polyline to the lazily following readout.
        let side = if anchor.x < vertex.x { -1.0 } else { 1.0 };
        let p1 = Vec2::new(vertex.x + side * 18.0 * ui_scale, vertex.y);
        let p2 = Vec2::new(anchor.x + side * 24.0 * ui_scale, vertex.y);
        let p3 = Vec2::new(anchor.x + side * 24.0 * ui_scale, anchor.y);
        push(vertex, p1);
        push(p1, p2);
        push(p2, p3);
        push(p3, anchor);
        push(
            Vec2::new(anchor.x - 4.0 * ui_scale, anchor.y),
            Vec2::new(anchor.x + 4.0 * ui_scale, anchor.y),
        );
    }

    fn handle_hit(&mut self, hit: Hit) {
        self.kick = self.kick.max(hit.strength);
        if self.transition_cooldown > 0.0 {
            return;
        }
        match roll_transition(hit.kind, &mut self.rng) {
            Some(Transition::Scene) => {
                self.scene.change(&mut self.rng);
                self.transition_cooldown = 0.3;
            }
            Some(Transition::Parameter) => {
                self.scene.mutate(&mut self.rng);
                self.transition_cooldown = 0.18;
            }
            None => {}
        }
    }

    #[cfg(test)]
    fn debug_music(&mut self, bands: [f32; BAND_COUNT], power: f32) {
        for (envelope, value) in self.bands.iter_mut().zip(bands) {
            envelope.value = value;
        }
        self.power.value = power;
        self.bass.value = bands[..4].iter().sum::<f32>() / 4.0;
    }
}

impl Default for Visualizer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct Music {
    bands: [f32; BAND_COUNT],
    power: f32,
    bass: f32,
    treble: f32,
    kick: f32,
}

impl Music {
    fn band(self, index: usize) -> f32 {
        self.bands[index.min(BAND_COUNT - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HitKind {
    Bass,
    Broadband,
}

impl HitKind {
    fn name(self) -> &'static str {
        match self {
            Self::Bass => "BASS",
            Self::Broadband => "WIDE",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Hit {
    kind: HitKind,
    strength: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transition {
    Scene,
    Parameter,
}

fn roll_transition(kind: HitKind, rng: &mut Rng) -> Option<Transition> {
    let (scene_chance, parameter_chance) = match kind {
        HitKind::Bass => (0.55, 0.85),
        HitKind::Broadband => (0.18, 0.45),
    };
    if rng.unit() < scene_chance {
        Some(Transition::Scene)
    } else if rng.unit() < parameter_chance {
        Some(Transition::Parameter)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
struct Envelope {
    value: f32,
    attack: f32,
    release: f32,
}

impl Envelope {
    fn new(attack: f32, release: f32) -> Self {
        assert!(attack > 0.0 && release > 0.0);
        Self {
            value: 0.0,
            attack,
            release,
        }
    }

    fn advance(&mut self, target: f32, dt: f32) {
        let rate = if target > self.value {
            self.attack
        } else {
            self.release
        };
        let blend = 1.0 - (-rate * dt).exp();
        self.value += (target - self.value) * blend;
    }

    fn reset(&mut self) {
        self.value = 0.0;
    }
}

#[derive(Clone, Copy)]
struct Smoother {
    value: f32,
}

impl Smoother {
    fn new(value: f32) -> Self {
        Self { value }
    }

    fn approach(&mut self, target: f32, dt: f32, rate: f32) -> f32 {
        let blend = 1.0 - (-rate * dt).exp();
        self.value += (target - self.value) * blend;
        self.value
    }
}

struct Onset {
    previous: f32,
    baseline: f32,
    cooldown: f32,
    threshold: f32,
    min_flux: f32,
    baseline_ratio: f32,
    cooldown_time: f32,
}

impl Onset {
    fn bass() -> Self {
        Self {
            previous: 0.0,
            baseline: 0.0,
            cooldown: 0.0,
            threshold: 0.17,
            min_flux: 0.035,
            baseline_ratio: 1.08,
            cooldown_time: 0.11,
        }
    }

    fn broadband() -> Self {
        Self {
            previous: 0.0,
            baseline: 0.0,
            cooldown: 0.0,
            threshold: 0.22,
            min_flux: 0.03,
            baseline_ratio: 1.1,
            cooldown_time: 0.15,
        }
    }

    fn observe(&mut self, value: f32, flux: f32, dt: f32) -> Option<f32> {
        self.cooldown = (self.cooldown - dt).max(0.0);
        let baseline = self.baseline;
        let blend = 1.0 - (-dt / 0.7).exp();
        self.baseline += (value - self.baseline) * blend;
        self.previous = value;
        if self.cooldown == 0.0
            && value >= self.threshold
            && flux >= self.min_flux
            && value >= baseline * self.baseline_ratio + 0.015
        {
            self.cooldown = self.cooldown_time;
            Some((value + flux * 2.0).clamp(0.25, 1.5))
        } else {
            None
        }
    }

    fn reset(&mut self) {
        self.previous = 0.0;
        self.baseline = 0.0;
        self.cooldown = 0.0;
    }
}

struct Scene {
    current: FigureRuntime,
    outgoing: Option<FigureRuntime>,
    transition: f32,
}

impl Scene {
    fn new() -> Self {
        Self {
            current: FigureRuntime::new(FigureKind::Cube, 1.0),
            outgoing: None,
            transition: 1.0,
        }
    }

    fn change(&mut self, rng: &mut Rng) {
        let next = self.current.kind.next();
        let mut entering = FigureRuntime::new(next, 0.0);
        entering.pose.yaw += rng.signed() * 0.9;
        entering.pose.pitch += rng.signed() * 0.4;
        entering.phase = rng.unit() * TAU;
        entering.palette_phase_a = rng.unit() * TAU;
        entering.palette_phase_b = entering.palette_phase_a + 1.5 + rng.unit() * 2.0;
        entering.target_style = Style::random(rng);
        self.outgoing = None;
        self.current.opacity = 1.0;
        let old = std::mem::replace(&mut self.current, entering);
        self.outgoing = Some(old);
        self.transition = 0.0;
    }

    fn mutate(&mut self, rng: &mut Rng) {
        self.current.target_style = Style::random(rng);
        self.current.flash = 1.0;
    }

    fn reset_temporal(&mut self) {
        self.current.reset_temporal();
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.reset_temporal();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        dt: f32,
        music: &Music,
        exposure_start: AudioTime,
        frame_end: AudioTime,
        camera: Camera,
        colors: &MeterColors,
        palette: &Palette,
        width: f32,
        time: f32,
        out: &mut Vec<LineInstance>,
        out_ribbons: &mut Vec<RibbonInstance>,
    ) {
        if self.outgoing.is_some() {
            let rate = 1.2 + 2.8 * music.power + 1.7 * music.kick;
            self.transition = (self.transition + dt * rate).min(1.0);
            let progress = smoothstep(self.transition);
            if let Some(outgoing) = &mut self.outgoing {
                outgoing.opacity = 1.0 - progress;
            }
            self.current.opacity = progress;
        } else {
            self.current.opacity = 1.0;
        }
        self.current.update(
            dt,
            music,
            exposure_start,
            frame_end,
            camera,
            colors,
            palette,
            width,
            time,
            out,
            out_ribbons,
        );
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.update(
                dt,
                music,
                exposure_start,
                frame_end,
                camera,
                colors,
                palette,
                width,
                time,
                out,
                out_ribbons,
            );
        }
        if self.transition >= 1.0 {
            self.outgoing = None;
        }
    }
}

struct FigureRuntime {
    kind: FigureKind,
    wireframe: Wireframe,
    pose: Pose,
    pitch: Smoother,
    roll: Smoother,
    style: Style,
    target_style: Style,
    phase: f32,
    palette_phase_a: f32,
    palette_phase_b: f32,
    entrance: f32,
    flash: f32,
    opacity: f32,
    history: VecDeque<PoseSnapshot>,
    vertices: Vec<Projected>,
    world_vertices: Vec<Vec3>,
    previous_screen: Vec<Vec2>,
    velocity: Vec<Vec2>,
    acceleration: Vec<Vec2>,
    current: Vec<LineGeometry>,
}

#[derive(Clone)]
struct PoseSnapshot {
    at: AudioTime,
    lines: Vec<LineGeometry>,
}

#[derive(Clone, Copy)]
struct LineGeometry {
    start: Vec2,
    end: Vec2,
    depth: f32,
}

#[derive(Clone, Copy)]
struct VertexDiagnosis {
    screen: Vec2,
    world: Vec3,
    velocity: Vec2,
    acceleration: Vec2,
}

impl FigureRuntime {
    fn new(kind: FigureKind, opacity: f32) -> Self {
        let pose = Pose::default();
        Self {
            kind,
            wireframe: Wireframe::build(kind),
            pose,
            pitch: Smoother::new(pose.pitch),
            roll: Smoother::new(pose.roll),
            style: Style::default(),
            target_style: Style::default(),
            phase: 0.0,
            palette_phase_a: 0.0,
            palette_phase_b: 2.2,
            entrance: opacity,
            flash: 0.0,
            opacity,
            history: VecDeque::with_capacity(MAX_HISTORY),
            vertices: Vec::with_capacity(64),
            world_vertices: Vec::with_capacity(64),
            previous_screen: Vec::with_capacity(64),
            velocity: Vec::with_capacity(64),
            acceleration: Vec::with_capacity(64),
            current: Vec::with_capacity(64),
        }
    }

    fn reset_temporal(&mut self) {
        self.history.clear();
    }

    // Sixteen log-spaced areas drive the scene: 0-3 bass pulse and hits, 4 spin,
    // 5 pitch, 6 roll, 7 twist, 8 bulge, 9 ripple, 10 shear, 11 camera zoom,
    // 12 accent colour, 13 line energy, 14 jitter, 15 phase speed.
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        dt: f32,
        music: &Music,
        exposure_start: AudioTime,
        frame_end: AudioTime,
        camera: Camera,
        colors: &MeterColors,
        palette: &Palette,
        base_width: f32,
        time: f32,
        out: &mut Vec<LineInstance>,
        out_ribbons: &mut Vec<RibbonInstance>,
    ) {
        self.flash *= (-dt * 3.0).exp();
        self.entrance = (self.entrance + dt * (1.4 + 2.5 * music.power)).min(1.0);
        self.style
            .approach(self.target_style, dt, 1.1 + 3.0 * music.power);
        let spin = (0.16 + 1.9 * music.power + 0.7 * music.band(4)) * self.style.phase_rate;
        self.pose.yaw =
            (self.pose.yaw + dt * spin * (0.3 + self.style.axis.x.abs())).rem_euclid(TAU);
        let pitch_target = -0.24
            + music.band(5) * 0.55 * self.style.axis.y
            + (time * 0.47 + self.phase).sin() * 0.08;
        self.pose.pitch = self
            .pitch
            .approach(pitch_target, dt, 1.6 + 1.8 * music.power);
        let roll_target = music.band(6) * 0.9 * self.style.axis.z;
        self.pose.roll = self.roll.approach(roll_target, dt, 1.3 + 1.6 * music.power);
        self.phase = (self.phase
            + dt * (0.35 + 2.6 * music.power + 0.7 * music.band(15)) * self.style.phase_rate)
            .rem_euclid(PHASE_PERIOD);
        let palette_step =
            dt * (0.12 + 0.8 * music.power + 0.45 * music.band(12)) * self.style.palette_rate;
        self.palette_phase_a = (self.palette_phase_a + palette_step).rem_euclid(TAU);
        self.palette_phase_b = (self.palette_phase_b + palette_step * 0.73).rem_euclid(TAU);
        let entrance_scale = 0.62 + 0.38 * smoothstep(self.entrance);
        let camera_zoom = 1.0 + music.band(11) * 0.18;
        let scale = entrance_scale
            * (0.9 + 0.18 * music.power + 0.11 * music.bass + 0.12 * music.kick)
            * self.style.zoom_bias
            * camera_zoom;
        self.pose.scale = soft_saturate(scale, 1.45).max(0.35);
        let deformation = Deformation {
            twist: music.band(7) * 1.7 * self.style.deform_bias * self.style.twist_sign,
            bulge: music.band(8) * 0.32 * self.style.deform_bias,
            ripple: music.band(9) * 0.75 * self.style.deform_bias,
            shear: music.band(10) * 0.55 * self.style.shear_sign,
            jitter: music.band(14) * 0.055 * self.style.jitter_bias,
            phase: self.phase,
        };
        self.world_vertices.clear();
        self.vertices.clear();
        for index in 0..self.wireframe.vertex_count() {
            let world = deformation.apply(self.wireframe.vertex(index, self.phase));
            self.world_vertices.push(world);
            self.vertices.push(camera.project(world, self.pose));
        }
        self.update_kinematics(dt);
        self.current.clear();
        self.current
            .extend(self.wireframe.edges().iter().map(|edge| {
                let a = self.vertices[edge[0] as usize];
                let b = self.vertices[edge[1] as usize];
                LineGeometry {
                    start: a.screen,
                    end: b.screen,
                    depth: (a.depth + b.depth) * 0.5,
                }
            }));
        if self.history.len() == MAX_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(PoseSnapshot {
            at: frame_end,
            lines: self.current.clone(),
        });
        while self
            .history
            .front()
            .is_some_and(|snapshot| frame_end.seconds_since(snapshot.at) > HISTORY_SECONDS)
        {
            self.history.pop_front();
        }
        let exposure = self.exposure_poses(exposure_start, frame_end);
        let line_energy = music.band(13);
        let accent =
            (music.band(12) * self.style.accent_bias + music.flash() * 0.25).clamp(0.0, 1.0);
        let hot = (music.kick * 0.45 + self.flash * 0.25).clamp(0.0, 1.0);
        let highlight = if color_luma(colors.background) > 0.45 {
            palette.neutral[3]
        } else {
            palette.neutral[1]
        };
        for (index, current) in self.current.iter().enumerate() {
            let reference = exposure.as_ref().map_or(*current, |exposure| {
                lerp_line(exposure.first[index], exposure.last[index], 0.5)
            });
            let depth_light = 0.32 + 0.68 * (1.0 - reference.depth);
            let deep = mix(colors.background, palette.neutral[3], 0.22);
            let far = mix(deep, palette.neutral[2], 0.5);
            let core = mix(palette.neutral[0], highlight, 0.25 + 0.4 * depth_light);
            let base = mix(far, core, depth_light);
            let use_second = self.kind.is_complex() && index % 2 == 1;
            let accent_color = if use_second {
                palette.vivid_blend(self.palette_phase_b)
            } else {
                palette.vivid_blend(self.palette_phase_a)
            };
            let accent_mix = (0.3
                + accent * self.style.accent_bias * 0.3
                + music.band(12) * 0.3
                + if index % 4 == 0 { 0.22 } else { 0.0 })
            .clamp(0.0, 0.9);
            let tinted = mix(base, accent_color, accent_mix);
            // Hits brighten the existing accent instead of introducing a third hue.
            let hot_color = mix(accent_color, highlight, 0.5);
            let color = mix(tinted, hot_color, hot * 0.65);
            let alpha = self.opacity * (0.4 + 0.6 * depth_light) * (0.5 + 0.5 * line_energy);
            let width = base_width * (0.8 + 0.5 * depth_light) * (1.0 + music.kick * 0.25);
            let rgb = [
                color.components[0],
                color.components[1],
                color.components[2],
            ];
            let alpha = alpha.clamp(0.0, 1.0);

            let Some(exposure) = &exposure else {
                out.push(LineInstance::new(
                    current.start,
                    current.end,
                    [rgb[0], rgb[1], rgb[2], alpha],
                    width,
                ));
                continue;
            };
            let first = exposure.first[index];
            let last = exposure.last[index];
            let middle = lerp_line(first, last, 0.5);
            let distance_a = (last.start - first.start).length();
            let distance_b = (last.end - first.end).length();
            let movement = distance_a.max(distance_b);
            let fade = |distance: f32| alpha * (0.35 + 0.65 * width / (width + distance));

            if let Some(crossing) =
                segment_intersection(first.start, first.end, last.start, last.end)
            {
                let end_color = [rgb[0], rgb[1], rgb[2], 0.0];
                let x_color = [rgb[0], rgb[1], rgb[2], alpha];
                let am_color = [rgb[0], rgb[1], rgb[2], fade(distance_a)];
                let bm_color = [rgb[0], rgb[1], rgb[2], fade(distance_b)];
                // The fan shader zeros the endpoint-to-X edges. Constant-width
                // AM-X and BM-X strokes meet at X so the crossing cannot collapse
                // to zero thickness.
                out_ribbons.push(RibbonInstance::fan_triangle(
                    [crossing, middle.start, first.start],
                    [x_color, am_color, end_color],
                    width,
                ));
                out_ribbons.push(RibbonInstance::fan_triangle(
                    [crossing, middle.start, last.start],
                    [x_color, am_color, end_color],
                    width,
                ));
                out_ribbons.push(RibbonInstance::fan_triangle(
                    [crossing, middle.end, first.end],
                    [x_color, bm_color, end_color],
                    width,
                ));
                out_ribbons.push(RibbonInstance::fan_triangle(
                    [crossing, middle.end, last.end],
                    [x_color, bm_color, end_color],
                    width,
                ));
                push_gradient_stroke(
                    out_ribbons,
                    middle.start,
                    crossing,
                    am_color,
                    x_color,
                    width,
                );
                push_gradient_stroke(out_ribbons, middle.end, crossing, bm_color, x_color, width);
                continue;
            }

            // A fast edge spreads the same ink over more pixels, so its peak
            // opacity is lower. Even a motionless edge keeps the central line.
            let spread = movement / (width + movement);
            let peak = alpha * (0.35 + 0.65 * (1.0 - spread));
            out.push(LineInstance::new(
                middle.start,
                middle.end,
                [rgb[0], rgb[1], rgb[2], peak],
                width,
            ));
            if movement <= 0.0 {
                continue;
            }

            let motion = ((last.start - first.start) + (last.end - first.end)) * 0.5;
            let edge = middle.end - middle.start;
            let direction = if motion.length() > 1e-4 {
                motion.normalized()
            } else {
                let normal = edge.perpendicular().normalized();
                if normal.length() > 0.5 {
                    normal
                } else {
                    Vec2::new(1.0, 0.0)
                }
            };
            // The line body sits between the two gradients. Gradients stop at
            // its edges so their polygons meet the body without overlapping it.
            let offset = direction * (width * 0.5);
            let back_start = middle.start - offset;
            let back_end = middle.end - offset;
            let front_start = middle.start + offset;
            let front_end = middle.end + offset;
            let transparent = [rgb[0], rgb[1], rgb[2], 0.0];
            let opaque = [rgb[0], rgb[1], rgb[2], peak];
            if convex_quad(first.start, first.end, back_end, back_start) {
                out_ribbons.push(RibbonInstance::new(
                    [first.start, first.end],
                    [back_start, back_end],
                    transparent,
                    opaque,
                    width,
                ));
            } else {
                out_ribbons.push(RibbonInstance::triangle(
                    [first.start, first.end, back_end],
                    [transparent, transparent, opaque],
                    width,
                ));
                out_ribbons.push(RibbonInstance::triangle(
                    [first.start, back_end, back_start],
                    [transparent, opaque, opaque],
                    width,
                ));
            }
            if convex_quad(front_start, front_end, last.end, last.start) {
                out_ribbons.push(RibbonInstance::new(
                    [front_start, front_end],
                    [last.start, last.end],
                    opaque,
                    transparent,
                    width,
                ));
            } else {
                out_ribbons.push(RibbonInstance::triangle(
                    [front_start, front_end, last.end],
                    [opaque, opaque, transparent],
                    width,
                ));
                out_ribbons.push(RibbonInstance::triangle(
                    [front_start, last.end, last.start],
                    [opaque, transparent, transparent],
                    width,
                ));
            }
        }
    }

    fn update_kinematics(&mut self, dt: f32) {
        let count = self.vertices.len();
        self.previous_screen.resize(
            count,
            self.vertices
                .first()
                .map_or(Vec2::new(0.0, 0.0), |v| v.screen),
        );
        self.velocity.resize(count, Vec2::new(0.0, 0.0));
        self.acceleration.resize(count, Vec2::new(0.0, 0.0));
        let blend = 1.0 - (-dt * 10.0).exp();
        for index in 0..count {
            let current = self.vertices[index].screen;
            let previous = self.previous_screen[index];
            let old_velocity = self.velocity[index];
            let old_acceleration = self.acceleration[index];
            let (raw_velocity, raw_acceleration) = if dt > 1e-4 {
                let velocity = (current - previous) * (1.0 / dt);
                let acceleration = (velocity - old_velocity) * (1.0 / dt);
                (velocity, acceleration)
            } else {
                (old_velocity, Vec2::new(0.0, 0.0))
            };
            self.velocity[index] = old_velocity + (raw_velocity - old_velocity) * blend;
            self.acceleration[index] =
                old_acceleration + (raw_acceleration - old_acceleration) * blend;
            self.previous_screen[index] = current;
        }
    }

    fn vertex_diagnosis(&self, index: usize) -> Option<VertexDiagnosis> {
        Some(VertexDiagnosis {
            screen: self.vertices.get(index)?.screen,
            world: *self.world_vertices.get(index)?,
            velocity: *self.velocity.get(index)?,
            acceleration: *self.acceleration.get(index)?,
        })
    }

    /// The two recorded poses spanning one shutter exposure. The middle pose is
    /// reconstructed as the median of the endpoints, never sampled from history,
    /// so the gradient is centred on a straight interpolation of the exposure.
    fn exposure_poses(&self, start: AudioTime, end: AudioTime) -> Option<ExposurePoses> {
        if end <= start || self.history.len() < 2 {
            return None;
        }
        let duration = end.seconds_since(start);
        if duration <= 1e-6 {
            return None;
        }
        Some(ExposurePoses {
            first: self.pose_at(start)?,
            last: self.pose_at(end)?,
        })
    }

    fn pose_at(&self, time: AudioTime) -> Option<Vec<LineGeometry>> {
        let first = self.history.front()?;
        let last = self.history.back()?;
        if time <= first.at {
            return Some(first.lines.clone());
        }
        if time >= last.at {
            return Some(last.lines.clone());
        }
        let mut before = first;
        for snapshot in self.history.iter().skip(1) {
            if snapshot.at >= time {
                let span = snapshot.at.seconds_since(before.at);
                let fraction = if span > 1e-9 {
                    (time.seconds_since(before.at) / span).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                };
                return Some(
                    before
                        .lines
                        .iter()
                        .zip(&snapshot.lines)
                        .map(|(&from, &to)| lerp_line(from, to, fraction))
                        .collect(),
                );
            }
            before = snapshot;
        }
        Some(last.lines.clone())
    }
}

struct ExposurePoses {
    first: Vec<LineGeometry>,
    last: Vec<LineGeometry>,
}

/// A constant-width quad stroke with independent opacity at each end.
fn push_gradient_stroke(
    out: &mut Vec<RibbonInstance>,
    start: Vec2,
    end: Vec2,
    start_color: [f32; 4],
    end_color: [f32; 4],
    width: f32,
) {
    let direction = (end - start).normalized();
    if direction.length() <= 0.0 {
        return;
    }
    let normal = direction.perpendicular() * (width * 0.5);
    let a = start + normal;
    let b = start - normal;
    let c = end + normal;
    let d = end - normal;
    out.push(RibbonInstance::triangle(
        [a, b, c],
        [start_color, start_color, end_color],
        width,
    ));
    out.push(RibbonInstance::triangle(
        [b, d, c],
        [start_color, end_color, end_color],
        width,
    ));
}

fn segment_intersection(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> Option<Vec2> {
    let r = a1 - a0;
    let s = b1 - b0;
    let denominator = r.x * s.y - r.y * s.x;
    if denominator.abs() < 1e-6 {
        return None;
    }
    let offset = b0 - a0;
    let t = (offset.x * s.y - offset.y * s.x) / denominator;
    let u = (offset.x * r.y - offset.y * r.x) / denominator;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(a0 + r * t)
    } else {
        None
    }
}

fn convex_quad(a0: Vec2, b0: Vec2, b1: Vec2, a1: Vec2) -> bool {
    let points = [a0, b0, b1, a1];
    let mut sign = 0.0;
    for index in 0..4 {
        let a = points[index];
        let b = points[(index + 1) % 4];
        let c = points[(index + 2) % 4];
        let cross = (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x);
        if cross.abs() > 1e-6 {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
    }
    true
}

fn lerp_line(from: LineGeometry, to: LineGeometry, fraction: f32) -> LineGeometry {
    LineGeometry {
        start: from.start.lerp(to.start, fraction),
        end: from.end.lerp(to.end, fraction),
        depth: from.depth + (to.depth - from.depth) * fraction,
    }
}
#[derive(Clone, Copy)]
struct Style {
    axis: Vec3,
    twist_sign: f32,
    shear_sign: f32,
    deform_bias: f32,
    accent_bias: f32,
    zoom_bias: f32,
    phase_rate: f32,
    palette_rate: f32,
    jitter_bias: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            axis: Vec3::new(0.55, 0.25, 0.4).normalized(),
            twist_sign: 1.0,
            shear_sign: 1.0,
            deform_bias: 0.55,
            accent_bias: 0.35,
            zoom_bias: 1.0,
            phase_rate: 1.0,
            palette_rate: 1.0,
            jitter_bias: 0.5,
        }
    }
}

impl Style {
    fn random(rng: &mut Rng) -> Self {
        let axes = [
            Vec3::new(1.0, 0.2, 0.1),
            Vec3::new(0.15, 1.0, 0.2),
            Vec3::new(0.1, 0.25, 1.0),
            Vec3::new(0.7, 0.6, 0.35),
            Vec3::new(-0.6, 0.8, 0.3),
        ];
        Self {
            axis: axes[rng.range(axes.len())].normalized(),
            twist_sign: if rng.chance(0.5) { 1.0 } else { -1.0 },
            shear_sign: if rng.chance(0.5) { 1.0 } else { -1.0 },
            deform_bias: 0.35 + rng.unit() * 0.95,
            accent_bias: 0.15 + rng.unit() * 0.85,
            zoom_bias: 0.9 + rng.unit() * 0.25,
            phase_rate: 0.7 + rng.unit() * 0.7,
            palette_rate: 0.55 + rng.unit() * 1.1,
            jitter_bias: 0.2 + rng.unit() * 1.1,
        }
    }

    fn approach(&mut self, target: Self, dt: f32, rate: f32) {
        let blend = 1.0 - (-rate * dt).exp();
        // Do not renormalize here: opposite target axes must pass through zero
        // smoothly instead of snapping through a singularity.
        self.axis = self.axis + (target.axis - self.axis) * blend;
        self.twist_sign += (target.twist_sign - self.twist_sign) * blend;
        self.shear_sign += (target.shear_sign - self.shear_sign) * blend;
        self.deform_bias += (target.deform_bias - self.deform_bias) * blend;
        self.accent_bias += (target.accent_bias - self.accent_bias) * blend;
        self.zoom_bias += (target.zoom_bias - self.zoom_bias) * blend;
        self.phase_rate += (target.phase_rate - self.phase_rate) * blend;
        self.palette_rate += (target.palette_rate - self.palette_rate) * blend;
        self.jitter_bias += (target.jitter_bias - self.jitter_bias) * blend;
    }
}

impl Music {
    fn flash(self) -> f32 {
        (self.kick * 0.4 + self.treble * 0.15).clamp(0.0, 1.0)
    }
}

fn color_luma(color: Color) -> f32 {
    let [red, green, blue, _] = color.components;
    0.299 * red + 0.587 * green + 0.114 * blue
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn unit(&mut self) -> f32 {
        ((self.next() >> 40) as f32) / ((1u64 << 24) as f32)
    }

    fn chance(&mut self, probability: f32) -> bool {
        self.unit() < probability
    }

    fn range(&mut self, end: usize) -> usize {
        assert!(end > 0);
        ((self.unit() * end as f32) as usize).min(end - 1)
    }

    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bass_hits_are_more_likely_to_change_the_scene() {
        let mut bass = 0;
        let mut broadband = 0;
        let mut bass_scenes = 0;
        let mut broadband_scenes = 0;
        let mut rng = Rng::new(7);
        for _ in 0..20_000 {
            match roll_transition(HitKind::Bass, &mut rng) {
                Some(Transition::Scene) => {
                    bass += 1;
                    bass_scenes += 1;
                }
                Some(Transition::Parameter) => bass += 1,
                None => {}
            }
            match roll_transition(HitKind::Broadband, &mut rng) {
                Some(Transition::Scene) => {
                    broadband += 1;
                    broadband_scenes += 1;
                }
                Some(Transition::Parameter) => broadband += 1,
                None => {}
            }
        }
        assert!(
            bass > broadband + 4_000,
            "bass {bass} vs broadband {broadband}"
        );
        assert!(
            bass_scenes > broadband_scenes + 2_000,
            "bass scenes {bass_scenes} vs broadband scenes {broadband_scenes}"
        );
    }

    #[test]
    fn the_first_scene_is_a_cube_and_evolution_advances_it() {
        let visualizer = Visualizer::new();
        assert_eq!(visualizer.scene.current.kind, FigureKind::Cube);
        assert_eq!(
            visualizer.scene.current.wireframe.edges().len(),
            12,
            "the first figure is a simple wireframe cube"
        );
        assert_eq!(FigureKind::Cube.next(), FigureKind::Octahedron);
    }

    #[test]
    fn a_visible_scene_generates_swept_lines_and_ribbons() {
        let mut visualizer = Visualizer::new();
        visualizer.debug_music([0.5; BAND_COUNT], 0.6);
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        for _ in 0..6 {
            visualizer.update(
                frame,
                ShutterFrames::try_from(3.0).unwrap(),
                1.0 / 60.0,
                &colors,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        let frame = visualizer.frame();
        assert!(frame.lines.len() + frame.ribbons.len() >= 12);
        assert!(!frame.ribbons.is_empty());
        let ramps = frame
            .ribbons
            .iter()
            .filter(|ribbon| {
                let alphas = [
                    ribbon.start_color[3],
                    ribbon.end_color[3],
                    ribbon.third_color[3],
                ];
                alphas.iter().any(|alpha| *alpha > 0.0)
                    && (alphas[0] != alphas[1] || alphas[1] != alphas[2])
            })
            .count();
        assert!(ramps > 0);
        assert!(frame.lines.iter().all(|line| line.color[3] > 0.0));
        assert!(frame
            .lines
            .iter()
            .all(|line| { line.endpoints.iter().all(|value| value.is_finite()) }));
        assert!(frame.ribbons.iter().any(|ribbon| {
            let movement = (ribbon.start[0] - ribbon.end[0]).hypot(ribbon.start[1] - ribbon.end[1])
                + (ribbon.start[2] - ribbon.end[2]).hypot(ribbon.start[3] - ribbon.end[3]);
            movement > 0.0
        }));
    }

    #[test]
    fn a_step_in_spectrum_energy_moves_lines_without_jumping_out_of_view() {
        let mut visualizer = Visualizer::new();
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut previous: Vec<[f32; 2]> = Vec::new();
        for step in 0..120 {
            let level = (step as f32 / 60.0).min(1.0);
            visualizer.debug_music([level; BAND_COUNT], level);
            visualizer.update(
                frame,
                ShutterFrames::try_from(1.0).unwrap(),
                1.0 / 60.0,
                &colors,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            let current: Vec<[f32; 2]> = visualizer
                .scene
                .current
                .current
                .iter()
                .map(|line| [line.end.x, line.end.y])
                .collect();
            if !previous.is_empty() {
                let moved = current
                    .iter()
                    .zip(&previous)
                    .map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]))
                    .fold(0.0_f32, f32::max);
                assert!(
                    moved < 180.0,
                    "the figure jumped {moved} pixels in one frame"
                );
            }
            previous = current;
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        for line in visualizer.frame().lines {
            for value in &line.endpoints {
                assert!(value.is_finite());
            }
            assert!(line.endpoints[0] >= -1280.0 && line.endpoints[0] <= 2560.0);
            assert!(line.endpoints[1] >= -440.0 && line.endpoints[1] <= 880.0);
        }
    }

    #[test]
    fn strong_bass_bursts_reach_the_scene_and_trigger_evolution() {
        let mut visualizer = Visualizer::new();
        visualizer.set_samplerate(48_000);
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut at = AudioTime::ZERO;
        let mut last_name = visualizer.scene_name();
        let mut scene_changes = 0;
        let mut kick_seen = false;
        for frame in 0..360 {
            let input: Vec<[f32; 2]> = (0..800)
                .map(|index| {
                    let sample = frame * 800 + index;
                    let time = sample as f32 / 48_000.0;
                    let block = sample % 9_600;
                    let bass = if block < 3_000 {
                        (1.0 - block as f32 / 3_000.0) * (std::f32::consts::TAU * 55.0 * time).sin()
                    } else {
                        0.0
                    };
                    let tone = 0.2 * (std::f32::consts::TAU * 1_200.0 * time).sin();
                    [bass + tone; 2]
                })
                .collect();
            visualizer.consume_buf(&input, 1.0);
            let end = at.shifted(800.0 / 48_000.0);
            visualizer.update(
                AudioInterval::new(at, end),
                ShutterFrames::try_from(2.0).unwrap(),
                800.0 / 48_000.0,
                &colors,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            at = end;
            kick_seen |= visualizer.kick > 0.0;
            if visualizer.scene_name() != last_name {
                scene_changes += 1;
                last_name = visualizer.scene_name();
            }
        }
        assert!(kick_seen, "bass energy must reach the hit envelope");
        assert!(scene_changes > 0, "bass hits must evolve the scene");
    }

    #[test]
    fn longer_shutter_keeps_more_of_the_recorded_motion_path() {
        fn trail(shutter: f32) -> (usize, f32) {
            let mut visualizer = Visualizer::new();
            let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
            let palette = Palette::from_theme(crate::theme::ThemeId::default());
            let mut frame =
                AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
            for _ in 0..14 {
                visualizer.debug_music([0.8; BAND_COUNT], 0.8);
                visualizer.update(
                    frame,
                    ShutterFrames::try_from(shutter).unwrap(),
                    1.0 / 60.0,
                    &colors,
                    &palette,
                    (1280.0, 440.0),
                    1.0,
                    1.0,
                );
                frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
            }
            let frame = visualizer.frame();
            let mut min_x = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            for ribbon in frame.ribbons {
                for corner in [ribbon.start, ribbon.end] {
                    min_x = min_x.min(corner[0]).min(corner[2]);
                    max_x = max_x.max(corner[0]).max(corner[2]);
                }
            }
            (frame.ribbons.len(), max_x - min_x)
        }
        let (short_count, short_spread) = trail(1.0);
        let (long_count, long_spread) = trail(8.0);
        assert!(short_count > 0);
        assert!(long_count > 0);
        assert!(long_spread > short_spread);
    }

    #[test]
    fn palette_phase_wraps_continuously() {
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let before = palette.vivid_blend(TAU - 0.0001);
        let after = palette.vivid_blend(0.0);
        for channel in 0..3 {
            assert!((before.components[channel] - after.components[channel]).abs() < 1e-3);
        }
    }

    #[test]
    fn crossing_segments_produce_intersection_point() {
        let crossing = segment_intersection(
            Vec2::new(-50.0, 0.0),
            Vec2::new(50.0, 0.0),
            Vec2::new(0.0, -50.0),
            Vec2::new(0.0, 50.0),
        )
        .expect("the segments cross");
        assert!(crossing.x.abs() < 1e-4);
        assert!(crossing.y.abs() < 1e-4);

        assert!(segment_intersection(
            Vec2::new(-50.0, 0.0),
            Vec2::new(50.0, 0.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(0.0, 50.0),
        )
        .is_none());
    }

    #[test]
    fn non_convex_sweep_selects_triangle_fallback() {
        let convex = convex_quad(
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
        );
        let bowtie = convex_quad(
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(0.0, 100.0),
            Vec2::new(100.0, 100.0),
        );
        assert!(convex);
        assert!(!bowtie);

        let ribbon = RibbonInstance::triangle(
            [
                Vec2::new(0.0, 0.0),
                Vec2::new(100.0, 0.0),
                Vec2::new(0.0, 100.0),
            ],
            [
                [1.0, 1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0, 0.5],
                [1.0, 1.0, 1.0, 1.0],
            ],
            2.0,
        );
        assert_eq!(ribbon.style[1], 1.0);
        assert_eq!(ribbon.start_color[3], 0.0);
        assert_eq!(ribbon.end_color[3], 0.5);
        assert_eq!(ribbon.third_color[3], 1.0);
    }

    #[test]
    fn phase_rebase_preserves_tesseract_and_deformation_geometry() {
        let wireframe = Wireframe::build(FigureKind::Tesseract);
        let mut before = Deformation {
            twist: 0.3,
            bulge: 0.2,
            ripple: 0.4,
            shear: 0.1,
            jitter: 0.02,
            phase: 0.0,
        };
        let mut after = before;
        after.phase = PHASE_PERIOD;
        for index in 0..wireframe.vertex_count() {
            let a = before.apply(wireframe.vertex(index, 0.0));
            let b = after.apply(wireframe.vertex(index, PHASE_PERIOD));
            assert!(
                (a - b).length() < 0.01,
                "phase rebase jumped vertex {index}: {a:?} vs {b:?}"
            );
        }
        before.phase = TAU - 0.001;
        after.phase = TAU + 0.001;
        let a = before.apply(wireframe.vertex(0, TAU - 0.001));
        let b = after.apply(wireframe.vertex(0, TAU + 0.001));
        assert!((a - b).length() < 0.01);
    }

    #[test]
    fn tesseract_vertices_move_continuously() {
        let mut visualizer = Visualizer::new();
        visualizer.scene.current.kind = FigureKind::Tesseract;
        visualizer.scene.current.wireframe = Wireframe::build(FigureKind::Tesseract);
        visualizer.scene.current.reset_temporal();
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut previous = None;
        let mut maximum = 0.0_f32;
        for _ in 0..240 {
            visualizer.debug_music([0.4; BAND_COUNT], 0.4);
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &colors,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            let current = visualizer.scene.current.vertices[0].screen;
            if let Some(previous) = previous {
                maximum = maximum.max((current - previous).length());
            }
            previous = Some(current);
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        assert!(
            maximum < 120.0,
            "the tesseract vertex jumped {maximum} pixels in one frame"
        );
    }

    #[test]
    fn diagnosis_appears_on_a_vertex_and_follows_it_lazily() {
        let mut visualizer = Visualizer::new();
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        visualizer.diagnosis.next_in = 0.0;
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut first_anchor = None;
        let mut latest_anchor = None;
        for _ in 0..120 {
            visualizer.debug_music([0.7; BAND_COUNT], 0.7);
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &colors,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            if let Some(readout) = visualizer.diagnosis() {
                assert!(
                    readout.text.starts_with("POS")
                        || readout.text.starts_with("XYZ")
                        || readout.text.starts_with("VEL")
                        || readout.text.starts_with("ACC")
                );
                let anchor = visualizer
                    .diagnosis_anchor()
                    .expect("a visible diagnosis has an anchor");
                assert!(anchor[0] >= 0.0 && anchor[0] <= 1280.0);
                assert!(anchor[1] >= 0.0 && anchor[1] <= 440.0);
                first_anchor.get_or_insert(anchor);
                latest_anchor = Some(anchor);
            }
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        let first = first_anchor.expect("diagnosis must become visible");
        let latest = latest_anchor.expect("diagnosis must stay visible");
        assert!((first[0] - latest[0]).abs() + (first[1] - latest[1]).abs() > 0.05);
        let frame = visualizer.frame();
        assert!(frame.lines.len() > visualizer.scene.current.wireframe.edges().len());
    }

    #[test]
    fn debug_snapshot_exposes_all_analysis_layers() {
        let mut visualizer = Visualizer::new();
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        for _ in 0..3 {
            visualizer.debug_music([0.6; BAND_COUNT], 0.6);
            visualizer.update(
                frame,
                ShutterFrames::try_from(2.0).unwrap(),
                1.0 / 60.0,
                &colors,
                &palette,
                (1280.0, 440.0),
                1.0,
                1.0,
            );
            frame = AudioInterval::new(frame.end(), frame.end().shifted(1.0 / 60.0));
        }
        let snapshot = visualizer.debug_snapshot();
        assert_eq!(snapshot.bands.len(), BAND_COUNT);
        assert!(snapshot.bands.iter().all(|band| band.high_hz > band.low_hz));
        assert!(snapshot.power.smooth.is_finite());
        assert!(snapshot.bass.smooth.is_finite());
        assert!(snapshot.treble.smooth.is_finite());
        assert!(snapshot.bass_onset.threshold > 0.0);
        assert!(snapshot.broadband_onset.cooldown_time > 0.0);
        assert!(!snapshot.scene.is_empty());
        assert!(snapshot.lines > 0);
        assert!(snapshot.ribbons > 0);
    }

    #[test]
    fn style_axis_crosses_opposite_targets_without_snapping() {
        let mut style = Style::default();
        let target = Style {
            axis: style.axis * -1.0,
            ..style
        };
        let mut previous = style.axis;
        for _ in 0..120 {
            style.approach(target, 1.0 / 60.0, 4.0);
            assert!((style.axis - previous).length() < 0.2);
            previous = style.axis;
        }
    }

    #[test]
    fn envelope_smoothing_is_monotonic_and_bounded() {
        let mut envelope = Envelope::new(80.0, 5.0);
        let mut previous = 0.0;
        for _ in 0..60 {
            envelope.advance(1.0, 1.0 / 120.0);
            assert!(envelope.value >= previous);
            assert!(envelope.value <= 1.0);
            previous = envelope.value;
        }
        for _ in 0..120 {
            envelope.advance(0.0, 1.0 / 120.0);
            assert!(envelope.value <= previous);
            assert!(envelope.value >= 0.0);
            previous = envelope.value;
        }
    }
}
