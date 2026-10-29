//! Normal and POI-focused showcase modes, with the selector effect state.

use std::{collections::VecDeque, f32::consts::TAU};

use serde::{Deserialize, Serialize};

use super::{
    computation::validate_value,
    detection::{
        Spring, SPRING_DAMPING_MAX, SPRING_DAMPING_MIN, SPRING_STIFFNESS_MAX, SPRING_STIFFNESS_MIN,
    },
    geometry::{wrap_angle, Vec3},
    model::Anchor,
    Rng,
};

pub const SELECTOR_ATTENUATION_MIN: f32 = 0.0;
pub const SELECTOR_ATTENUATION_MAX: f32 = 1.0;
pub const SELECTOR_FADE_IN_MIN: f32 = 0.01;
pub const SELECTOR_FADE_IN_MAX: f32 = 2.0;
pub const SELECTOR_FADE_OUT_MIN: f32 = 0.005;
pub const SELECTOR_FADE_OUT_MAX: f32 = 0.5;
pub const SELECTOR_EXIT_TRIGGERS_MIN: u32 = 1;
pub const SELECTOR_EXIT_TRIGGERS_MAX: u32 = 100;
pub const SELECTOR_GAP_TIMEOUT_MIN: f32 = 1.0;
pub const SELECTOR_GAP_TIMEOUT_MAX: f32 = 60.0;

/// Fraction of the viewport half-extent used by the focused POI. Leaves 15%
/// padding on each side.
const FOCUS_FILL: f32 = 0.70;
pub const POI_ORBIT_STEP_MAX: f32 = std::f32::consts::FRAC_PI_2;
pub const POI_ORBIT_STEP_DEFAULT: f32 = std::f32::consts::FRAC_PI_4;
pub const POI_ORBIT_STEP_MIN_DEFAULT: f32 = std::f32::consts::PI / 9.0;
/// The shortest path to a reseed target is at most a half turn, so a half-turn
/// maximum leaves the switch rotation uncapped.
pub const POI_SWITCH_STEP_MAX: f32 = std::f32::consts::PI;
pub const POI_SWITCH_STEP_DEFAULT: f32 = POI_ORBIT_STEP_DEFAULT;
pub const POI_SWITCH_STEP_MIN_DEFAULT: f32 = 0.0;
pub const POI_FOV_STEP_MAX: f32 = std::f32::consts::FRAC_PI_2;
pub const POI_FOV_STEP_DEFAULT: f32 = std::f32::consts::PI / 9.0;
pub const POI_FOV_STEP_MIN_DEFAULT: f32 = std::f32::consts::PI / 18.0;
pub const POI_ORBIT_SPEED_DEFAULT: f32 = std::f32::consts::PI / 30.0;
pub const POI_ORBIT_SPEED_MAX: f32 = std::f32::consts::PI / 6.0;
pub const POI_DOLLY_DEPTH_DEFAULT: f32 = 0.15;
pub const POI_DOLLY_DEPTH_MAX: f32 = 0.5;
pub const POI_DOLLY_PERIOD_DEFAULT: f32 = 12.0;
pub const POI_DOLLY_PERIOD_MIN: f32 = 2.0;
pub const POI_DOLLY_PERIOD_MAX: f32 = 60.0;
pub const POI_SWITCH_GAP_MIN: f32 = 0.0;
pub const POI_SWITCH_GAP_MAX: f32 = 10.0;
pub const POI_SWITCH_GAP_DEFAULT: f32 = 1.0;
pub const KICK_ATTACK_MIN: f32 = 0.0;
pub const KICK_ATTACK_MAX: f32 = 100.0;
pub const KICK_ATTACK_DEFAULT: f32 = 4.0;
const POI_FOV_MIN: f32 = 30.0_f32.to_radians();
const POI_FOV_MAX: f32 = 120.0_f32.to_radians();
const RECENT_POIS: usize = 2;
/// The entry blend must be both visually complete and nearly motionless
/// before kick-driven orbit and FOV changes are accepted.
const ENTRY_SETTLED_BLEND: f32 = 0.999;
const ENTRY_SETTLED_VELOCITY: f32 = 0.02;

/// Every tunable showcase parameter. Stored with the rest of the inspector
/// tuning so the settings survive a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShowcaseSettings {
    /// Spring for the normal-to-POI camera blend and the focus target.
    pub focus_stiffness: f32,
    pub focus_damping: f32,
    /// Slower spring that fades the model's spin, flip, and pulse out.
    pub motion_stiffness: f32,
    pub motion_damping: f32,
    /// Initial velocity fed into the kick springs, as a multiple of the
    /// target step per second.
    pub kick_attack: f32,
    /// Smallest signed yaw or pitch target step magnitude per kick.
    pub orbit_step_min: f32,
    /// Largest signed yaw or pitch target step magnitude per kick.
    pub orbit_step_max: f32,
    /// Smallest yaw or pitch rotation applied when entering POI mode or
    /// switching to another POI. Zero turns only as far as the new target
    /// requires.
    pub poi_switch_step_min: f32,
    /// Largest yaw or pitch rotation applied when entering POI mode or
    /// switching to another POI. A farther target is reached over several
    /// switches instead of one swing.
    pub poi_switch_step_max: f32,
    /// Smallest absolute FOV target change per kick.
    pub fov_step_min: f32,
    /// Largest absolute FOV target change per kick.
    pub fov_step_max: f32,
    /// Slow automatic orbit yaw speed while POI mode is settled, rad/s.
    pub orbit_speed: f32,
    /// Slow automatic dolly depth as a fraction of the focus distance.
    pub dolly_depth: f32,
    /// Seconds for one slow dolly in/out cycle.
    pub dolly_period: f32,
    /// Minimum seconds between POI switches. Snares that arrive earlier are
    /// handled as kicks instead.
    pub poi_switch_gap: f32,
    /// Spring for the POI yaw and pitch orbit.
    pub orbit_stiffness: f32,
    pub orbit_damping: f32,
    /// Spring for the POI FOV and compensating dolly distance.
    pub dolly_stiffness: f32,
    pub dolly_damping: f32,
    /// Opacity multiplier for meshes beside the selected one.
    pub selector_attenuation: f32,
    /// Opacity multiplier for meshes beside a focused mesh POI.
    pub poi_attenuation: f32,
    /// Seconds to fade the other meshes out when a mesh effect turns on.
    pub selector_fade_in: f32,
    /// Seconds to bring other meshes back when the selector turns off.
    pub selector_fade_out: f32,
    /// Chance that the selected mesh uses a different palette color.
    pub selector_color_chance: f32,
    /// Kick/snare triggers required before the selector turns off.
    pub selector_exit_triggers: u32,
    /// Continuous seconds of armed gap confidence before the selector turns off.
    pub selector_gap_timeout: f32,
}

impl Default for ShowcaseSettings {
    fn default() -> Self {
        Self {
            focus_stiffness: 90.0,
            focus_damping: 18.0,
            motion_stiffness: 12.0,
            motion_damping: 7.0,
            kick_attack: KICK_ATTACK_DEFAULT,
            orbit_step_min: POI_ORBIT_STEP_MIN_DEFAULT,
            orbit_step_max: POI_ORBIT_STEP_DEFAULT,
            poi_switch_step_min: POI_SWITCH_STEP_MIN_DEFAULT,
            poi_switch_step_max: POI_SWITCH_STEP_DEFAULT,
            fov_step_min: POI_FOV_STEP_MIN_DEFAULT,
            fov_step_max: POI_FOV_STEP_DEFAULT,
            orbit_speed: POI_ORBIT_SPEED_DEFAULT,
            dolly_depth: POI_DOLLY_DEPTH_DEFAULT,
            dolly_period: POI_DOLLY_PERIOD_DEFAULT,
            poi_switch_gap: POI_SWITCH_GAP_DEFAULT,
            orbit_stiffness: 70.0,
            orbit_damping: 16.0,
            dolly_stiffness: 55.0,
            dolly_damping: 14.0,
            selector_attenuation: 0.12,
            poi_attenuation: 0.12,
            selector_fade_in: 0.30,
            selector_fade_out: 0.045,
            selector_color_chance: 0.25,
            selector_exit_triggers: 20,
            selector_gap_timeout: 10.0,
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
            self.kick_attack,
            (KICK_ATTACK_MIN, KICK_ATTACK_MAX),
            "kick attack",
        )?;
        validate_value(
            self.orbit_step_min,
            (0.0, POI_ORBIT_STEP_MAX),
            "orbit step min",
        )?;
        validate_value(
            self.orbit_step_max,
            (0.0, POI_ORBIT_STEP_MAX),
            "orbit step max",
        )?;
        validate_value(self.fov_step_min, (0.0, POI_FOV_STEP_MAX), "FOV step min")?;
        validate_value(self.fov_step_max, (0.0, POI_FOV_STEP_MAX), "FOV step max")?;
        validate_value(
            self.poi_switch_step_min,
            (0.0, POI_SWITCH_STEP_MAX),
            "POI switch step min",
        )?;
        validate_value(
            self.poi_switch_step_max,
            (0.0, POI_SWITCH_STEP_MAX),
            "POI switch step max",
        )?;
        anyhow::ensure!(
            self.orbit_step_min <= self.orbit_step_max,
            "orbit step min must not exceed orbit step max"
        );
        anyhow::ensure!(
            self.fov_step_min <= self.fov_step_max,
            "FOV step min must not exceed FOV step max"
        );
        anyhow::ensure!(
            self.poi_switch_step_min <= self.poi_switch_step_max,
            "POI switch step min must not exceed POI switch step max"
        );
        validate_value(self.orbit_speed, (0.0, POI_ORBIT_SPEED_MAX), "orbit speed")?;
        validate_value(self.dolly_depth, (0.0, POI_DOLLY_DEPTH_MAX), "dolly depth")?;
        validate_value(
            self.dolly_period,
            (POI_DOLLY_PERIOD_MIN, POI_DOLLY_PERIOD_MAX),
            "dolly period",
        )?;
        validate_value(
            self.poi_switch_gap,
            (POI_SWITCH_GAP_MIN, POI_SWITCH_GAP_MAX),
            "POI switch gap",
        )?;
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
        anyhow::ensure!(
            (SELECTOR_EXIT_TRIGGERS_MIN..=SELECTOR_EXIT_TRIGGERS_MAX)
                .contains(&self.selector_exit_triggers),
            "selector exit triggers must be between {SELECTOR_EXIT_TRIGGERS_MIN} and {SELECTOR_EXIT_TRIGGERS_MAX}"
        );
        validate_value(
            self.selector_gap_timeout,
            (SELECTOR_GAP_TIMEOUT_MIN, SELECTOR_GAP_TIMEOUT_MAX),
            "selector gap timeout",
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
    /// Mesh parts below this POI. The parent part is not included.
    pub children: Vec<u16>,
}

impl FocusPoi {
    pub fn new(name: String, anchor: Anchor, ejection_port: bool) -> Self {
        Self {
            name,
            anchor,
            ejection_port,
            children: Vec::new(),
        }
    }

    pub fn with_children(mut self, children: Vec<u16>) -> Self {
        self.children = children;
        self
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
    /// Continuous slow orbit added on top of the kick orbit, in radians.
    pub orbit_drift_yaw: f32,
    /// Multiplier applied to the focus distance for the slow in/out dolly.
    pub dolly_scale: f32,
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
    /// One child of a mesh POI that keeps full opacity while its siblings fade.
    pub focus_child: Option<u16>,
    /// Opacity multiplier for every mesh part except `highlight_part` and
    /// `focus_child`.
    pub mesh_attenuation: f32,
    /// Translation applied to the slide subtree in normalized model space.
    pub slide_offset: Vec3,
    pub selector: SelectorView,
}

pub struct Showcase {
    settings: ShowcaseSettings,
    mode: ShowcaseMode,
    blend: Spring,
    entry_settled: bool,
    motion: Spring,
    center: [Spring; 3],
    radius: Spring,
    orbit_yaw: Spring,
    orbit_pitch: Spring,
    orbit_drift_yaw: f32,
    dolly_phase: f32,
    mesh_fade: f32,
    poi_child: Option<u16>,
    poi_switch_cooldown: f32,
    fov: Spring,
    slide: Spring,
    target: Option<FocusPoi>,
    pois: Vec<FocusPoi>,
    current: Option<usize>,
    recent: VecDeque<usize>,
    normal_flips: u32,
    poi_switches: u32,
    selector: SelectorState,
    gap_armed: bool,
    part_count: u16,
    slide_distance: f32,
}

impl Showcase {
    pub fn new(settings: ShowcaseSettings) -> Self {
        Self {
            settings,
            mode: ShowcaseMode::Normal,
            blend: Spring::default(),
            entry_settled: false,
            motion: {
                let mut spring = Spring::default();
                spring.reset(1.0);
                spring
            },
            center: [Spring::default(); 3],
            radius: Spring::default(),
            orbit_yaw: Spring::default(),
            orbit_pitch: Spring::default(),
            orbit_drift_yaw: 0.0,
            dolly_phase: 0.0,
            mesh_fade: 0.0,
            poi_child: None,
            poi_switch_cooldown: 0.0,
            fov: Spring::default(),
            slide: Spring::default(),
            target: None,
            pois: Vec::new(),
            current: None,
            recent: VecDeque::with_capacity(RECENT_POIS),
            normal_flips: 0,
            poi_switches: 0,
            selector: SelectorState::default(),
            gap_armed: false,
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
        self.entry_settled = false;
        self.motion.reset(1.0);
        for spring in &mut self.center {
            spring.reset(0.0);
        }
        self.radius.reset(0.0);
        self.orbit_yaw.reset(0.0);
        self.orbit_pitch.reset(0.0);
        self.orbit_drift_yaw = 0.0;
        self.dolly_phase = 0.0;
        self.mesh_fade = 0.0;
        self.poi_child = None;
        self.poi_switch_cooldown = 0.0;
        self.fov.reset(0.0);
        self.slide.reset(0.0);
        self.normal_flips = 0;
        self.poi_switches = 0;
        self.selector = SelectorState::default();
        self.gap_armed = false;
        self.slide_distance = 0.0;
    }

    /// Keep the current model's POIs but start a fresh performance.
    pub fn reset_cycle(&mut self) {
        self.mode = ShowcaseMode::Normal;
        self.blend.reset(0.0);
        self.entry_settled = false;
        self.motion.reset(1.0);
        self.current = None;
        self.target = None;
        self.recent.clear();
        self.normal_flips = 0;
        self.poi_switches = 0;
        self.selector = SelectorState::default();
        self.gap_armed = false;
        self.poi_child = None;
        self.slide.reset(0.0);
        self.orbit_yaw.reset(0.0);
        self.orbit_pitch.reset(0.0);
        self.orbit_drift_yaw = 0.0;
        self.dolly_phase = 0.0;
        self.mesh_fade = 0.0;
        self.poi_child = None;
        self.poi_switch_cooldown = 0.0;
        self.fov.reset(0.0);
    }

    pub fn snare(
        &mut self,
        rng: &mut Rng,
        flips_before_poi: u32,
        switches_before_normal: u32,
        normal_fov: f32,
    ) -> SnareOutcome {
        // A snare counts toward the selector exit threshold, and while the
        // selector is still active it must not also start a flip.
        let selector_active = self.selector.active;
        if self.mode == ShowcaseMode::Normal && selector_active {
            self.selector.record_trigger(&self.settings);
        }
        let flips_before_poi = flips_before_poi.max(1);
        let switches_before_normal = switches_before_normal.max(1);
        match self.mode {
            ShowcaseMode::Poi => {
                if self.poi_switch_cooldown > 0.0 {
                    self.kick(rng);
                    return SnareOutcome { flip: false };
                }
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
                    SnareOutcome {
                        flip: !selector_active,
                    }
                } else if self.pois.is_empty() {
                    self.normal_flips = 0;
                    SnareOutcome {
                        flip: !selector_active,
                    }
                } else {
                    self.normal_flips = 0;
                    self.enter_poi(rng, normal_fov);
                    SnareOutcome { flip: false }
                }
            }
        }
    }

    pub fn set_gap_armed(&mut self, armed: bool) {
        self.gap_armed = armed;
    }

    pub fn kick(&mut self, rng: &mut Rng) {
        if self.mode == ShowcaseMode::Poi {
            self.selector.deactivate();
            // The entry owns the camera until it settles. Kicks during the
            // flight are ignored instead of fighting the entry spring.
            if !self.entry_settled {
                return;
            }
            let yaw = signed_step(
                rng,
                self.settings.orbit_step_min,
                self.settings.orbit_step_max,
            );
            let pitch = signed_step(
                rng,
                self.settings.orbit_step_min,
                self.settings.orbit_step_max,
            );
            let attack = self.settings.kick_attack;
            self.orbit_yaw.set_target(self.orbit_yaw.value() + yaw);
            self.orbit_yaw.impulse(yaw * attack);
            self.orbit_pitch
                .set_target(self.orbit_pitch.value() + pitch);
            self.orbit_pitch.impulse(pitch * attack);
            let fov = signed_fov_step(
                rng,
                self.fov.value(),
                self.settings.fov_step_min,
                self.settings.fov_step_max,
            );
            let fov_delta = fov - self.fov.value();
            self.fov.set_target(fov);
            self.fov.impulse(fov_delta * attack);
            return;
        }
        if self.selector.active {
            self.selector.record_trigger(&self.settings);
        } else {
            self.selector.activate(rng, self.part_count, &self.settings);
        }
    }

    pub fn hat(&mut self, rng: &mut Rng) {
        if self.mode == ShowcaseMode::Poi {
            self.advance_poi_child();
            return;
        }
        self.selector.select_mesh(rng, self.part_count);
        if self.selector.active {
            self.selector.roll_color(rng, &self.settings);
        }
    }

    /// Enter POI focus mode from a non-snare trigger. Returns false when the
    /// model has no resolvable POIs.
    pub fn enter_focus(&mut self, rng: &mut Rng, normal_fov: f32) -> bool {
        if self.mode == ShowcaseMode::Poi || self.pois.is_empty() {
            return false;
        }
        self.normal_flips = 0;
        self.enter_poi(rng, normal_fov);
        self.mode == ShowcaseMode::Poi
    }

    /// Leave POI focus mode through the normal exit spring when already there.
    pub fn exit_focus(&mut self) {
        if self.mode == ShowcaseMode::Poi {
            self.exit_poi();
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
    }

    pub fn advance(&mut self, dt: f32) {
        let settings = &self.settings;
        self.blend
            .advance(dt, settings.focus_stiffness, settings.focus_damping);
        if self.mode == ShowcaseMode::Poi
            && !self.entry_settled
            && self.blend.value() >= ENTRY_SETTLED_BLEND
            && self.blend.velocity().abs() <= ENTRY_SETTLED_VELOCITY
        {
            self.entry_settled = true;
        }
        if self.mode == ShowcaseMode::Poi && self.entry_settled {
            self.orbit_drift_yaw += dt * settings.orbit_speed;
            self.dolly_phase =
                (self.dolly_phase + dt * TAU / settings.dolly_period).rem_euclid(TAU);
        }
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
        self.poi_switch_cooldown = (self.poi_switch_cooldown - dt).max(0.0);
        let mesh_target = if self.mode == ShowcaseMode::Poi
            && self
                .target
                .as_ref()
                .is_some_and(|poi| poi.anchor.part.is_some())
        {
            1.0
        } else {
            0.0
        };
        let mesh_duration = if mesh_target > self.mesh_fade {
            settings.selector_fade_in
        } else {
            settings.selector_fade_out
        };
        let mesh_change = if mesh_duration <= f32::EPSILON {
            1.0
        } else {
            dt / mesh_duration
        };
        self.mesh_fade += (mesh_target - self.mesh_fade).clamp(-mesh_change, mesh_change);
        self.selector.advance(dt, settings, self.gap_armed);
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
                orbit_drift_yaw: self.orbit_drift_yaw,
                dolly_scale: 1.0 + self.settings.dolly_depth * self.dolly_phase.sin(),
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
            focus_child: self.poi_child,
            mesh_attenuation: 1.0
                - smoothstep(self.mesh_fade) * (1.0 - self.settings.poi_attenuation),
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
        self.entry_settled = false;
        self.selector.deactivate();
        self.blend.set_target(1.0);
        self.motion.set_target(0.0);
        self.current = Some(index);
        self.target = Some(self.pois[index].clone());
        let anchor = self.pois[index].anchor;
        let center = [anchor.center.x, anchor.center.y, anchor.center.z];
        if settled {
            self.orbit_yaw.reset(0.0);
            self.orbit_pitch.reset(0.0);
            self.orbit_drift_yaw = 0.0;
            self.dolly_phase = 0.0;
            self.fov.reset(normal_fov);
            for (spring, value) in self.center.iter_mut().zip(center) {
                spring.reset(value);
            }
            self.radius.reset(anchor.radius);
            self.slide.reset(0.0);
        } else {
            self.fov.set_target(normal_fov);
            for (spring, value) in self.center.iter_mut().zip(center) {
                spring.set_target(value);
            }
            self.radius.set_target(anchor.radius);
        }
        self.set_slide_target();
        self.reseed_orbit(rng);
        self.reset_poi_child();
        self.poi_switch_cooldown = self.settings.poi_switch_gap;
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
        self.reseed_orbit(rng);
        self.reset_poi_child();
        self.poi_switch_cooldown = self.settings.poi_switch_gap;
    }

    /// Re-aims the orbit for a new POI. The target is drawn from the kick step
    /// range, but the switch only covers a step inside the switch step range,
    /// so a distant target is reached over consecutive switches.
    fn reseed_orbit(&mut self, rng: &mut Rng) {
        let settings = &self.settings;
        let drift = self.orbit_drift_yaw;
        let yaw = self.orbit_yaw.value();
        let pitch = self.orbit_pitch.value();
        let yaw_aim = signed_step(rng, settings.orbit_step_min, settings.orbit_step_max);
        let pitch_aim = signed_step(rng, settings.orbit_step_min, settings.orbit_step_max);
        let minimum = settings.poi_switch_step_min;
        let maximum = settings.poi_switch_step_max;
        let yaw_target = bounded_approach(rng, yaw + drift, yaw_aim, minimum, maximum);
        self.orbit_yaw.set_target(yaw_target - drift);
        self.orbit_pitch
            .set_target(bounded_approach(rng, pitch, pitch_aim, minimum, maximum));
    }

    fn reset_poi_child(&mut self) {
        self.poi_child = self
            .target
            .as_ref()
            .filter(|poi| poi.anchor.part.is_some() && poi.children.len() > 2)
            .and_then(|poi| poi.children.first().copied());
    }

    fn advance_poi_child(&mut self) {
        let Some(poi) = self.target.as_ref() else {
            self.poi_child = None;
            return;
        };
        if poi.children.len() <= 2 {
            self.poi_child = None;
            return;
        }
        let next = poi
            .children
            .iter()
            .position(|part| Some(*part) == self.poi_child)
            .map_or(poi.children[0], |index| {
                poi.children[(index + 1) % poi.children.len()]
            });
        self.poi_child = Some(next);
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
        self.entry_settled = false;
        self.poi_child = None;
        self.poi_switch_cooldown = 0.0;
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
    exit_triggers: u32,
    gap_elapsed: f32,
}

impl SelectorState {
    fn activate(&mut self, rng: &mut Rng, part_count: u16, settings: &ShowcaseSettings) {
        if self.selected.is_none_or(|part| part >= part_count) {
            self.selected = pick_mesh(rng, None, part_count);
        }
        self.active = true;
        self.exit_triggers = 0;
        self.gap_elapsed = 0.0;
        self.roll_color(rng, settings);
    }

    fn deactivate(&mut self) {
        self.active = false;
        self.alternate_color = false;
        self.gap_elapsed = 0.0;
    }

    fn record_trigger(&mut self, settings: &ShowcaseSettings) {
        if !self.active {
            return;
        }
        self.exit_triggers = self.exit_triggers.saturating_add(1);
        if self.exit_triggers >= settings.selector_exit_triggers {
            self.deactivate();
        }
    }

    fn select_mesh(&mut self, rng: &mut Rng, part_count: u16) {
        self.selected = pick_mesh(rng, self.selected, part_count);
    }

    fn roll_color(&mut self, rng: &mut Rng, settings: &ShowcaseSettings) {
        self.alternate_color =
            self.selected.is_some() && rng.unit() < settings.selector_color_chance;
    }

    fn advance(&mut self, dt: f32, settings: &ShowcaseSettings, gap_armed: bool) {
        if self.active {
            if gap_armed {
                self.gap_elapsed += dt;
                if self.gap_elapsed >= settings.selector_gap_timeout {
                    self.deactivate();
                }
            } else {
                self.gap_elapsed = 0.0;
            }
        } else {
            self.gap_elapsed = 0.0;
        }
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

fn signed_step(rng: &mut Rng, minimum: f32, maximum: f32) -> f32 {
    let magnitude = minimum + rng.unit() * (maximum - minimum);
    if rng.unit() < 0.5 {
        -magnitude
    } else {
        magnitude
    }
}

/// Moves `current` toward `aim` along the shortest angular path by a step
/// inside the switch step range. When the aim is farther away than the
/// maximum, the step only approaches it and later switches cover the rest.
#[must_use]
fn bounded_approach(rng: &mut Rng, current: f32, aim: f32, minimum: f32, maximum: f32) -> f32 {
    let delta = wrap_angle(aim - current);
    let magnitude = delta.abs().clamp(minimum, maximum);
    let direction = if delta < 0.0 {
        -1.0
    } else if delta > 0.0 {
        1.0
    } else if rng.unit() < 0.5 {
        // Already on the aim: the minimum step picks either side.
        -1.0
    } else {
        1.0
    };
    current + direction * magnitude
}

/// Keeps the FOV step inside the 30-120 degree range and, where possible,
/// chooses a direction with enough room for the minimum step.
fn signed_fov_step(rng: &mut Rng, current: f32, minimum: f32, maximum: f32) -> f32 {
    let magnitude = minimum + rng.unit() * (maximum - minimum);
    let positive_room = POI_FOV_MAX - current;
    let negative_room = current - POI_FOV_MIN;
    let direction = if positive_room < minimum {
        -1.0
    } else if negative_room < minimum {
        1.0
    } else if rng.unit() < 0.5 {
        -1.0
    } else {
        1.0
    };
    (current + direction * magnitude).clamp(POI_FOV_MIN, POI_FOV_MAX)
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
        let mut showcase = Showcase::new(ShowcaseSettings {
            poi_switch_gap: 0.0,
            ..ShowcaseSettings::default()
        });
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

    /// The rotation a reseed applies must stay inside the switch step range.
    fn assert_switch_step(step: f32, settings: &ShowcaseSettings) {
        let minimum = settings.poi_switch_step_min;
        let maximum = settings.poi_switch_step_max;
        assert!(
            step.abs() >= minimum - 1e-5,
            "{step} rad turns less than the switch minimum"
        );
        assert!(
            step.abs() <= maximum + 1e-5,
            "{step} rad turns more than the switch maximum"
        );
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
    fn selector_exit_needs_the_configured_trigger_count() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            selector_exit_triggers: 2,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
        let mut rng = Rng::new(11);
        showcase.kick(&mut rng);
        assert!(showcase.selector_active());
        showcase.kick(&mut rng);
        assert!(
            showcase.selector_active(),
            "one trigger is not enough with N=2"
        );
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
    fn focus_mode_can_be_requested_and_left_outside_the_snare_cycle() {
        let mut showcase = showcase();
        let mut rng = Rng::new(44);
        assert_eq!(showcase.mode(), ShowcaseMode::Normal);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        assert_eq!(showcase.mode(), ShowcaseMode::Poi);
        showcase.exit_focus();
        assert_eq!(showcase.mode(), ShowcaseMode::Normal);
        assert!(showcase.enter_focus(&mut rng, 0.75));
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
    fn snares_count_toward_selector_exit() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            selector_exit_triggers: 2,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
        let mut rng = Rng::new(32);
        showcase.kick(&mut rng);
        assert!(showcase.selector_active());

        let first = showcase.snare(&mut rng, 5, 2, 0.75);
        assert!(!first.flip);
        assert!(showcase.selector_active());

        let second = showcase.snare(&mut rng, 5, 2, 0.75);
        assert!(!second.flip, "the disabling snare also suppresses its flip");
        assert!(!showcase.selector_active());

        let third = showcase.snare(&mut rng, 5, 2, 0.75);
        assert!(third.flip, "the next snare flips normally");
    }

    #[test]
    fn gap_confidence_exits_the_selector_only_after_its_timeout() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            selector_gap_timeout: 2.0,
            selector_exit_triggers: 100,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
        let mut rng = Rng::new(8);
        showcase.kick(&mut rng);
        assert!(showcase.selector_active());
        showcase.set_gap_armed(true);
        for _ in 0..119 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.selector_active(), "under two seconds stays active");
        for _ in 0..3 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(!showcase.selector_active(), "the timeout turns it off");
    }

    #[test]
    fn clearing_gap_confidence_resets_the_selector_timeout() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            selector_gap_timeout: 2.0,
            selector_exit_triggers: 100,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
        let mut rng = Rng::new(9);
        showcase.kick(&mut rng);
        showcase.set_gap_armed(true);
        for _ in 0..90 {
            showcase.advance(1.0 / 60.0);
        }
        showcase.set_gap_armed(false);
        showcase.advance(1.0 / 60.0);
        showcase.set_gap_armed(true);
        for _ in 0..90 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(
            showcase.selector_active(),
            "the timer restarts after the gap clears"
        );
    }

    #[test]
    fn gap_entry_does_not_exit_the_selector_and_drop_does() {
        let mut showcase = showcase();
        let mut rng = Rng::new(10);
        showcase.kick(&mut rng);
        showcase.gap_entry();
        assert!(
            showcase.selector_active(),
            "gap entry alone starts the timeout instead"
        );
        showcase.drop();
        assert!(!showcase.selector_active());
    }

    #[test]
    fn exit_poi_springs_the_view_back_to_normal() {
        let mut showcase = showcase();
        let mut rng = Rng::new(13);
        showcase.enter_poi(&mut rng, 0.75);
        for _ in 0..180 {
            showcase.advance(1.0 / 60.0);
        }
        showcase.kick(&mut rng);
        for _ in 0..60 {
            showcase.advance(1.0 / 60.0);
        }
        let before = showcase.frame();
        showcase.exit_poi();
        let immediate = showcase.frame();
        assert!((immediate.focus.blend - before.focus.blend).abs() < 1e-6);
        assert!((immediate.focus.center - before.focus.center).length() < 1e-6);
        assert!((immediate.focus.orbit_yaw - before.focus.orbit_yaw).abs() < 1e-6);
        assert!((immediate.focus.fov - before.focus.fov).abs() < 1e-6);

        showcase.advance(1.0 / 60.0);
        let after = showcase.frame();
        assert!(after.focus.blend < before.focus.blend);
        for _ in 0..120 {
            showcase.advance(1.0 / 60.0);
        }
        let settled = showcase.frame();
        assert!(
            settled.focus.blend < 0.05,
            "the camera exits through its spring"
        );
        assert!(
            settled.focus.model_motion > 0.95,
            "normal model motion fades back in"
        );
    }

    #[test]
    fn poi_switch_gap_delays_the_exit_snare() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            poi_switch_gap: 5.0,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
                FocusPoi::new("magazine".to_owned(), anchor(-0.2, Some(1)), false),
            ],
            2,
        );
        let mut rng = Rng::new(61);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        for _ in 0..300 {
            showcase.advance(1.0 / 60.0);
        }
        assert_eq!(showcase.poi_switch_cooldown, 0.0);
        showcase.snare(&mut rng, 5, 1, 0.75);
        assert_eq!(showcase.poi_switches, 1);
        assert!(showcase.poi_switch_cooldown > 0.0);

        showcase.snare(&mut rng, 5, 1, 0.75);
        assert_eq!(showcase.mode(), ShowcaseMode::Poi);
        for _ in 0..300 {
            showcase.advance(1.0 / 60.0);
        }
        showcase.snare(&mut rng, 5, 1, 0.75);
        assert_eq!(showcase.mode(), ShowcaseMode::Normal);
    }

    #[test]
    fn poi_switch_gap_turns_early_snares_into_kicks() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            poi_switch_gap: 5.0,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
                FocusPoi::new("magazine".to_owned(), anchor(-0.2, Some(1)), false),
                FocusPoi::new("ejection_port".to_owned(), anchor(0.1, None), true),
            ],
            2,
        );
        let mut rng = Rng::new(60);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        for _ in 0..180 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.entry_settled);
        assert!(showcase.poi_switch_cooldown > 0.0);

        let switches = showcase.poi_switches;
        let target = showcase.target_name().map(str::to_owned);
        let yaw_before = showcase.orbit_yaw.value();
        let fov_before = showcase.fov.value();
        let outcome = showcase.snare(&mut rng, 5, 5, 0.75);
        assert!(!outcome.flip);
        assert_eq!(showcase.poi_switches, switches);
        assert_eq!(showcase.target_name(), target.as_deref());
        assert!(
            (showcase.orbit_yaw.target() - yaw_before).abs() >= showcase.settings.orbit_step_min
        );
        assert!((showcase.fov.target() - fov_before).abs() >= showcase.settings.fov_step_min);

        for _ in 0..126 {
            showcase.advance(1.0 / 60.0);
        }
        assert_eq!(showcase.poi_switch_cooldown, 0.0);
        showcase.snare(&mut rng, 5, 5, 0.75);
        assert_eq!(showcase.poi_switches, switches + 1);
    }

    #[test]
    fn normal_to_poi_entry_reseeds_orbit_within_the_step_range() {
        let mut showcase = showcase();
        let mut rng = Rng::new(59);
        showcase.orbit_yaw.reset(std::f32::consts::TAU * 2.0 + 0.7);
        showcase
            .orbit_pitch
            .reset(std::f32::consts::TAU * 3.0 + 0.2);
        showcase.orbit_drift_yaw = 2.0;
        assert!(showcase.enter_focus(&mut rng, 0.75));
        assert_eq!(showcase.orbit_yaw.value(), 0.0);
        assert_eq!(showcase.orbit_pitch.value(), 0.0);
        assert_eq!(showcase.orbit_drift_yaw, 0.0);
        let yaw_target = showcase.orbit_yaw.target();
        let pitch_target = showcase.orbit_pitch.target();
        assert_switch_step(yaw_target, showcase.settings());
        assert_switch_step(pitch_target, showcase.settings());
        let aim_minimum = showcase.settings().orbit_step_min;
        let aim_maximum = showcase.settings().orbit_step_max;
        assert!(yaw_target.abs() >= aim_minimum - 1e-6);
        assert!(yaw_target.abs() <= aim_maximum + 1e-6);
        assert!(pitch_target.abs() >= aim_minimum - 1e-6);
        assert!(pitch_target.abs() <= aim_maximum + 1e-6);
    }

    #[test]
    fn poi_switch_rotates_the_orbit_within_the_switch_step_range() {
        let step_minimum = 10.0_f32.to_radians();
        let step_maximum = 15.0_f32.to_radians();
        let mut showcase = showcase();
        showcase.settings_mut().poi_switch_step_min = step_minimum;
        showcase.settings_mut().poi_switch_step_max = step_maximum;
        let mut rng = Rng::new(58);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        showcase.orbit_yaw.reset(30.0_f32.to_radians());
        showcase.orbit_pitch.reset(0.0);
        showcase.orbit_drift_yaw = 0.0;

        let mut steps = Vec::new();
        for _ in 0..40 {
            let yaw_before = showcase.orbit_yaw.value();
            let pitch_before = showcase.orbit_pitch.value();
            showcase.select_next_poi(&mut rng);
            let yaw_step = showcase.orbit_yaw.target() - yaw_before;
            let pitch_step = showcase.orbit_pitch.target() - pitch_before;
            assert_switch_step(yaw_step, showcase.settings());
            assert_switch_step(pitch_step, showcase.settings());
            steps.push(yaw_step);
        }
        assert!(
            steps.iter().any(|step| *step < 0.0),
            "the switch turns both ways"
        );
        assert!(
            steps.iter().any(|step| *step > 0.0),
            "the switch turns both ways"
        );
        assert!(
            steps
                .iter()
                .any(|step| (step.abs() - step_minimum).abs() < 1e-5),
            "a small correction is raised to the switch minimum"
        );
        assert!(
            steps
                .iter()
                .any(|step| (step.abs() - step_maximum).abs() < 1e-5),
            "a distant target is capped at the switch maximum"
        );
    }

    #[test]
    fn poi_switch_unwinds_a_distant_orbit_over_several_switches() {
        let step_maximum = 30.0_f32.to_radians();
        let mut showcase = showcase();
        showcase.settings_mut().orbit_step_min = 0.0;
        showcase.settings_mut().orbit_step_max = 0.0;
        showcase.settings_mut().poi_switch_step_min = 10.0_f32.to_radians();
        showcase.settings_mut().poi_switch_step_max = step_maximum;
        let mut rng = Rng::new(71);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        showcase.orbit_yaw.reset(170.0_f32.to_radians());

        let mut steps = Vec::new();
        for _ in 0..8 {
            if showcase.orbit_yaw.value().abs() <= 1e-5 {
                break;
            }
            let yaw_before = showcase.orbit_yaw.value();
            showcase.select_next_poi(&mut rng);
            let yaw_target = showcase.orbit_yaw.target();
            let yaw_step = yaw_target - yaw_before;
            assert_switch_step(yaw_step, showcase.settings());
            assert!(yaw_step <= 0.0, "the orbit turns toward the target");
            steps.push(yaw_step);
            showcase.orbit_yaw.reset(yaw_target);
        }
        assert!(
            steps.len() >= 5,
            "the accumulated rotation is spread over several switches"
        );
        assert!(
            (steps[0].abs() - step_maximum).abs() < 1e-5,
            "the first switch takes only the maximum step"
        );
        assert!(
            showcase.orbit_yaw.value().abs() <= 1e-5,
            "successive switches recenter the orbit on the target"
        );
    }

    #[test]
    fn hat_sequences_the_full_opacity_child_of_a_mesh_poi() {
        let mut showcase = Showcase::new(ShowcaseSettings::default());
        showcase.set_model(
            vec![
                FocusPoi::new("body".to_owned(), anchor(0.8, Some(0)), false)
                    .with_children(vec![1, 2, 3]),
            ],
            4,
        );
        let mut rng = Rng::new(63);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        assert_eq!(showcase.frame().focus_child, Some(1));
        for expected in [Some(2), Some(3), Some(1)] {
            showcase.hat(&mut rng);
            assert_eq!(showcase.frame().focus_child, expected);
        }

        showcase.set_model(
            vec![
                FocusPoi::new("body".to_owned(), anchor(0.8, Some(0)), false)
                    .with_children(vec![1, 2]),
            ],
            3,
        );
        assert!(showcase.enter_focus(&mut rng, 0.75));
        assert_eq!(showcase.frame().focus_child, None);
        showcase.hat(&mut rng);
        assert_eq!(showcase.frame().focus_child, None);
    }

    #[test]
    fn mesh_poi_attenuates_the_other_meshes() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            selector_attenuation: 0.3,
            poi_attenuation: 0.2,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![FocusPoi::new(
                "slide".to_owned(),
                anchor(0.8, Some(0)),
                false,
            )],
            2,
        );
        let mut rng = Rng::new(57);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        for _ in 0..180 {
            showcase.advance(1.0 / 60.0);
        }
        let frame = showcase.frame();
        assert_eq!(frame.highlight_part, Some(0));
        assert!((frame.mesh_attenuation - 0.2).abs() < 1e-3);

        showcase.exit_focus();
        for _ in 0..10 {
            showcase.advance(1.0 / 60.0);
        }
        assert!((showcase.frame().mesh_attenuation - 1.0).abs() < 1e-3);

        showcase.set_model(
            vec![FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false)],
            2,
        );
        assert!(showcase.enter_focus(&mut rng, 0.75));
        for _ in 0..60 {
            showcase.advance(1.0 / 60.0);
        }
        let empty = showcase.frame();
        assert_eq!(empty.highlight_part, None);
        assert!((empty.mesh_attenuation - 1.0).abs() < 1e-6);
    }

    #[test]
    fn slow_orbit_and_dolly_start_after_the_entry_settles() {
        let mut showcase = showcase();
        let mut rng = Rng::new(55);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        for _ in 0..10 {
            showcase.advance(1.0 / 60.0);
        }
        let early = showcase.frame();
        assert!(!showcase.entry_settled);
        assert_eq!(early.focus.orbit_drift_yaw, 0.0);
        assert!((early.focus.dolly_scale - 1.0).abs() < 1e-6);

        for _ in 0..180 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.entry_settled);
        let before = showcase.frame();
        for _ in 0..60 {
            showcase.advance(1.0 / 60.0);
        }
        let after = showcase.frame();
        assert!(after.focus.orbit_drift_yaw > before.focus.orbit_drift_yaw);
        assert!(
            (after.focus.dolly_scale - 1.0).abs() > 0.01,
            "the slow dolly moves the camera in or out"
        );
    }

    #[test]
    fn kicks_cap_orbit_and_fov_step_variation() {
        let orbit_min = 20.0_f32.to_radians();
        let orbit_cap = 45.0_f32.to_radians();
        let fov_min = 10.0_f32.to_radians();
        let fov_cap = 20.0_f32.to_radians();
        let mut showcase = Showcase::new(ShowcaseSettings {
            orbit_step_min: orbit_min,
            orbit_step_max: orbit_cap,
            fov_step_min: fov_min,
            fov_step_max: fov_cap,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
        let mut rng = Rng::new(42);
        showcase.enter_poi(&mut rng, 0.75);
        for _ in 0..180 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.entry_settled);

        let mut yaw_signs = [false; 2];
        let mut fov_signs = [false; 2];
        for _ in 0..24 {
            let yaw_before = showcase.orbit_yaw.value();
            let pitch_before = showcase.orbit_pitch.value();
            let fov_before = showcase.fov.value();
            showcase.kick(&mut rng);
            let yaw_step = showcase.orbit_yaw.target() - yaw_before;
            let pitch_step = showcase.orbit_pitch.target() - pitch_before;
            let fov_step = showcase.fov.target() - fov_before;
            assert!(yaw_step.abs() >= orbit_min - 1e-6);
            assert!(pitch_step.abs() >= orbit_min - 1e-6);
            assert!(yaw_step.abs() <= orbit_cap + 1e-6);
            assert!(pitch_step.abs() <= orbit_cap + 1e-6);
            assert!(fov_step.abs() >= fov_min - 1e-6);
            assert!(fov_step.abs() <= fov_cap + 1e-6);
            assert!(showcase.fov.target() >= 30.0_f32.to_radians() - 1e-6);
            assert!(showcase.fov.target() <= 120.0_f32.to_radians() + 1e-6);
            yaw_signs[usize::from(yaw_step > 0.0)] = true;
            fov_signs[usize::from(fov_step > 0.0)] = true;
        }
        assert!(yaw_signs[0] && yaw_signs[1], "orbit steps use both signs");
        assert!(fov_signs[0] && fov_signs[1], "FOV steps use both signs");
    }

    #[test]
    fn kick_attack_feeds_initial_velocity_into_the_kick_springs() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            kick_attack: 4.0,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
        let mut rng = Rng::new(62);
        assert!(showcase.enter_focus(&mut rng, 0.75));
        for _ in 0..300 {
            showcase.advance(1.0 / 60.0);
        }
        let yaw_before = showcase.orbit_yaw.value();
        let pitch_before = showcase.orbit_pitch.value();
        let fov_before = showcase.fov.value();
        showcase.kick(&mut rng);
        assert!(
            (showcase.orbit_yaw.velocity() - (showcase.orbit_yaw.target() - yaw_before) * 4.0)
                .abs()
                < 1e-5
        );
        assert!(
            (showcase.orbit_pitch.velocity()
                - (showcase.orbit_pitch.target() - pitch_before) * 4.0)
                .abs()
                < 1e-5
        );
        assert!(
            (showcase.fov.velocity() - (showcase.fov.target() - fov_before) * 4.0).abs() < 1e-5
        );
    }

    #[test]
    fn fov_min_step_moves_inward_at_the_range_edges() {
        let fov_min = 10.0_f32.to_radians();
        let fov_max = 20.0_f32.to_radians();
        let mut showcase = Showcase::new(ShowcaseSettings {
            fov_step_min: fov_min,
            fov_step_max: fov_max,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
        let mut rng = Rng::new(43);
        showcase.enter_poi(&mut rng, 0.75);
        for _ in 0..180 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.entry_settled);

        let high = 120.0_f32.to_radians() - 1.0_f32.to_radians();
        showcase.fov.reset(high);
        showcase.kick(&mut rng);
        assert!(showcase.fov.target() < high);
        assert!(high - showcase.fov.target() >= fov_min - 1e-6);

        let low = 30.0_f32.to_radians() + 1.0_f32.to_radians();
        showcase.fov.reset(low);
        showcase.kick(&mut rng);
        assert!(showcase.fov.target() > low);
        assert!(showcase.fov.target() - low >= fov_min - 1e-6);
    }

    #[test]
    fn step_minima_must_not_exceed_the_maxima() {
        let orbit = ShowcaseSettings {
            orbit_step_min: 1.0,
            ..ShowcaseSettings::default()
        };
        assert!(orbit.validate().is_err());
        let fov = ShowcaseSettings {
            fov_step_min: 1.0,
            ..ShowcaseSettings::default()
        };
        assert!(fov.validate().is_err());
        let switch = ShowcaseSettings {
            poi_switch_step_min: std::f32::consts::PI,
            ..ShowcaseSettings::default()
        };
        assert!(switch.validate().is_err());
    }

    #[test]
    fn kicks_are_ignored_until_the_entry_spring_settles() {
        let mut showcase = showcase();
        let mut rng = Rng::new(21);
        showcase.enter_poi(&mut rng, 0.75);
        for _ in 0..10 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(!showcase.entry_settled);
        let orbit_target = showcase.orbit_yaw.target();
        let fov_target = showcase.fov.target();

        showcase.kick(&mut rng);
        assert_eq!(showcase.orbit_yaw.target(), orbit_target);
        assert_eq!(showcase.fov.target(), fov_target);

        for _ in 0..180 {
            showcase.advance(1.0 / 60.0);
        }
        assert!(showcase.entry_settled);
        showcase.kick(&mut rng);
        assert!((showcase.orbit_yaw.target() - orbit_target).abs() > 1e-6);
        assert!((showcase.fov.target() - fov_target).abs() > 1e-6);
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
        let mut showcase = Showcase::new(ShowcaseSettings {
            selector_exit_triggers: 1,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
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
    fn selector_active_suppresses_the_flip_for_that_snare() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            selector_exit_triggers: 1,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
        let mut rng = Rng::new(31);
        showcase.kick(&mut rng);
        assert!(showcase.selector_active());

        let outcome = showcase.snare(&mut rng, 5, 2, 0.75);
        assert!(!outcome.flip, "the enabling beat cannot also flip");
        assert!(!showcase.selector_active());
        assert_eq!(showcase.normal_flips(), 1);

        let outcome = showcase.snare(&mut rng, 5, 2, 0.75);
        assert!(outcome.flip, "the next beat flips normally");
        assert_eq!(showcase.normal_flips(), 2);
    }

    #[test]
    fn the_nth_snare_disables_the_selector() {
        let mut showcase = Showcase::new(ShowcaseSettings {
            selector_exit_triggers: 1,
            ..ShowcaseSettings::default()
        });
        showcase.set_model(
            vec![
                FocusPoi::new("muzzle".to_owned(), anchor(0.8, None), false),
                FocusPoi::new("slide".to_owned(), anchor(0.0, Some(0)), false),
            ],
            2,
        );
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
