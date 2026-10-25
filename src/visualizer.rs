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
    frame_schedule::Adjustment,
    shutter::ShutterWindow,
    theme::{mix, Accent, MeterColors, ThemeId},
};

use geometry::{
    soft_saturate, Camera, Deformation, DepthOpacity, FigureKind, Pose, Projected, Vec2, Vec3,
    Wireframe,
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

/// One onset detector threshold, normalized to 0..1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct OnsetThreshold(f32);

impl OnsetThreshold {
    const STEP: f32 = 0.01;

    pub fn value(self) -> f32 {
        self.0
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let delta = match adjustment {
            Adjustment::Decrease => -Self::STEP,
            Adjustment::Increase => Self::STEP,
        };
        Self((self.0 + delta).clamp(0.0, 1.0))
    }
}

impl TryFrom<f32> for OnsetThreshold {
    type Error = anyhow::Error;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        anyhow::ensure!(
            value.is_finite() && (0.0..=1.0).contains(&value),
            "onset thresholds must be finite and between 0 and 1"
        );
        Ok(Self(value))
    }
}

impl From<OnsetThreshold> for f32 {
    fn from(value: OnsetThreshold) -> Self {
        value.0
    }
}

const BASS_THRESHOLD_DEFAULT: f32 = 0.17;
const BROADBAND_THRESHOLD_DEFAULT: f32 = 0.22;

/// The bass and broadband onset thresholds used for visualizer hit detection.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OnsetThresholds {
    pub bass: OnsetThreshold,
    pub broadband: OnsetThreshold,
}

impl Default for OnsetThresholds {
    fn default() -> Self {
        Self {
            bass: OnsetThreshold(BASS_THRESHOLD_DEFAULT),
            broadband: OnsetThreshold(BROADBAND_THRESHOLD_DEFAULT),
        }
    }
}

/// One exposure-swept line, in physical pixels, uploaded as a vertex instance.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    /// Start and end in physical pixels: a.xy, b.xy.
    pub endpoints: [f32; 4],
    /// Tint and opacity at the start endpoint; the shader premultiplies before
    /// blending.
    pub start_color: [f32; 4],
    /// Tint and opacity at the end endpoint.
    pub end_color: [f32; 4],
    /// Stroke width, triangle fallback flag, static-center flag, padding.
    pub style: [f32; 4],
}

impl LineInstance {
    fn new(start: Vec2, end: Vec2, color: [f32; 4], width: f32) -> Self {
        Self::shaded(start, end, color, color, width)
    }

    fn shaded(
        start: Vec2,
        end: Vec2,
        start_color: [f32; 4],
        end_color: [f32; 4],
        width: f32,
    ) -> Self {
        Self {
            endpoints: [start.x, start.y, end.x, end.y],
            start_color,
            end_color,
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
    pub zoom_bias: f32,
    pub phase_rate: f32,
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
    /// First two triangle vertices: a.xy, b.xy.
    pub start: [f32; 4],
    /// Third triangle vertex duplicated: c.xy, c.xy.
    pub end: [f32; 4],
    /// Tint and opacity at the first vertex.
    pub start_color: [f32; 4],
    /// Tint and opacity at the second vertex.
    pub end_color: [f32; 4],
    /// Tint and opacity at the third vertex.
    pub third_color: [f32; 4],
    /// Wing-mask flag, padding, padding, padding.
    pub style: [f32; 4],
}

impl RibbonInstance {
    /// A single alpha-interpolated triangle. The three vertices carry
    /// independent colors.
    fn triangle(points: [Vec2; 3], colors: [[f32; 4]; 3]) -> Self {
        Self {
            start: [points[0].x, points[0].y, points[1].x, points[1].y],
            end: [points[2].x, points[2].y, points[2].x, points[2].y],
            start_color: colors[0],
            end_color: colors[1],
            third_color: colors[2],
            style: [0.0; 4],
        }
    }

    /// A crossing wing triangle. The first vertex is the outer A/B endpoint,
    /// the second is the body-edge endpoint, and the third is the dual X point.
    /// The shader masks the outer-to-X edge to zero.
    fn wing_triangle(points: [Vec2; 3], colors: [[f32; 4]; 3]) -> Self {
        let mut ribbon = Self::triangle(points, colors);
        ribbon.style[0] = 1.0;
        ribbon
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
        let thresholds = OnsetThresholds::default();
        let mut rng = Rng::new(0x005e_ed0f_7e5e_1e55u64);
        let scene = Scene::new(&mut rng);
        Self {
            analyzer: SpectrumAnalyzer::new(48_000),
            bands,
            power: Envelope::new(18.0, 4.5),
            bass: Envelope::new(90.0, 7.0),
            treble: Envelope::new(16.0, 3.5),
            bass_onset: Onset::bass(thresholds.bass),
            broadband_onset: Onset::broadband(thresholds.broadband),
            pending_hits: VecDeque::with_capacity(MAX_PENDING_HITS),
            rng,
            scene,
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

    pub fn onset_thresholds(&self) -> OnsetThresholds {
        OnsetThresholds {
            bass: OnsetThreshold(self.bass_onset.threshold),
            broadband: OnsetThreshold(self.broadband_onset.threshold),
        }
    }

    pub fn set_onset_thresholds(&mut self, thresholds: OnsetThresholds) {
        self.bass_onset.threshold = thresholds.bass.value();
        self.broadband_onset.threshold = thresholds.broadband.value();
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
                zoom_bias: style.zoom_bias,
                phase_rate: style.phase_rate,
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
        let accent = self.bands[12].value;
        if let Some(hit) = strongest {
            self.handle_hit(hit, accent);
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

    fn handle_hit(&mut self, hit: Hit, accent: f32) {
        self.kick = self.kick.max(hit.strength);
        if self.transition_cooldown > 0.0 {
            return;
        }
        match roll_transition(hit.kind, &mut self.rng) {
            Some(Transition::Scene) => {
                self.scene.change(&mut self.rng, accent);
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
    fn bass(threshold: OnsetThreshold) -> Self {
        Self {
            previous: 0.0,
            baseline: 0.0,
            cooldown: 0.0,
            threshold: threshold.value(),
            min_flux: 0.035,
            baseline_ratio: 1.08,
            cooldown_time: 0.11,
        }
    }

    fn broadband(threshold: OnsetThreshold) -> Self {
        Self {
            previous: 0.0,
            baseline: 0.0,
            cooldown: 0.0,
            threshold: threshold.value(),
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
    fn new(rng: &mut Rng) -> Self {
        let style = Style::default();
        let tint = FigureTint::spawn(FigureKind::Cube, 0.0, rng);
        Self {
            current: FigureRuntime::new(FigureKind::Cube, 1.0, style, tint),
            outgoing: None,
            transition: 1.0,
        }
    }

    fn change(&mut self, rng: &mut Rng, accent: f32) {
        let next = self.current.kind.next();
        let target_style = Style::random(rng);
        let tint = FigureTint::spawn(next, accent, rng);
        let mut entering = FigureRuntime::new(next, 0.0, target_style, tint);
        entering.pose.yaw += rng.signed() * 0.9;
        entering.pose.pitch += rng.signed() * 0.4;
        entering.phase = rng.unit() * TAU;
        self.outgoing = None;
        self.current.opacity = 1.0;
        let old = std::mem::replace(&mut self.current, entering);
        self.outgoing = Some(old);
        self.transition = 0.0;
    }

    fn mutate(&mut self, rng: &mut Rng) {
        self.current.target_style = Style::random(rng);
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
    tint: FigureTint,
    phase: f32,
    entrance: f32,
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
    start_depth_opacity: f32,
    end_depth_opacity: f32,
}

#[derive(Clone, Copy)]
struct VertexDiagnosis {
    screen: Vec2,
    world: Vec3,
    velocity: Vec2,
    acceleration: Vec2,
}

impl FigureRuntime {
    fn new(kind: FigureKind, opacity: f32, target_style: Style, tint: FigureTint) -> Self {
        let pose = Pose::default();
        Self {
            kind,
            wireframe: Wireframe::build(kind),
            pose,
            pitch: Smoother::new(pose.pitch),
            roll: Smoother::new(pose.roll),
            style: Style::default(),
            target_style,
            tint,
            phase: 0.0,
            entrance: opacity,
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
    // 12 accent saturation of newly spawned figures, 13 line energy, 14 jitter,
    // 15 phase speed.
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
        let depth_opacity = DepthOpacity::new(self.vertices.iter().map(|vertex| vertex.depth));
        self.current.clear();
        self.current
            .extend(self.wireframe.edges().iter().map(|edge| {
                let a = self.vertices[edge[0] as usize];
                let b = self.vertices[edge[1] as usize];
                LineGeometry {
                    start: a.screen,
                    end: b.screen,
                    start_depth_opacity: depth_opacity.factor(a.depth),
                    end_depth_opacity: depth_opacity.factor(b.depth),
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
        for (index, current) in self.current.iter().enumerate() {
            let rgb = self.tint.edge_rgb(index, palette, colors);
            let alpha = self.opacity * (0.6 + 0.4 * line_energy);
            let width = base_width * (1.0 + music.kick * 0.25);
            let alpha = alpha.clamp(0.0, 1.0);
            let start_alpha = alpha * current.start_depth_opacity;
            let end_alpha = alpha * current.end_depth_opacity;

            let Some(exposure) = &exposure else {
                out.push(LineInstance::shaded(
                    current.start,
                    current.end,
                    [rgb[0], rgb[1], rgb[2], start_alpha],
                    [rgb[0], rgb[1], rgb[2], end_alpha],
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
            let fade = |distance: f32| 0.35 + 0.65 * width / (width + distance);
            let start_alpha = alpha * fade(distance_a) * middle.start_depth_opacity;
            let end_alpha = alpha * fade(distance_b) * middle.end_depth_opacity;
            let peak_x_alpha =
                alpha * (middle.start_depth_opacity + middle.end_depth_opacity) * 0.5;

            if segment_intersection(first.start, first.end, last.start, last.end).is_some() {
                let direction = (middle.end - middle.start).normalized();
                let mut normal = if direction.length() > 0.0 {
                    direction.perpendicular()
                } else {
                    Vec2::new(0.0, 1.0)
                };
                // Keep A1 on the X1 side regardless of projected edge order.
                if (first.start - middle.start).x * normal.x
                    + (first.start - middle.start).y * normal.y
                    < 0.0
                {
                    normal = normal * -1.0;
                }
                let body_half = (width * 0.5).max(0.5);
                let l_top = middle.start + normal * body_half;
                let l_bottom = middle.start - normal * body_half;
                let r_top = middle.end + normal * body_half;
                let r_bottom = middle.end - normal * body_half;
                let x1 = (l_top + r_top) * 0.5;
                let x2 = (l_bottom + r_bottom) * 0.5;
                let clear = [rgb[0], rgb[1], rgb[2], 0.0];
                let am = [rgb[0], rgb[1], rgb[2], start_alpha];
                let bm = [rgb[0], rgb[1], rgb[2], end_alpha];
                let peak_x = [rgb[0], rgb[1], rgb[2], peak_x_alpha];

                // The body is two flat-capped quads; the shared long-edge
                // gradients are the same U -> X -> V ramps as the wings.
                out_ribbons.push(RibbonInstance::triangle(
                    [l_top, l_bottom, x2],
                    [am, am, peak_x],
                ));
                out_ribbons.push(RibbonInstance::triangle(
                    [l_top, x2, x1],
                    [am, peak_x, peak_x],
                ));
                out_ribbons.push(RibbonInstance::triangle(
                    [x1, x2, r_bottom],
                    [peak_x, peak_x, bm],
                ));
                out_ribbons.push(RibbonInstance::triangle(
                    [x1, r_bottom, r_top],
                    [peak_x, bm, bm],
                ));
                // Top wings attach to the top long edge and meet at X1. Their
                // outer-to-X edges are masked to zero by the wing shader.
                out_ribbons.push(RibbonInstance::wing_triangle(
                    [first.start, l_top, x1],
                    [clear, am, peak_x],
                ));
                out_ribbons.push(RibbonInstance::wing_triangle(
                    [last.end, r_top, x1],
                    [clear, bm, peak_x],
                ));
                // Bottom wings attach to the bottom long edge and meet at X2.
                out_ribbons.push(RibbonInstance::wing_triangle(
                    [last.start, l_bottom, x2],
                    [clear, am, peak_x],
                ));
                out_ribbons.push(RibbonInstance::wing_triangle(
                    [first.end, r_bottom, x2],
                    [clear, bm, peak_x],
                ));
                continue;
            }

            // A fast edge spreads the same ink over more pixels, so its peak
            // opacity is lower. Even a motionless edge keeps the central body.
            let spread = movement / (width + movement);
            let peak = alpha * (0.35 + 0.65 * (1.0 - spread));
            let direction = (middle.end - middle.start).normalized();
            let mut normal = if direction.length() > 0.0 {
                direction.perpendicular()
            } else {
                Vec2::new(0.0, 1.0)
            };
            // Point the body normal from the first pose toward the last pose.
            let motion = ((last.start - first.start) + (last.end - first.end)) * 0.5;
            if normal.x * motion.x + normal.y * motion.y < 0.0 {
                normal = normal * -1.0;
            }
            let half = (width * 0.5).max(0.5);
            let back_start = middle.start - normal * half;
            let back_end = middle.end - normal * half;
            let front_start = middle.start + normal * half;
            let front_end = middle.end + normal * half;
            let clear = [rgb[0], rgb[1], rgb[2], 0.0];
            let opaque_start = [rgb[0], rgb[1], rgb[2], peak * middle.start_depth_opacity];
            let opaque_end = [rgb[0], rgb[1], rgb[2], peak * middle.end_depth_opacity];

            // One continuous strip: incoming gradient, body, outgoing
            // gradient. The body is the middle band, so the boundary vertices
            // and colors are shared exactly with the gradient sections.
            out_ribbons.push(RibbonInstance::triangle(
                [first.start, first.end, back_end],
                [clear, clear, opaque_end],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [first.start, back_end, back_start],
                [clear, opaque_end, opaque_start],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [back_start, back_end, front_end],
                [opaque_start, opaque_end, opaque_end],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [back_start, front_end, front_start],
                [opaque_start, opaque_end, opaque_start],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [front_start, front_end, last.end],
                [opaque_start, opaque_end, clear],
            ));
            out_ribbons.push(RibbonInstance::triangle(
                [front_start, last.end, last.start],
                [opaque_start, clear, clear],
            ));
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

/// A constant-width quad stroke with independent opacity at each end. Used for
/// the short, faded-in intersection cores rather than the full median edges.
/// A single beveled V stroke through X. The two arms share one join edge, so the
/// intersection opacity is composited once instead of twice.
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

fn lerp_line(from: LineGeometry, to: LineGeometry, fraction: f32) -> LineGeometry {
    LineGeometry {
        start: from.start.lerp(to.start, fraction),
        end: from.end.lerp(to.end, fraction),
        start_depth_opacity: from.start_depth_opacity
            + (to.start_depth_opacity - from.start_depth_opacity) * fraction,
        end_depth_opacity: from.end_depth_opacity
            + (to.end_depth_opacity - from.end_depth_opacity) * fraction,
    }
}
#[derive(Clone, Copy)]
struct Style {
    axis: Vec3,
    twist_sign: f32,
    shear_sign: f32,
    deform_bias: f32,
    zoom_bias: f32,
    phase_rate: f32,
    jitter_bias: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            axis: Vec3::new(0.55, 0.25, 0.4).normalized(),
            twist_sign: 1.0,
            shear_sign: 1.0,
            deform_bias: 0.55,
            zoom_bias: 1.0,
            phase_rate: 1.0,
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
            zoom_bias: 0.9 + rng.unit() * 0.25,
            phase_rate: 0.7 + rng.unit() * 0.7,
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
        self.zoom_bias += (target.zoom_bias - self.zoom_bias) * blend;
        self.phase_rate += (target.phase_rate - self.phase_rate) * blend;
        self.jitter_bias += (target.jitter_bias - self.jitter_bias) * blend;
    }
}

/// The palette assignment of one spawned figure: which vivid hues its edges
/// sample and how strongly they are mixed into the neutral base. The assignment
/// is frozen at spawn, so the figure keeps its colors until it leaves the scene.
#[derive(Clone, Copy, Debug)]
struct FigureTint {
    primary_phase: f32,
    secondary_phase: Option<f32>,
    accent_mix: f32,
}

impl FigureTint {
    /// `accent` is the accent band value at spawn; it selects the saturation of
    /// the figure's colors without making them follow the spectrum afterwards.
    fn spawn(kind: FigureKind, accent: f32, rng: &mut Rng) -> Self {
        let primary_phase = rng.unit() * TAU;
        let secondary_phase = kind
            .uses_two_accents()
            .then(|| primary_phase + 1.5 + rng.unit() * 2.0);
        Self {
            primary_phase,
            secondary_phase,
            accent_mix: (0.3 + accent * (0.15 + rng.unit() * 0.45)).clamp(0.0, 0.9),
        }
    }

    /// RGB for one edge. The palette is still sampled live so a theme
    /// crossfade reaches figures that are already on screen.
    fn edge_rgb(self, edge: usize, palette: &Palette, colors: &MeterColors) -> [f32; 3] {
        let phase = match self.secondary_phase {
            Some(secondary) if edge % 2 == 1 => secondary,
            _ => self.primary_phase,
        };
        let base = mix(colors.background, palette.neutral[0], 0.55);
        let tinted = mix(base, palette.vivid_blend(phase), self.accent_mix);
        [
            tinted.components[0],
            tinted.components[1],
            tinted.components[2],
        ]
    }
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
        assert!(frame
            .lines
            .iter()
            .all(|line| line.start_color[3] > 0.0 && line.end_color[3] > 0.0));
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
    fn onset_thresholds_adjust_in_hundredths_and_stay_normalized() {
        let thresholds = OnsetThresholds::default();
        assert_eq!(thresholds.bass.value(), 0.17);
        assert_eq!(thresholds.broadband.value(), 0.22);

        let higher = thresholds.bass.adjusted(Adjustment::Increase);
        assert!((higher.value() - 0.18).abs() < 1e-6);

        let mut lowest = thresholds.bass;
        for _ in 0..200 {
            lowest = lowest.adjusted(Adjustment::Decrease);
        }
        assert_eq!(lowest.value(), 0.0);

        let mut highest = thresholds.broadband;
        for _ in 0..200 {
            highest = highest.adjusted(Adjustment::Increase);
        }
        assert_eq!(highest.value(), 1.0);

        assert!(OnsetThreshold::try_from(-0.01).is_err());
        assert!(OnsetThreshold::try_from(1.01).is_err());
        assert!(OnsetThreshold::try_from(f32::NAN).is_err());
    }

    #[test]
    fn setting_onset_thresholds_reaches_the_debug_snapshot() {
        let mut visualizer = Visualizer::new();
        let thresholds = OnsetThresholds {
            bass: OnsetThreshold::try_from(0.61).unwrap(),
            broadband: OnsetThreshold::try_from(0.72).unwrap(),
        };
        visualizer.set_onset_thresholds(thresholds);
        let snapshot = visualizer.debug_snapshot();
        assert!((snapshot.bass_onset.threshold - 0.61).abs() < 1e-6);
        assert!((snapshot.broadband_onset.threshold - 0.72).abs() < 1e-6);
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
    fn edge_opacity_fades_with_projected_depth() {
        let mut visualizer = Visualizer::new();
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        visualizer.debug_music([0.0; BAND_COUNT], 0.0);
        let frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
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
        let frame = visualizer.frame();
        assert_eq!(frame.lines.len(), 12, "the first frame is the cube edges");
        let alphas: Vec<f32> = frame
            .lines
            .iter()
            .flat_map(|line| [line.start_color[3], line.end_color[3]])
            .collect();
        let min = alphas.iter().copied().fold(f32::INFINITY, f32::min);
        let max = alphas.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (max / min - 2.0).abs() < 1e-4,
            "the farthest endpoint must keep half the nearest opacity: {min}..{max}"
        );
        assert!(
            frame
                .lines
                .iter()
                .any(|line| (line.start_color[3] - line.end_color[3]).abs() > 0.01),
            "an edge must interpolate between its endpoint depths"
        );
    }

    #[test]
    fn the_tesseract_shares_one_edge_color_and_the_torus_alternates_two() {
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut rng = Rng::new(17);
        let tesseract = FigureTint::spawn(FigureKind::Tesseract, 0.8, &mut rng);
        let expected = tesseract.edge_rgb(0, &palette, &colors);
        for edge in 0..Wireframe::build(FigureKind::Tesseract).edges().len() {
            assert_eq!(tesseract.edge_rgb(edge, &palette, &colors), expected);
        }
        let torus = FigureTint::spawn(FigureKind::Torus, 0.8, &mut rng);
        assert_ne!(
            torus.edge_rgb(0, &palette, &colors),
            torus.edge_rgb(1, &palette, &colors)
        );
    }

    #[test]
    fn a_spawned_figure_keeps_its_edge_colors_while_the_music_changes() {
        let mut visualizer = Visualizer::new();
        visualizer.scene.current.kind = FigureKind::Tesseract;
        visualizer.scene.current.wireframe = Wireframe::build(FigureKind::Tesseract);
        visualizer.scene.current.reset_temporal();
        visualizer.scene.current.tint =
            FigureTint::spawn(FigureKind::Tesseract, 0.8, &mut visualizer.rng);
        let colors = crate::theme::Theme::new(crate::theme::ThemeId::default()).colors;
        let palette = Palette::from_theme(crate::theme::ThemeId::default());
        let mut frame = AudioInterval::new(AudioTime::ZERO, AudioTime::ZERO.shifted(1.0 / 60.0));
        let mut expected: Option<[f32; 3]> = None;
        for step in 0..30 {
            let level = step as f32 / 29.0;
            visualizer.debug_music([level; BAND_COUNT], level);
            visualizer.kick = level;
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
            let frame = visualizer.frame();
            let mut rgbs: Vec<[f32; 3]> = Vec::new();
            for line in frame.lines {
                for color in [line.start_color, line.end_color] {
                    if color[3] > 0.0 {
                        rgbs.push([color[0], color[1], color[2]]);
                    }
                }
            }
            for ribbon in frame.ribbons {
                for color in [ribbon.start_color, ribbon.end_color, ribbon.third_color] {
                    if color[3] > 0.0 {
                        rgbs.push([color[0], color[1], color[2]]);
                    }
                }
            }
            assert!(!rgbs.is_empty());
            let first = *expected.get_or_insert(rgbs[0]);
            for rgb in rgbs {
                assert_eq!(rgb, first, "edge colors changed with the music");
            }
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
        for step in 0..120 {
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
            if step == 0 {
                // Make the follow test independent of which vertex the RNG
                // picked: the top vertices clamp the readout anchor to the
                // viewport edge and can keep it there for the whole run.
                let vertex = visualizer
                    .scene
                    .current
                    .vertices
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.screen.y.total_cmp(&b.screen.y))
                    .map(|(index, _)| index)
                    .expect("the cube has vertices");
                visualizer.diagnosis.vertex = vertex;
                visualizer.diagnosis.anchor = visualizer.diagnosis_target((1280.0, 440.0), 1.0);
                visualizer.diagnosis.refresh_timer = 0.0;
            }
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
        assert!(
            !frame.lines.is_empty(),
            "diagnosis must add line instancing"
        );
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
        assert!(snapshot.lines + snapshot.ribbons > 0);
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
