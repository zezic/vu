//! Normal and POI-focused showcase modes, with the selector effect state.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::{
    computation::validate_value,
    detection::{
        Spring, SPRING_DAMPING_MAX, SPRING_DAMPING_MIN, SPRING_STIFFNESS_MAX, SPRING_STIFFNESS_MIN,
    },
    geometry::Vec3,
    model::Anchor,
    Rng,
};

pub const SELECTOR_ATTENUATION_MIN: f32 = 0.0;
pub const SELECTOR_ATTENUATION_MAX: f32 = 1.0;
pub const SELECTOR_FADE_IN_MIN: f32 = 0.01;
pub const SELECTOR_FADE_IN_MAX: f32 = 2.0;
pub const SELECTOR_FADE_OUT_MIN: f32 = 0.005;
pub const SELECTOR_FADE_OUT_MAX: f32 = 0.5;

/// Fraction of the viewport half-extent used by the focused POI. Leaves 15%
/// padding on each side.
const FOCUS_FILL: f32 = 0.70;
const POI_STEP_MIN: f32 = 0.0;
const POI_STEP_MAX: f32 = std::f32::consts::FRAC_PI_2;
const POI_FOV_MIN: f32 = 30.0_f32.to_radians();
const POI_FOV_MAX: f32 = 120.0_f32.to_radians();
const RECENT_POIS: usize = 2;

/// Every tunable showcase parameter. Stored with the rest of the inspector
/// tuning so the settings survive a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShowcaseSettings {
    /// Spring for the normal-to-POI camera blend and the focus target.
    pub focus_stiffness: f32,
    pub focus_damping: f32,
    /// Slower spring that fades the model's spin, flip, and pulse out.
    pub motion_stiffness: f32,
    pub motion_damping: f32,
    /// Spring for the POI yaw and pitch orbit.
    pub orbit_stiffness: f32,
    pub orbit_damping: f32,
    /// Spring for the POI FOV and compensating dolly distance.
    pub dolly_stiffness: f32,
    pub dolly_damping: f32,
    /// Opacity multiplier for meshes beside the selected one.
    pub selector_attenuation: f32,
    /// Seconds to fade other meshes out when the selector turns on.
    pub selector_fade_in: f32,
    /// Seconds to bring other meshes back when the selector turns off.
    pub selector_fade_out: f32,
    /// Chance that the selected mesh uses a different palette color.
    pub selector_color_chance: f32,
}

impl Default for ShowcaseSettings {
    fn default() -> Self {
        Self {
            focus_stiffness: 90.0,
            focus_damping: 18.0,
            motion_stiffness: 12.0,
            motion_damping: 7.0,
            orbit_stiffness: 70.0,
            orbit_damping: 16.0,
            dolly_stiffness: 55.0,
            dolly_damping: 14.0,
            selector_attenuation: 0.12,
            selector_fade_in: 0.30,
            selector_fade_out: 0.045,
            selector_color_chance: 0.25,
        }
    }
}

impl ShowcaseSettings {
    pub fn validate(&self) -> anyhow::Result<()> {
        for (name, value) in [
            ("focus_stiffness", self.focus_stiffness),
            ("motion_stiffness", self.motion_stiffness),
            ("orbit_stiffness", self.orbit_stiffness),
            ("dolly_stiffness", self.dolly_stiffness),
        ] {
            validate_value(value, (SPRING_STIFFNESS_MIN, SPRING_STIFFNESS_MAX), name)?;
        }
        for (name, value) in [
            ("focus_damping", self.focus_damping),
            ("motion_damping", self.motion_damping),
            ("orbit_damping", self.orbit_damping),
            ("dolly_damping", self.dolly_damping),
        ] {
            validate_value(value, (SPRING_DAMPING_MIN, SPRING_DAMPING_MAX), name)?;
        }
        validate_value(
            self.selector_attenuation,
            (SELECTOR_ATTENUATION_MIN, SELECTOR_ATTENUATION_MAX),
            "selector attenuation",
        )?;
        validate_value(
            self.selector_fade_in,
            (SELECTOR_FADE_IN_MIN, SELECTOR_FADE_IN_MAX),
            "selector fade in",
        )?;
        validate_value(
            self.selector_fade_out,
            (SELECTOR_FADE_OUT_MIN, SELECTOR_FADE_OUT_MAX),
            "selector fade out",
        )?;
        validate_value(
            self.selector_color_chance,
            (0.0, 1.0),
            "selector color chance",
        )?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowcaseMode {
    Normal,
    Poi,
}

/// One catalog POI that resolves to a node in the current model.
#[derive(Clone, Debug)]
pub struct FocusPoi {
    pub name: String,
    pub anchor: Anchor,
    pub ejection_port: bool,
}

impl FocusPoi {
    pub fn new(name: String, anchor: Anchor, ejection_port: bool) -> Self {
        Self {
            name,
            anchor,
            ejection_port,
        }
    }
}

/// Result of a snare trigger for the normal-mode flip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SnareOutcome {
    pub flip: bool,
}

/// The camera parameters produced by the showcase springs.
#[derive(Clone, Copy, Debug)]
pub struct FocusView {
    pub center: Vec3,
    pub radius: f32,
    pub orbit_yaw: f32,
    pub orbit_pitch: f32,
    pub fov: f32,
    pub blend: f32,
    /// One keeps the normal model motion, zero fades it fully out.
    pub model_motion: f32,
    /// Whether POI mode is asking the model motion to fade out.
    pub freeze_motion: bool,
}

/// Per-edge selector effect for the current frame.
#[derive(Clone, Copy, Debug)]
pub struct SelectorView {
    /// Mesh part that keeps full opacity.
    pub selected: Option<u16>,
    /// Opacity factor applied to every other mesh.
    pub attenuation: f32,
    /// Whether the selected mesh uses the alternate palette color.
    pub alternate_color: bool,
}

impl SelectorView {
    pub const NONE: Self = Self {
        selected: None,
        attenuation: 1.0,
        alternate_color: false,
    };
}

pub struct ShowcaseFrame {
    pub focus: FocusView,
    pub highlight_part: Option<u16>,
    /// Translation applied to the slide subtree in normalized model space.
    pub slide_offset: Vec3,
    pub selector: SelectorView,
}

pub struct Showcase {
    settings: ShowcaseSettings,
    mode: ShowcaseMode,
    blend: Spring,
    motion: Spring,
    center: [Spring; 3],
    radius: Spring,
    orbit_yaw: Spring,
    orbit_pitch: Spring,
    fov: Spring,
    slide: Spring,
    target: Option<FocusPoi>,
    pois: Vec<FocusPoi>,
    current: Option<usize>,
    recent: VecDeque<usize>,
    normal_flips: u32,
    poi_switches: u32,
    selector: SelectorState,
    part_count: u16,
    slide_distance: f32,
}

impl Showcase {
    pub fn new(settings: ShowcaseSettings) -> Self {
        Self {
            settings,
            mode: ShowcaseMode::Normal,
            blend: Spring::default(),
            motion: {
                let mut spring = Spring::default();
                spring.reset(1.0);
                spring
            },
            center: [Spring::default(); 3],
            radius: Spring::default(),
            orbit_yaw: Spring::default(),
            orbit_pitch: Spring::default(),
            fov: Spring::default(),
            slide: Spring::default(),
            target: None,
            pois: Vec::new(),
            current: None,
            recent: VecDeque::with_capacity(RECENT_POIS),
            normal_flips: 0,
            poi_switches: 0,
            selector: SelectorState::default(),
            part_count: 0,
            slide_distance: 0.0,
        }
    }

    pub fn settings(&self) -> &ShowcaseSettings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: ShowcaseSettings) {
        settings
            .validate()
            .expect("showcase settings are validated before they reach the showcase");
        self.settings = settings;
    }

    pub fn settings_mut(&mut self) -> &mut ShowcaseSettings {
        &mut self.settings
    }

    pub fn mode(&self) -> ShowcaseMode {
        self.mode
    }

    pub fn target_name(&self) -> Option<&str> {
        self.target.as_ref().map(|poi| poi.name.as_str())
    }

    pub fn poi_count(&self) -> usize {
        self.pois.len()
    }

    pub fn normal_flips(&self) -> u32 {
        self.normal_flips
    }

    pub fn poi_switches(&self) -> u32 {
        self.poi_switches
    }

    pub fn selector_active(&self) -> bool {
        self.selector.active
    }

    pub fn selected_mesh(&self) -> Option<u16> {
        self.selector.selected
    }

    /// Replace the POIs available for the current model and restart the cycle.
    pub fn set_model(&mut self, pois: Vec<FocusPoi>, part_count: u16) {
        self.pois = pois;
        self.part_count = part_count;
        self.current = None;
        self.target = None;
        self.recent.clear();
        self.mode = ShowcaseMode::Normal;
        self.blend.reset(0.0);
        self.motion.reset(1.0);
        for spring in &mut self.center {
            spring.reset(0.0);
        }
        self.radius.reset(0.0);
        self.orbit_yaw.reset(0.0);
        self.orbit_pitch.reset(0.0);
        self.fov.reset(0.0);
        self.slide.reset(0.0);
        self.normal_flips = 0;
        self.poi_switches = 0;
        self.selector = SelectorState::default();
        self.slide_distance = 0.0;
    }

    /// Keep the current model's POIs but start a fresh performance.
    pub fn reset_cycle(&mut self) {
        self.mode = ShowcaseMode::Normal;
        self.blend.reset(0.0);
        self.motion.reset(1.0);
        self.current = None;
        self.target = None;
        self.recent.clear();
        self.normal_flips = 0;
        self.poi_switches = 0;
        self.selector = SelectorState::default();
        self.slide.reset(0.0);
        self.orbit_yaw.reset(0.0);
        self.orbit_pitch.reset(0.0);
        self.fov.reset(0.0);
    }

    pub fn snare(
        &mut self,
        rng: &mut Rng,
        flips_before_poi: u32,
        switches_before_normal: u32,
        normal_fov: f32,
    ) -> SnareOutcome {
        self.selector.deactivate();
        let flips_before_poi = flips_before_poi.max(1);
        let switches_before_normal = switches_before_normal.max(1);
        match self.mode {
            ShowcaseMode::Poi => {
                if self.poi_switches < switches_before_normal {
                    self.poi_switches += 1;
                    self.select_next_poi(rng);
                } else {
                    self.exit_poi();
                }
                SnareOutcome { flip: false }
            }
            ShowcaseMode::Normal => {
                if self.normal_flips < flips_before_poi {
                    self.normal_flips += 1;
                    SnareOutcome { flip: true }
                } else if self.pois.is_empty() {
                    self.normal_flips = 0;
                    SnareOutcome { flip: true }
                } else {
                    self.normal_flips = 0;
                    self.enter_poi(rng, normal_fov);
                    SnareOutcome { flip: false }
                }
            }
        }
    }

    pub fn kick(&mut self, rng: &mut Rng) {
        if self.mode == ShowcaseMode::Poi {
            self.selector.deactivate();
            let yaw = POI_STEP_MIN + rng.unit() * (POI_STEP_MAX - POI_STEP_MIN);
            let pitch = POI_STEP_MIN + rng.unit() * (POI_STEP_MAX - POI_STEP_MIN);
            self.orbit_yaw.set_target(self.orbit_yaw.value() + yaw);
            self.orbit_pitch
                .set_target(self.orbit_pitch.value() + pitch);
            let fov = POI_FOV_MIN + rng.unit() * (POI_FOV_MAX - POI_FOV_MIN);
            self.fov.set_target(fov);
            return;
        }
        if self.selector.active {
            self.selector.deactivate();
        } else {
            self.selector.activate(rng, self.part_count, &self.settings);
        }
    }

    pub fn hat(&mut self, rng: &mut Rng) {
        self.selector.select_mesh(rng, self.part_count);
        if self.selector.active {
            self.selector.roll_color(rng, &self.settings);
        }
    }

    pub fn drop(&mut self) {
        self.selector.deactivate();
    }

    pub fn gap_entry(&mut self) {
        self.normal_flips = 0;
        if self.mode == ShowcaseMode::Poi {
            self.exit_poi();
        }
        self.selector.deactivate();
    }

    pub fn advance(&mut self, dt: f32) {
        let settings = &self.settings;
        self.blend
            .advance(dt, settings.focus_stiffness, settings.focus_damping);
        self.motion
            .advance(dt, settings.motion_stiffness, settings.motion_damping);
        for spring in &mut self.center {
            spring.advance(dt, settings.focus_stiffness, settings.focus_damping);
        }
        self.radius
            .advance(dt, settings.focus_stiffness, settings.focus_damping);
        self.orbit_yaw
            .advance(dt, settings.orbit_stiffness, settings.orbit_damping);
        self.orbit_pitch
            .advance(dt, settings.orbit_stiffness, settings.orbit_damping);
        self.fov
            .advance(dt, settings.dolly_stiffness, settings.dolly_damping);
        self.slide
            .advance(dt, settings.focus_stiffness, settings.focus_damping);
        self.selector.advance(dt, settings);
    }

    pub fn frame(&self) -> ShowcaseFrame {
        let blend = self.blend.value().clamp(0.0, 1.0);
        let target = self.target.as_ref();
        let center = Vec3::new(
            self.center[0].value(),
            self.center[1].value(),
            self.center[2].value(),
        );
        let fov = self.fov.value().clamp(0.017, std::f32::consts::PI - 0.017);
        let slide = self.slide.value().clamp(0.0, 1.0);
        ShowcaseFrame {
            focus: FocusView {
                center,
                radius: self.radius.value().max(0.0),
                orbit_yaw: self.orbit_yaw.value(),
                orbit_pitch: self.orbit_pitch.value(),
                fov,
                blend,
                model_motion: self.motion.value().clamp(0.0, 1.0),
                freeze_motion: self.mode == ShowcaseMode::Poi,
            },
            highlight_part: if blend > 0.01 {
                target.and_then(|poi| poi.anchor.part)
            } else {
                None
            },
            slide_offset: Vec3::new(0.0, 0.0, -self.slide_distance * slide),
            selector: self.selector.view(&self.settings),
        }
    }

    fn enter_poi(&mut self, rng: &mut Rng, normal_fov: f32) {
        let Some(index) = self.choose_poi(rng) else {
            self.mode = ShowcaseMode::Normal;
            self.blend.set_target(0.0);
            self.motion.set_target(1.0);
            return;
        };
        let settled = self.blend.value() <= 0.001;
        self.mode = ShowcaseMode::Poi;
        self.poi_switches = 0;
        self.blend.set_target(1.0);
        self.motion.set_target(0.0);
        self.current = Some(index);
        self.target = Some(self.pois[index].clone());
        let anchor = self.pois[index].anchor;
        let center = [anchor.center.x, anchor.center.y, anchor.center.z];
        if settled {
            self.orbit_yaw.reset(0.0);
            self.orbit_pitch.reset(0.0);
            self.fov.reset(normal_fov);
            for (spring, value) in self.center.iter_mut().zip(center) {
                spring.reset(value);
            }
            self.radius.reset(anchor.radius);
            self.slide.reset(0.0);
        } else {
            self.orbit_yaw.set_target(0.0);
            self.orbit_pitch.set_target(0.0);
            self.fov.set_target(normal_fov);
            for (spring, value) in self.center.iter_mut().zip(center) {
                spring.set_target(value);
            }
            self.radius.set_target(anchor.radius);
        }
        self.set_slide_target();
    }

    fn select_next_poi(&mut self, rng: &mut Rng) {
        let Some(index) = self.choose_poi(rng) else {
            return;
        };
        self.current = Some(index);
        self.target = Some(self.pois[index].clone());
        let anchor = self.pois[index].anchor;
        self.center[0].set_target(anchor.center.x);
        self.center[1].set_target(anchor.center.y);
        self.center[2].set_target(anchor.center.z);
        self.radius.set_target(anchor.radius);
        self.set_slide_target();
    }

    fn choose_poi(&mut self, rng: &mut Rng) -> Option<usize> {
        if self.pois.is_empty() {
            return None;
        }
        let candidate = |index: usize| Some(index) != self.current && !self.recent.contains(&index);
        let mut pool: Vec<_> = (0..self.pois.len())
            .filter(|&index| candidate(index))
            .collect();
        if pool.is_empty() {
            pool = (0..self.pois.len())
                .filter(|&index| Some(index) != self.current)
                .collect();
        }
        if pool.is_empty() {
            pool = (0..self.pois.len()).collect();
        }
        let index = pool[rng.range(pool.len())];
        if let Some(current) = self.current {
            if self.recent.len() == RECENT_POIS {
                self.recent.pop_front();
            }
            self.recent.push_back(current);
        }
        Some(index)
    }

    fn exit_poi(&mut self) {
        self.mode = ShowcaseMode::Normal;
        self.blend.set_target(0.0);
        self.motion.set_target(1.0);
        self.poi_switches = 0;
        self.slide.set_target(0.0);
        self.selector.deactivate();
    }

    fn set_slide_target(&mut self) {
        let Some(target) = &self.target else {
            self.slide.set_target(0.0);
            return;
        };
        if target.ejection_port {
            self.slide_distance = target.anchor.size.z;
            self.slide.set_target(1.0);
        } else {
            self.slide.set_target(0.0);
        }
    }
}

#[derive(Default)]
struct SelectorState {
    active: bool,
    progress: f32,
    selected: Option<u16>,
    alternate_color: bool,
}

impl SelectorState {
    fn activate(&mut self, rng: &mut Rng, part_count: u16, settings: &ShowcaseSettings) {
        if self.selected.is_none_or(|part| part >= part_count) {
            self.selected = pick_mesh(rng, None, part_count);
        }
        self.active = true;
        self.roll_color(rng, settings);
    }

    fn deactivate(&mut self) {
        self.active = false;
        self.alternate_color = false;
    }

    fn select_mesh(&mut self, rng: &mut Rng, part_count: u16) {
        self.selected = pick_mesh(rng, self.selected, part_count);
    }

    fn roll_color(&mut self, rng: &mut Rng, settings: &ShowcaseSettings) {
        self.alternate_color =
            self.selected.is_some() && rng.unit() < settings.selector_color_chance;
    }

    fn advance(&mut self, dt: f32, settings: &ShowcaseSettings) {
        let target = if self.active { 1.0 } else { 0.0 };
        let duration = if target > self.progress {
            settings.selector_fade_in
        } else {
            settings.selector_fade_out
        };
        let change = if duration <= f32::EPSILON {
            1.0
        } else {
            dt / duration
        };
        self.progress += (target - self.progress).clamp(-change, change);
        self.progress = self.progress.clamp(0.0, 1.0);
    }

    fn view(&self, settings: &ShowcaseSettings) -> SelectorView {
        if self.selected.is_none() || self.progress <= 0.0 {
            return SelectorView::NONE;
        }
        let progress = smoothstep(self.progress);
        SelectorView {
            selected: self.selected,
            attenuation: 1.0 - progress * (1.0 - settings.selector_attenuation),
            alternate_color: self.active && self.alternate_color,
        }
    }
}

fn pick_mesh(rng: &mut Rng, current: Option<u16>, part_count: u16) -> Option<u16> {
    if part_count == 0 {
        return None;
    }
    if part_count == 1 {
        return Some(0);
    }
    let current = current.filter(|part| *part < part_count);
    let offset = 1 + rng.range(usize::from(part_count) - 1);
    Some(match current {
        Some(current) => (current + offset as u16) % part_count,
        None => rng.range(usize::from(part_count)) as u16,
    })
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

/// Camera distance that fits a sphere of `radius` into the focused view while
/// keeping its projected size constant across the FOV.
pub fn framed_distance(radius: f32, fov: f32) -> f32 {
    let radius = radius.max(1e-4);
    let tangent = (fov * 0.5).clamp(0.01, 1.5).tan();
    radius * (1.0 + 1.0 / (FOCUS_FILL * tangent).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(x: f32, part: Option<u16>) -> Anchor {
        Anchor {
            center: Vec3::new(x, 0.0, 0.0),
            radius: 0.2,
            size: Vec3::new(0.05, 0.05, 0.25),
            part,
        }
    }

    fn showcase() -> Showcase {
        let mut showcase = Showcase::new(ShowcaseSettings::default());
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
                FocusPoi::new("magazine".to_owned(), anchor(-0.2, Some(1)), false),
                FocusPoi::new("ejection_port".to_owned(), anchor(0.1, None), true),
            ],
            2,
        );
        showcase
    }

    #[test]
    fn snare_cycle_enters_switches_and_exits() {
        let mut showcase = showcase();
        let mut rng = Rng::new(7);
        assert!(showcase.snare(&mut rng, 2, 2, 0.75).flip);
        assert!(showcase.snare(&mut rng, 2, 2, 0.75).flip);
        assert!(!showcase.snare(&mut rng, 2, 2, 0.75).flip);
        assert_eq!(showcase.mode(), ShowcaseMode::Poi);
        assert_eq!(showcase.poi_switches(), 0);
        assert!(!showcase.snare(&mut rng, 2, 2, 0.75).flip);
        assert_eq!(showcase.poi_switches(), 1);
        assert!(!showcase.snare(&mut rng, 2, 2, 0.75).flip);
        assert_eq!(showcase.poi_switches(), 2);
        assert!(!showcase.snare(&mut rng, 2, 2, 0.75).flip);
        assert_eq!(showcase.mode(), ShowcaseMode::Normal);
        assert!(showcase.snare(&mut rng, 2, 2, 0.75).flip);
    }

    #[test]
    fn gap_entry_resets_the_cycle_and_leaves_poi() {
        let mut showcase = showcase();
        let mut rng = Rng::new(3);
        for _ in 0..3 {
            showcase.snare(&mut rng, 2, 2, 0.75);
        }
        assert_eq!(showcase.mode(), ShowcaseMode::Poi);
        showcase.gap_entry();
        assert_eq!(showcase.mode(), ShowcaseMode::Normal);
        assert_eq!(showcase.normal_flips(), 0);
        assert!(showcase.snare(&mut rng, 2, 2, 0.75).flip);
    }

    #[test]
    fn kick_toggles_selector_in_normal_mode_only() {
        let mut showcase = showcase();
        let mut rng = Rng::new(11);
        showcase.kick(&mut rng);
        assert!(showcase.selector_active());
        showcase.kick(&mut rng);
        assert!(!showcase.selector_active());
        for _ in 0..3 {
            showcase.snare(&mut rng, 1, 2, 0.75);
        }
        assert_eq!(showcase.mode(), ShowcaseMode::Poi);
        showcase.kick(&mut rng);
        assert!(!showcase.selector_active());
    }

    #[test]
    fn hat_changes_the_selected_mesh() {
        let mut showcase = showcase();
        let mut rng = Rng::new(5);
        showcase.kick(&mut rng);
        let first = showcase.selected_mesh();
        assert!(first.is_some());
        showcase.hat(&mut rng);
        assert_ne!(showcase.selected_mesh(), first);
    }

    #[test]
    fn focus_springs_settle_on_the_selected_poi() {
        let mut showcase = showcase();
        let mut rng = Rng::new(9);
        for _ in 0..3 {
            showcase.snare(&mut rng, 1, 2, 0.75);
        }
        assert_eq!(showcase.mode(), ShowcaseMode::Poi);
        for _ in 0..240 {
            showcase.advance(1.0 / 60.0);
        }
        let frame = showcase.frame();
        assert!((frame.focus.blend - 1.0).abs() < 0.02);
        assert!(frame.highlight_part.is_some() || frame.focus.center.length() > 0.0);
        assert!(frame.focus.fov > 0.0 && frame.focus.fov < std::f32::consts::PI);
    }

    #[test]
    fn model_motion_fades_out_and_back_instead_of_snapping() {
        let mut showcase = showcase();
        let mut rng = Rng::new(6);
        assert!(showcase.snare(&mut rng, 1, 2, 0.75).flip);
        assert!(!showcase.snare(&mut rng, 1, 2, 0.75).flip);
        assert_eq!(showcase.mode(), ShowcaseMode::Poi);
        assert!(showcase.frame().focus.model_motion > 0.99);
        showcase.advance(1.0 / 60.0);
        assert!(
            showcase.frame().focus.model_motion > 0.9,
            "the model does not freeze in one frame"
        );
        for _ in 0..120 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.frame().focus.model_motion < 0.05);
        showcase.exit_poi();
        showcase.advance(1.0 / 60.0);
        assert!(showcase.frame().focus.model_motion < 0.1);
        for _ in 0..120 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.frame().focus.model_motion > 0.95);
    }

    #[test]
    fn re_entry_mid_exit_keeps_center_and_fov_continuous() {
        let mut showcase = showcase();
        let mut rng = Rng::new(12);
        showcase.enter_poi(&mut rng, 0.75);
        for _ in 0..180 {
            showcase.advance(1.0 / 60.0);
        }
        showcase.kick(&mut rng);
        for _ in 0..60 {
            showcase.advance(1.0 / 60.0);
        }
        showcase.exit_poi();
        for _ in 0..5 {
            showcase.advance(1.0 / 60.0);
        }
        let before = showcase.frame();
        assert!(before.focus.blend > 0.1, "the exit is still in flight");

        showcase.enter_poi(&mut rng, 0.75);
        let after = showcase.frame();
        assert!(
            (after.focus.center - before.focus.center).length() < 1e-4,
            "the center must not reset on re-entry"
        );
        assert!(
            (after.focus.fov - before.focus.fov).abs() < 1e-4,
            "the FOV must not reset on re-entry"
        );
        assert!((after.focus.blend - before.focus.blend).abs() < 1e-4);
        assert!((after.focus.model_motion - before.focus.model_motion).abs() < 1e-4);
    }

    #[test]
    fn ejection_port_targets_the_slide_travel() {
        let mut showcase = Showcase::new(ShowcaseSettings::default());
        showcase.set_model(
            vec![FocusPoi::new(
                "ejection_port".to_owned(),
                anchor(0.1, None),
                true,
            )],
            1,
        );
        let mut rng = Rng::new(1);
        assert!(showcase.snare(&mut rng, 1, 2, 0.75).flip);
        assert!(!showcase.snare(&mut rng, 1, 2, 0.75).flip);
        assert_eq!(showcase.mode(), ShowcaseMode::Poi);
        for _ in 0..240 {
            showcase.advance(1.0 / 60.0);
        }
        let frame = showcase.frame();
        assert!(frame.focus.blend > 0.9);
        assert!(
            frame.slide_offset.z < -0.2,
            "the slide travels by the ejection port length"
        );
    }

    #[test]
    fn selector_fades_out_slowly_and_returns_quickly() {
        let mut showcase = showcase();
        let mut rng = Rng::new(2);
        showcase.kick(&mut rng);
        assert!(showcase.selector_active());
        for _ in 0..18 {
            showcase.advance(1.0 / 60.0);
        }
        let active = showcase.frame().selector;
        assert_eq!(active.selected, showcase.selected_mesh());
        assert!(active.attenuation < 0.2);
        showcase.kick(&mut rng);
        assert!(!showcase.selector_active());
        for _ in 0..3 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.frame().selector.attenuation > 0.95);
    }

    #[test]
    fn snare_disables_the_selector() {
        let mut showcase = showcase();
        let mut rng = Rng::new(4);
        showcase.kick(&mut rng);
        assert!(showcase.selector_active());
        showcase.snare(&mut rng, 4, 2, 0.75);
        assert!(!showcase.selector_active());
        for _ in 0..3 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.frame().selector.attenuation > 0.95);
    }
}
