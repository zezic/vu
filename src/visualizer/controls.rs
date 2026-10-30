//! Mouse controls for unattended presentation playback.

mod audio_gap;

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use vello::{
    kurbo::{Affine, Point, Rect, Stroke},
    peniko::{Color, Fill, Gradient},
    Scene,
};

use crate::{
    config::{Config, CursorVisibility, PreampDb},
    frame_schedule::Adjustment,
    text::{Fonts, Text, UiFace},
    theme::{ThemeContrast, ThemeId, Tone},
};

use super::{presentation::ScenePalette, DebugHistory, ModelMotion, ModelRotation, View};

const IDLE: Duration = Duration::from_secs(5);
const BLOCK_WIDTH: f64 = 180.0;
const BUTTON_HEIGHT: f64 = 30.0;
const LABEL_HEIGHT: f64 = 16.0;
const CELL_HEIGHT: f64 = BUTTON_HEIGHT + LABEL_HEIGHT;
const GAP: f64 = 6.0;
const ROW_GAP: f64 = 8.0;
const GROUP_GAP: f64 = 16.0;
const POPUP_OFFSET: f64 = 112.0;
const VIEW_MARGIN: f64 = 12.0;
const BOTTOM_MARGIN: f64 = 24.0;

#[derive(Clone, Copy)]
pub enum Dial {
    Theme,
    Model,
    LineWidth,
    FrameRate,
    Shutter,
    TextOpacity,
    Zoom,
}

#[derive(Clone, Copy)]
pub enum ControlAction {
    Fullscreen,
    Adjust(Dial, Adjustment),
    AutoColors,
    Contrast(ThemeContrast),
    Tone(Tone),
    ModelLock,
    Boost(PreampDb),
    MotionBlur,
    HideCursor,
    Dislike,
    SaveDefault,
    Manage,
    Restore(ThemeId),
    DeveloperMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(strum::VariantArray))]
enum Control {
    Fullscreen,
    Theme,
    AutoColors,
    Contrast,
    Tone,
    Model,
    ModelLock,
    Dislike,
    SaveDefault,
    Boost,
    MotionBlur,
    Shutter,
    LineWidth,
    FrameRate,
    Zoom,
    TextOpacity,
    HideCursor,
}

impl Control {
    fn title(self) -> &'static str {
        match self {
            Self::Theme => "COLOR THEME",
            Self::AutoColors => "AUTO COLORS",
            Self::Contrast => "ALLOW CONTRAST",
            Self::Tone => "THEME TONE",
            Self::Model => "MODEL",
            Self::ModelLock => "MODEL LOCK",
            Self::Boost => "ANALYZER BOOST",
            Self::Fullscreen => "DISPLAY",
            Self::MotionBlur => "MOTION BLUR",
            Self::Shutter => "SHUTTER",
            Self::LineWidth => "LINE WIDTH",
            Self::FrameRate => "FPS LIMIT",
            Self::Zoom => "ZOOM",
            Self::TextOpacity => "TEXT OPACITY",
            Self::HideCursor => "CURSOR",
            Self::Dislike => "THEME DISLIKE",
            Self::SaveDefault => "STARTUP THEME",
        }
    }

    fn hint(self, config: &Config) -> &'static str {
        match self {
            Self::Theme => "Browse color themes. Disliked themes are skipped.",
            Self::AutoColors => "Choose a new theme on model changes, after 10 seconds without mouse movement.",
            Self::Contrast if !config.theme_preferences.auto_colors => "Enable Auto Colors to use contrast settings.",
            Self::Contrast => "Off: keep tone. On: either tone. Always: switch dark/light on every automatic change.",
            Self::Tone => "Return to your last manually chosen dark or light theme.",
            Self::Model => "Browse models in catalog order. Manual navigation works while locked.",
            Self::ModelLock => "Keep this model during music drops. Music-driven motion and focus changes continue.",
            Self::Boost => "Adjust analyzer sensitivity. This does not change playback volume.",
            Self::Fullscreen => "Toggle fullscreen here or double-click the background. Esc leaves fullscreen. The last state is remembered.",
            Self::MotionBlur => "Toggle motion blur on the model wireframe.",
            Self::Shutter if config.model_motion == ModelMotion::Still => "Enable Motion Blur to use the model shutter.",
            Self::Shutter => "Set the model's exposure length in frame intervals.",
            Self::LineWidth => "Adjust the thickness of model wireframe lines.",
            Self::FrameRate => "Limit drawing speed, or synchronize with the display using VSync.",
            Self::Zoom => "Adjust visualizer magnification.",
            Self::TextOpacity => "Adjust infographic opacity. Set 0% to hide the presentation text.",
            Self::HideCursor => "Hide the mouse pointer when the controls fade out.",
            Self::Dislike => "Skip this theme now and exclude it from browsing and automatic colors.",
            Self::SaveDefault => "Save this theme for startup and as your preferred theme for its tone.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Domain {
    Colors,
    Scene,
    Audio,
    Rendering,
    Window,
}

impl Domain {
    fn group(self) -> &'static ControlGroup {
        GROUPS
            .iter()
            .find(|group| group.domain == self)
            .expect("every domain has a control group")
    }

    fn title(self) -> &'static str {
        match self {
            Self::Colors => "COLORS",
            Self::Scene => "SCENE",
            Self::Audio => "AUDIO",
            Self::Rendering => "RENDERING",
            Self::Window => "WINDOW",
        }
    }
}

#[derive(Clone, Copy)]
struct ControlColumn {
    top: Control,
    bottom: Option<Control>,
}

impl ControlColumn {
    const fn pair(top: Control, bottom: Control) -> Self {
        Self {
            top,
            bottom: Some(bottom),
        }
    }

    const fn single(top: Control) -> Self {
        Self { top, bottom: None }
    }

    fn cells(self) -> impl Iterator<Item = (Control, Row)> {
        std::iter::once((self.top, Row::Top))
            .chain(self.bottom.map(|control| (control, Row::Bottom)))
    }
}

struct ControlGroup {
    domain: Domain,
    columns: &'static [ControlColumn],
}

impl ControlGroup {
    fn width(&self) -> f64 {
        self.columns.len() as f64 * (BLOCK_WIDTH + GAP) - GAP
    }
}

const GROUPS: [ControlGroup; 5] = [
    ControlGroup {
        domain: Domain::Colors,
        columns: &[
            ControlColumn::pair(Control::Theme, Control::Tone),
            ControlColumn::pair(Control::AutoColors, Control::Contrast),
            ControlColumn::pair(Control::Dislike, Control::SaveDefault),
        ],
    },
    ControlGroup {
        domain: Domain::Scene,
        columns: &[ControlColumn::pair(Control::Model, Control::ModelLock)],
    },
    ControlGroup {
        domain: Domain::Audio,
        columns: &[ControlColumn::single(Control::Boost)],
    },
    ControlGroup {
        domain: Domain::Rendering,
        columns: &[
            ControlColumn::pair(Control::MotionBlur, Control::Shutter),
            ControlColumn::pair(Control::LineWidth, Control::Zoom),
            ControlColumn::pair(Control::FrameRate, Control::TextOpacity),
        ],
    },
    ControlGroup {
        domain: Domain::Window,
        columns: &[ControlColumn::pair(
            Control::Fullscreen,
            Control::HideCursor,
        )],
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Top,
    Bottom,
}

#[derive(Clone, Copy)]
struct Cell {
    control: Control,
    row: Row,
    rect: Rect,
}

impl Cell {
    fn label(self) -> Rect {
        match self.row {
            Row::Top => Rect::new(
                self.rect.x0,
                self.rect.y0,
                self.rect.x1,
                self.rect.y0 + LABEL_HEIGHT,
            ),
            Row::Bottom => Rect::new(
                self.rect.x0,
                self.rect.y1 - LABEL_HEIGHT,
                self.rect.x1,
                self.rect.y1,
            ),
        }
    }

    fn buttons(self) -> Rect {
        match self.row {
            Row::Top => Rect::new(
                self.rect.x0,
                self.rect.y0 + LABEL_HEIGHT,
                self.rect.x1,
                self.rect.y1,
            ),
            Row::Bottom => Rect::new(
                self.rect.x0,
                self.rect.y0,
                self.rect.x1,
                self.rect.y1 - LABEL_HEIGHT,
            ),
        }
    }
}

struct Choice {
    label: String,
    action: Option<ControlAction>,
    selected: bool,
}

impl Choice {
    fn new(label: impl Into<String>, action: ControlAction, selected: bool) -> Self {
        Self {
            label: label.into(),
            action: Some(action),
            selected,
        }
    }

    fn width_weight(&self) -> f64 {
        match self.action {
            Some(ControlAction::Contrast(ThemeContrast::Always)) => 1.5,
            _ => 1.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LabelStyle {
    Caption,
    Value,
    ListValue,
}

pub struct ControlState<'a> {
    pub config: &'a Config,
    pub theme: ThemeId,
    pub model: &'a str,
    pub fullscreen: bool,
    pub palette: ScenePalette,
    pub background: Color,
    pub history: &'a DebugHistory,
}

struct Canvas<'a> {
    fonts: &'a mut Fonts,
    scene: &'a mut Scene,
    scale: f64,
    palette: ScenePalette,
    background: Color,
}

impl Canvas<'_> {
    fn physical(&self, rect: Rect) -> Rect {
        Rect::new(
            rect.x0 * self.scale,
            rect.y0 * self.scale,
            rect.x1 * self.scale,
            rect.y1 * self.scale,
        )
    }

    fn fill(&mut self, rect: Rect, color: Color) {
        self.scene
            .fill(Fill::NonZero, Affine::scale(self.scale), color, None, &rect);
    }

    fn backdrop(&mut self, rect: Rect, opacity: f32) {
        let gradient = Gradient::new_linear(
            Point::new(rect.center().x, rect.y0),
            Point::new(rect.center().x, rect.y1),
        )
        .with_stops([
            (0.0, self.background.with_alpha(0.0)),
            (0.25, self.background.with_alpha(0.90 * opacity)),
            (0.55, self.background.with_alpha(0.96 * opacity)),
            (1.0, self.background.with_alpha(0.98 * opacity)),
        ]);
        self.scene.fill(
            Fill::NonZero,
            Affine::scale(self.scale),
            &gradient,
            None,
            &rect,
        );
    }

    fn rule(&mut self, rect: Rect, color: Color) {
        self.scene.stroke(
            &Stroke::new(0.5),
            Affine::scale(self.scale),
            color,
            None,
            &rect,
        );
    }
}

struct Target {
    rect: Rect,
    action: ControlAction,
}

struct Hover {
    rect: Rect,
    hint: &'static str,
}

struct Layout {
    scale: f64,
    grid: Rect,
}

impl Layout {
    fn new(view: &View) -> Self {
        let width = GROUPS.iter().map(ControlGroup::width).sum::<f64>()
            + (GROUPS.len() - 1) as f64 * GROUP_GAP;
        let height = CELL_HEIGHT * 2.0 + ROW_GAP;
        let scale = view
            .ui_scale()
            .min(view.viewport.width / (width + 32.0))
            .min(
                view.viewport.height
                    / (height
                        + Domain::Audio.group().width()
                        + POPUP_OFFSET
                        + VIEW_MARGIN
                        + BOTTOM_MARGIN),
            );
        let x = (view.viewport.width / scale - width) * 0.5;
        let y = view.viewport.height / scale - height - BOTTOM_MARGIN;
        Self {
            scale,
            grid: Rect::new(x, y, x + width, y + height),
        }
    }

    fn groups(&self) -> impl Iterator<Item = (&ControlGroup, Rect)> {
        GROUPS.iter().scan(self.grid.x0, |x, group| {
            let rect = Rect::new(*x, self.grid.y0, *x + group.width(), self.grid.y1);
            *x += group.width() + GROUP_GAP;
            Some((group, rect))
        })
    }

    fn group(&self, domain: Domain) -> Rect {
        self.groups()
            .find(|(group, _)| group.domain == domain)
            .expect("every domain belongs to the layout")
            .1
    }

    fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.groups().flat_map(|(group, rect)| {
            group
                .columns
                .iter()
                .copied()
                .enumerate()
                .flat_map(move |(column, controls)| {
                    let x = rect.x0 + column as f64 * (BLOCK_WIDTH + GAP);
                    controls.cells().map(move |(control, row)| {
                        let y = rect.y0
                            + if row == Row::Top {
                                0.0
                            } else {
                                CELL_HEIGHT + ROW_GAP
                            };
                        Cell {
                            control,
                            row,
                            rect: Rect::new(x, y, x + BLOCK_WIDTH, y + CELL_HEIGHT),
                        }
                    })
                })
        })
    }

    fn cell(&self, control: Control) -> Cell {
        self.cells()
            .find(|cell| cell.control == control)
            .expect("every control belongs to a layout group")
    }
}

pub struct Controls {
    pointer: Option<Point>,
    last_activity: Instant,
    last_motion: Instant,
    last_draw: Instant,
    opacity: f32,
    modifiers: bool,
    manager_open: bool,
    manager_offset: usize,
    manage_revealed: bool,
    targets: Vec<Target>,
    hovers: Vec<Hover>,
    bounds: Vec<Rect>,
    manager_rect: Option<Rect>,
    text: BTreeMap<(String, LabelStyle), Text>,
}

impl Controls {
    pub fn new(now: Instant) -> Self {
        Self {
            pointer: None,
            last_activity: now,
            last_motion: now,
            last_draw: now,
            opacity: 1.0,
            modifiers: false,
            manager_open: false,
            manager_offset: 0,
            manage_revealed: false,
            targets: Vec::new(),
            hovers: Vec::new(),
            bounds: Vec::new(),
            manager_rect: None,
            text: BTreeMap::new(),
        }
    }

    pub fn pointer(&mut self, pointer: Option<Point>, now: Instant) {
        if pointer.is_some() && self.pointer != pointer {
            self.last_activity = now;
            self.last_motion = now;
        }
        self.pointer = pointer;
    }

    pub fn modifiers(&mut self, held: bool, now: Instant) {
        self.modifiers = held;
        if held {
            self.last_activity = now;
        }
    }

    pub fn cooldown(&self, now: Instant) -> f32 {
        1.0 - (now
            .saturating_duration_since(self.last_motion)
            .as_secs_f32()
            / IDLE.as_secs_f32())
        .min(1.0)
    }

    pub fn cursor_visible(&self) -> bool {
        self.opacity > 0.01
    }

    pub fn activate(&mut self, now: Instant) {
        self.last_activity = now;
        self.last_draw = now;
        self.opacity = 1.0;
    }

    pub fn contains(&self, point: Point) -> bool {
        self.opacity > 0.1 && self.bounds.iter().any(|rect| rect.contains(point))
    }

    pub fn click(&mut self, point: Point, now: Instant) -> Option<ControlAction> {
        let visible = self.opacity > 0.1;
        self.last_activity = now;
        if !visible {
            return None;
        }
        let over_manager =
            self.manager_open && self.manager_rect.is_some_and(|rect| rect.contains(point));
        let action = self
            .targets
            .iter()
            .rev()
            .find(|target| {
                target.rect.contains(point)
                    && match target.action {
                        ControlAction::DeveloperMode => self.modifiers && !over_manager,
                        ControlAction::Restore(_) => self.manager_open,
                        _ => !over_manager,
                    }
            })
            .map(|target| target.action);
        if matches!(action, Some(ControlAction::Manage)) {
            self.manager_open = !self.manager_open;
            self.manager_offset = 0;
        } else if self.manager_open && !self.bounds.iter().any(|rect| rect.contains(point)) {
            self.manager_open = false;
        }
        action
    }

    pub fn scroll(&mut self, delta: f64, now: Instant) {
        self.last_activity = now;
        if self
            .manager_rect
            .is_some_and(|rect| self.pointer.is_some_and(|point| rect.contains(point)))
        {
            if delta > 0.0 {
                self.manager_offset = self.manager_offset.saturating_sub(delta.ceil() as usize);
            } else {
                self.manager_offset = self.manager_offset.saturating_add((-delta).ceil() as usize);
            }
        }
    }

    fn label(
        &mut self,
        canvas: &mut Canvas<'_>,
        rect: Rect,
        label: &str,
        style: LabelStyle,
        color: Color,
    ) {
        let text = self
            .text
            .entry((label.to_owned(), style))
            .or_insert_with(|| {
                canvas.fonts.layout_ui(
                    label,
                    match style {
                        LabelStyle::Caption => 10.0,
                        LabelStyle::Value | LabelStyle::ListValue => 13.0,
                    },
                    match style {
                        LabelStyle::Caption => UiFace::Light,
                        LabelStyle::Value | LabelStyle::ListValue => UiFace::SemiBold,
                    },
                )
            });
        let fit = ((rect.width() - 8.0) / text.width().max(1.0)).min(1.0);
        let transform = Affine::scale(canvas.scale)
            * Affine::translate((
                match style {
                    LabelStyle::Caption | LabelStyle::ListValue => rect.x0,
                    LabelStyle::Value => rect.center().x - text.width() * fit * 0.5,
                },
                rect.center().y + text.cap_height() * fit * 0.5,
            ))
            * Affine::scale(fit);
        text.draw_left(canvas.scene, transform, color);
    }

    fn button(&mut self, canvas: &mut Canvas<'_>, rect: Rect, choice: Choice, enabled: bool) {
        let hovered = self
            .pointer
            .is_some_and(|point| canvas.physical(rect).contains(point));
        let alpha = self.opacity * if enabled { 1.0 } else { 0.32 };
        let palette = canvas.palette;
        let fill = if choice.selected || hovered && enabled {
            palette.ink.with_alpha(0.14 * alpha)
        } else {
            palette.muted.with_alpha(0.08 * alpha)
        };
        let border = if hovered && enabled {
            palette.ink.with_alpha(0.98 * alpha)
        } else if choice.selected {
            palette.ink.with_alpha(0.72 * alpha)
        } else {
            palette.muted.with_alpha(0.30 * alpha)
        };
        canvas.fill(rect, fill);
        canvas.rule(rect, border);
        self.label(
            canvas,
            rect,
            &choice.label,
            LabelStyle::Value,
            palette.ink.with_alpha(0.82 * alpha),
        );
        if enabled {
            if let Some(action) = choice.action {
                self.targets.push(Target {
                    rect: canvas.physical(rect),
                    action,
                });
            }
        }
    }

    fn choices(
        control: Control,
        config: &Config,
        theme: ThemeId,
        model: &str,
        fullscreen: bool,
    ) -> Vec<Choice> {
        let dial = |setting, value: String| {
            vec![
                Choice::new(
                    "-",
                    ControlAction::Adjust(setting, Adjustment::Decrease),
                    false,
                ),
                Choice {
                    label: value,
                    action: None,
                    selected: false,
                },
                Choice::new(
                    "+",
                    ControlAction::Adjust(setting, Adjustment::Increase),
                    false,
                ),
            ]
        };
        let toggle = |checked, label: &str, action| {
            vec![Choice::new(
                format!("[{}] {label}", if checked { "x" } else { " " }),
                action,
                checked,
            )]
        };
        match control {
            Control::Theme => dial(Dial::Theme, theme.name().to_owned()),
            Control::Model => dial(Dial::Model, model.to_owned()),
            Control::LineWidth => {
                dial(Dial::LineWidth, format!("{}%", config.line_width.percent()))
            }
            Control::FrameRate => dial(Dial::FrameRate, config.frame_rate_limit.to_string()),
            Control::Zoom => dial(Dial::Zoom, format!("{:.0}%", config.zoom.factor() * 100.0)),
            Control::Shutter => dial(
                Dial::Shutter,
                format!("{:.1} frames", config.shutter_frames.frames()),
            ),
            Control::TextOpacity => dial(
                Dial::TextOpacity,
                format!("{:.0}%", config.presentation_opacity.factor() * 100.0),
            ),
            Control::AutoColors => toggle(
                config.theme_preferences.auto_colors,
                "Auto Colors",
                ControlAction::AutoColors,
            ),
            Control::ModelLock => toggle(
                config.model_rotation == ModelRotation::Locked,
                "Lock model",
                ControlAction::ModelLock,
            ),
            Control::MotionBlur => toggle(
                config.model_motion == ModelMotion::Swept,
                "Motion blur",
                ControlAction::MotionBlur,
            ),
            Control::HideCursor => toggle(
                config.cursor_visibility == CursorVisibility::AutoHide,
                "Auto-hide",
                ControlAction::HideCursor,
            ),
            Control::Contrast => [
                ("Off", ThemeContrast::Off),
                ("On", ThemeContrast::On),
                ("Always", ThemeContrast::Always),
            ]
            .into_iter()
            .map(|(label, contrast)| {
                Choice::new(
                    label,
                    ControlAction::Contrast(contrast),
                    config.theme_preferences.contrast == contrast,
                )
            })
            .collect(),
            Control::Tone => [("Dark", Tone::Dark), ("Light", Tone::Light)]
                .into_iter()
                .map(|(label, tone)| Choice {
                    label: label.to_owned(),
                    action: config
                        .theme_preferences
                        .remembered(tone)
                        .map(|_| ControlAction::Tone(tone)),
                    selected: theme.tone() == tone,
                })
                .collect(),
            Control::Boost => [-6.0, 0.0, 6.0]
                .into_iter()
                .map(|db| {
                    Choice::new(
                        format!("{db:+.0} dB"),
                        ControlAction::Boost(PreampDb::try_from(db).expect("fixed boost is valid")),
                        config.preamp_db.db() == db,
                    )
                })
                .collect(),
            Control::Fullscreen => vec![Choice::new(
                if fullscreen {
                    "Leave fullscreen"
                } else {
                    "Fullscreen"
                },
                ControlAction::Fullscreen,
                fullscreen,
            )],
            Control::Dislike => vec![Choice::new("Dislike theme", ControlAction::Dislike, false)],
            Control::SaveDefault => vec![Choice::new(
                if config.theme == theme {
                    "Saved as default"
                } else {
                    "Save as default"
                },
                ControlAction::SaveDefault,
                config.theme == theme,
            )],
        }
    }

    #[cfg(test)]
    pub(super) fn target_center(&self, matches: impl Fn(ControlAction) -> bool) -> Point {
        self.targets
            .iter()
            .rev()
            .find(|target| matches(target.action))
            .expect("the requested control is visible")
            .rect
            .center()
    }

    pub fn exclusion_bounds(&self) -> Option<Rect> {
        (self.opacity > 0.0)
            .then(|| self.bounds.iter().copied().reduce(|a, b| a.union(b)))
            .flatten()
    }

    pub fn draw(
        &mut self,
        fonts: &mut Fonts,
        scene: &mut Scene,
        view: &View,
        state: ControlState<'_>,
        now: Instant,
    ) -> bool {
        let held = self
            .pointer
            .is_some_and(|point| self.bounds.iter().any(|rect| rect.contains(point)));
        let active = held
            || self.manager_open
            || self.modifiers
            || now.saturating_duration_since(self.last_activity) < IDLE;
        let dt = now.saturating_duration_since(self.last_draw).as_secs_f32();
        self.last_draw = now;
        self.opacity = (self.opacity + if active { dt / 0.15 } else { -dt / 0.45 }).clamp(0.0, 1.0);
        self.targets.clear();
        self.hovers.clear();
        self.bounds.clear();
        self.manager_rect = None;
        if self.opacity == 0.0 {
            return false;
        }
        let config = state.config;
        let layout = Layout::new(view);
        let mut canvas = Canvas {
            fonts,
            scene,
            scale: layout.scale,
            palette: state.palette,
            background: state.background,
        };
        let panel = layout.grid.inflate(10.0, 10.0);
        let hint_rect = Rect::new(
            layout.grid.x0,
            layout.grid.y0 - 54.0,
            layout.grid.x1,
            layout.grid.y0 - 28.0,
        );
        self.bounds.push(canvas.physical(panel.union(hint_rect)));
        let backdrop = Rect::new(
            0.0,
            hint_rect.y0,
            view.viewport.width / layout.scale,
            view.viewport.height / layout.scale,
        );
        canvas.backdrop(backdrop, self.opacity);
        for (group, rect) in layout.groups() {
            let heading = Rect::new(rect.x0, rect.y0 - 20.0, rect.x1, rect.y0 - 6.0);
            self.label(
                &mut canvas,
                heading,
                group.domain.title(),
                LabelStyle::Caption,
                state.palette.muted.with_alpha(0.70 * self.opacity),
            );
        }
        for cell in layout.cells() {
            let control = cell.control;
            let rect = cell.rect;
            let enabled = match control {
                Control::Contrast => config.theme_preferences.auto_colors,
                Control::Shutter => config.model_motion == ModelMotion::Swept,
                Control::Dislike => config.theme_preferences.can_dislike(state.theme),
                _ => true,
            };
            self.hovers.push(Hover {
                rect: canvas.physical(rect),
                hint: if control == Control::Dislike && !enabled {
                    "At least one theme must remain available."
                } else {
                    control.hint(config)
                },
            });
            if control == Control::Dislike
                && self
                    .pointer
                    .is_some_and(|point| canvas.physical(rect).contains(point))
            {
                self.manage_revealed = true;
            }
            let mut title_rect = cell.label();
            if control == Control::AutoColors && config.theme_preferences.auto_colors {
                title_rect.x1 = title_rect.x0 + 64.0;
            }
            self.label(
                &mut canvas,
                title_rect,
                control.title(),
                LabelStyle::Caption,
                state
                    .palette
                    .ink
                    .with_alpha(0.75 * self.opacity * if enabled { 1.0 } else { 0.4 }),
            );
            let choices =
                Self::choices(control, config, state.theme, state.model, state.fullscreen);
            let dial = matches!(
                control,
                Control::Theme
                    | Control::Model
                    | Control::LineWidth
                    | Control::FrameRate
                    | Control::Shutter
                    | Control::TextOpacity
                    | Control::Zoom
            );
            let count = choices.len();
            let weight: f64 = choices.iter().map(Choice::width_weight).sum();
            let buttons = cell.buttons();
            let available_width = buttons.width() - 3.0 * (count - 1) as f64;
            let mut next_x = buttons.x0;
            for (i, choice) in choices.into_iter().enumerate() {
                let (x, width) = if dial {
                    match i {
                        0 => (buttons.x0, BUTTON_HEIGHT),
                        1 => (
                            buttons.x0 + BUTTON_HEIGHT + 3.0,
                            buttons.width() - 2.0 * (BUTTON_HEIGHT + 3.0),
                        ),
                        _ => (buttons.x1 - BUTTON_HEIGHT, BUTTON_HEIGHT),
                    }
                } else {
                    let width = available_width * choice.width_weight() / weight;
                    let x = next_x;
                    next_x += width + 3.0;
                    (x, width)
                };
                let button = Rect::new(x, buttons.y0, x + width, buttons.y1);
                let available = choice.action.is_some();
                if control == Control::Tone && !available {
                    self.hovers.push(Hover {
                        rect: canvas.physical(button),
                        hint: "All themes of this tone are disliked. Restore one in Manage.",
                    });
                }
                self.button(&mut canvas, button, choice, enabled && (available || dial));
            }
            if control == Control::AutoColors && config.theme_preferences.auto_colors {
                self.draw_cooldown(&mut canvas, cell.label(), now);
            }
        }
        let boost = layout.cell(Control::Boost).rect;
        if !self.manager_open
            && self
                .pointer
                .is_some_and(|point| canvas.physical(boost).contains(point))
        {
            let audio = layout.group(Domain::Audio);
            let bottom = layout.grid.y0 - POPUP_OFFSET;
            let graph = Rect::new(audio.x0, bottom - audio.width(), audio.x1, bottom);
            let physical = canvas.physical(graph);
            self.bounds.push(physical);
            self.hovers.push(Hover {
                rect: physical,
                hint: "Live analyzer levels and the drop gap gate after the selected boost.",
            });
            let opacity = self.opacity;
            audio_gap::draw(self, &mut canvas, graph, &state, opacity);
        }
        let has_disliked = config.theme_preferences.disliked().len() > 0;
        let manage_visible = has_disliked && (self.manage_revealed || self.manager_open);
        let logical_width = view.viewport.width / layout.scale;
        let manage_beside = layout.grid.x1 + 106.0 <= logical_width - 12.0;
        if manage_visible {
            let (x, y) = if manage_beside {
                (layout.grid.x1 + 16.0, layout.grid.y1 - CELL_HEIGHT + 20.0)
            } else {
                (layout.grid.x1 - 90.0, layout.grid.y0 - 96.0)
            };
            let manage = Rect::new(x, y, x + 90.0, y + 32.0);
            // The joined rectangle keeps the hover reveal alive through the
            // gap in either the beside-grid or above-grid placement.
            self.bounds
                .push(canvas.physical(panel.union(manage.inflate(8.0, 8.0))));
            self.button(
                &mut canvas,
                manage,
                Choice::new(
                    if self.manager_open {
                        "Close list"
                    } else {
                        "Manage"
                    },
                    ControlAction::Manage,
                    self.manager_open,
                ),
                true,
            );
            self.hovers.push(Hover {
                rect: canvas.physical(manage),
                hint: "Restore themes to make them available again.",
            });
        } else if !has_disliked {
            self.manager_open = false;
        }
        if self.manager_open {
            self.draw_manager(&mut canvas, &layout, config, view);
        }
        if self.modifiers {
            let width = 260.0_f64.min(logical_width - 24.0);
            let x = if manage_visible && !manage_beside {
                layout.grid.x0
            } else {
                logical_width * 0.5 - width * 0.5
            };
            let y = layout.grid.y0 - 96.0;
            let rect = Rect::new(x, y, x + width, y + 34.0);
            self.bounds.push(canvas.physical(rect));
            self.button(
                &mut canvas,
                rect,
                Choice::new(
                    "Developer: exit presentation",
                    ControlAction::DeveloperMode,
                    false,
                ),
                true,
            );
            self.hovers.push(Hover {
                rect: canvas.physical(rect),
                hint: "Leave presentation mode and open the developer controls.",
            });
        }
        let hint = self
            .pointer
            .and_then(|point| {
                self.hovers
                    .iter()
                    .rev()
                    .find(|hover| hover.rect.contains(point))
            })
            .map_or(
                "Hover a control for help. Double-click the background to toggle fullscreen.",
                |hover| hover.hint,
            );
        self.label(
            &mut canvas,
            hint_rect,
            hint,
            LabelStyle::Caption,
            state.palette.ink.with_alpha(0.82 * self.opacity),
        );
        if !held
            && !self.manager_open
            && !self
                .pointer
                .is_some_and(|point| self.bounds.iter().any(|rect| rect.contains(point)))
        {
            self.manage_revealed = false;
        }
        true
    }

    fn draw_cooldown(&mut self, canvas: &mut Canvas<'_>, rect: Rect, now: Instant) {
        let track = Rect::new(
            rect.x1 - 64.0,
            rect.center().y - 2.0,
            rect.x1 - 24.0,
            rect.center().y + 2.0,
        );
        canvas.fill(track, canvas.palette.muted.with_alpha(0.30 * self.opacity));
        let remaining = self.cooldown(now);
        if remaining > 0.0 {
            canvas.fill(
                Rect::new(
                    track.x0,
                    track.y0,
                    track.x0 + track.width() * f64::from(remaining),
                    track.y1,
                ),
                canvas.palette.ink.with_alpha(0.72 * self.opacity),
            );
        }
        let label = format!("{}s", (remaining * IDLE.as_secs_f32()).ceil() as u32);
        self.label(
            canvas,
            Rect::new(rect.x1 - 23.0, rect.y0, rect.x1, rect.y1),
            &label,
            LabelStyle::Caption,
            canvas.palette.muted.with_alpha(0.70 * self.opacity),
        );
    }

    fn draw_manager(
        &mut self,
        canvas: &mut Canvas<'_>,
        layout: &Layout,
        config: &Config,
        view: &View,
    ) {
        let width = 270.0_f64.min(view.viewport.width / canvas.scale - 24.0);
        let beside = layout.grid.x1 + width + 30.0 < view.viewport.width / canvas.scale;
        let x = if beside {
            layout.grid.x1 + 18.0
        } else {
            layout.grid.center().x - width * 0.5
        };
        let above_space = layout.grid.y0 - POPUP_OFFSET - VIEW_MARGIN;
        let modal = !beside && above_space < 68.0;
        let available = if beside {
            layout.grid.y1 - 52.0
        } else if modal {
            layout.grid.height() - 8.0
        } else {
            above_space
        };
        let rows = ((available.min(338.0) - 38.0) / 30.0).floor().max(1.0) as usize;
        let count = config.theme_preferences.disliked().len();
        let rows = rows.min(count);
        self.manager_offset = self.manager_offset.min(count.saturating_sub(rows));
        let height = rows as f64 * 30.0 + 38.0;
        let y = if beside {
            (layout.grid.y1 - height - 40.0).max(12.0)
        } else if modal {
            layout.grid.center().y - height * 0.5
        } else {
            layout.grid.y0 - POPUP_OFFSET - height
        };
        let rect = Rect::new(x, y, x + width, y + height);
        let physical = canvas.physical(rect);
        self.bounds.push(physical);
        self.manager_rect = Some(physical);
        self.hovers.push(Hover {
            rect: physical,
            hint: "Scroll to browse disliked themes. Click x to restore a theme.",
        });
        canvas.fill(rect, canvas.background.with_alpha(0.96 * self.opacity));
        self.label(
            canvas,
            Rect::new(x + 8.0, y + 5.0, x + width - 8.0, y + 30.0),
            &format!("DISLIKED THEMES ({count})"),
            LabelStyle::Caption,
            canvas.palette.ink.with_alpha(0.75 * self.opacity),
        );
        for (index, theme) in config
            .theme_preferences
            .disliked()
            .skip(self.manager_offset)
            .take(rows)
            .enumerate()
        {
            let row = Rect::new(
                x + 8.0,
                y + 34.0 + index as f64 * 30.0,
                x + width - 8.0,
                y + 61.0 + index as f64 * 30.0,
            );
            self.label(
                canvas,
                Rect::new(row.x0, row.y0, row.x1 - 32.0, row.y1),
                theme.name(),
                LabelStyle::ListValue,
                canvas.palette.ink.with_alpha(0.82 * self.opacity),
            );
            self.button(
                canvas,
                Rect::new(row.x1 - 28.0, row.y0, row.x1, row.y1),
                Choice::new("x", ControlAction::Restore(theme), false),
                true,
            );
        }
        if count > rows {
            let track = Rect::new(rect.x1 - 4.0, rect.y0 + 34.0, rect.x1 - 2.0, rect.y1 - 5.0);
            let thumb = track.height() * rows as f64 / count as f64;
            let top = track.y0
                + (track.height() - thumb) * self.manager_offset as f64 / (count - rows) as f64;
            canvas.fill(
                Rect::new(track.x0, top, track.x1, top + thumb),
                canvas.palette.ink.with_alpha(0.72 * self.opacity),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::VariantArray;

    fn draw(
        controls: &mut Controls,
        fonts: &mut Fonts,
        view: &View,
        config: &Config,
        now: Instant,
    ) -> bool {
        let history = DebugHistory::default();
        controls.draw(
            fonts,
            &mut Scene::new(),
            view,
            ControlState {
                config,
                theme: config.theme,
                model: "Model",
                fullscreen: false,
                palette: ScenePalette::choose(
                    &super::super::Palette::from_theme(config.theme),
                    crate::theme::Theme::new(config.theme).colors,
                    &mut super::super::Rng::new(42),
                ),
                background: crate::theme::Theme::new(config.theme).colors.background,
                history: &history,
            },
            now,
        )
    }

    fn target(controls: &Controls, predicate: impl Fn(ControlAction) -> bool) -> Rect {
        controls
            .targets
            .iter()
            .find(|target| predicate(target.action))
            .expect("control is visible")
            .rect
    }

    fn config_with_disliked(count: usize) -> Config {
        let mut config = Config::default();
        for theme in ThemeId::all()
            .filter(|theme| *theme != config.theme)
            .take(count)
        {
            assert!(config.theme_preferences.dislike(theme));
        }
        config
    }

    #[test]
    fn motion_cooldown_expires_and_pointer_departure_does_not_restart_it() {
        let now = Instant::now();
        let mut controls = Controls::new(now);
        controls.pointer(Some(Point::new(2.0, 3.0)), now + Duration::from_secs(2));
        controls.pointer(None, now + Duration::from_secs(7));
        assert_eq!(controls.cooldown(now + Duration::from_secs(7)), 0.5);
        assert_eq!(controls.cooldown(now + Duration::from_secs(12)), 0.0);
    }

    #[test]
    fn idle_controls_hide_and_modifier_or_mouse_restores_them() -> anyhow::Result<()> {
        let now = Instant::now();
        let mut controls = Controls::new(now);
        let mut fonts = Fonts::new()?;
        let view = View::fit(vello::kurbo::Size::new(1280.0, 720.0));
        let config = Config::default();
        assert!(!draw(
            &mut controls,
            &mut fonts,
            &view,
            &config,
            now + Duration::from_secs(11)
        ));
        assert!(!controls.cursor_visible());
        controls.modifiers(true, now + Duration::from_secs(12));
        assert!(draw(
            &mut controls,
            &mut fonts,
            &view,
            &config,
            now + Duration::from_secs(12)
        ));
        assert!(controls
            .targets
            .iter()
            .any(|target| matches!(target.action, ControlAction::DeveloperMode)));
        Ok(())
    }

    #[test]
    fn manage_remains_reachable_across_the_gap_at_regular_and_high_dpi_sizes() -> anyhow::Result<()>
    {
        let now = Instant::now();
        let mut fonts = Fonts::new()?;
        let config = config_with_disliked(2);
        for (width, height, dpi) in [
            (800.0, 600.0, 1.0),
            (1280.0, 720.0, 2.0),
            (360.0, 740.0, 1.0),
        ] {
            let mut view = View::fit(vello::kurbo::Size::new(width, height));
            view.set_ui_scale(dpi);
            let mut controls = Controls::new(now);
            draw(&mut controls, &mut fonts, &view, &config, now);
            let dislike =
                target(&controls, |action| matches!(action, ControlAction::Dislike)).center();
            controls.pointer(Some(dislike), now);
            draw(&mut controls, &mut fonts, &view, &config, now);
            let manage =
                target(&controls, |action| matches!(action, ControlAction::Manage)).center();
            // Follow the actual pointer route through the gap; every redraw
            // must keep the destination visible until it can be clicked.
            for step in 1..=20 {
                let fraction = step as f64 / 20.0;
                let point = Point::new(
                    dislike.x + (manage.x - dislike.x) * fraction,
                    dislike.y + (manage.y - dislike.y) * fraction,
                );
                controls.pointer(Some(point), now);
                draw(&mut controls, &mut fonts, &view, &config, now);
                target(&controls, |action| matches!(action, ControlAction::Manage));
            }
            assert!(matches!(
                controls.click(manage, now),
                Some(ControlAction::Manage)
            ));
            assert!(controls.manager_open);
        }
        Ok(())
    }

    #[test]
    fn releasing_modifiers_invalidates_the_developer_button_before_redraw() -> anyhow::Result<()> {
        let now = Instant::now();
        let mut fonts = Fonts::new()?;
        let config = config_with_disliked(20);
        for (width, height, dpi) in [(1920.0, 1080.0, 1.0), (1280.0, 720.0, 2.0)] {
            let mut controls = Controls::new(now);
            let mut view = View::fit(vello::kurbo::Size::new(width, height));
            view.set_ui_scale(dpi);
            controls.manager_open = true;
            controls.modifiers(true, now);
            draw(&mut controls, &mut fonts, &view, &config, now);
            let developer = target(&controls, |action| {
                matches!(action, ControlAction::DeveloperMode)
            });
            let manage = target(&controls, |action| matches!(action, ControlAction::Manage));
            let manager = controls.manager_rect.expect("manager is visible");
            assert_eq!(developer.intersect(manager).area(), 0.0);
            assert_eq!(manage.intersect(manager).area(), 0.0);
            controls.modifiers(false, now);
            assert!(!matches!(
                controls.click(developer.center(), now),
                Some(ControlAction::DeveloperMode)
            ));
            assert!(matches!(
                controls.click(manage.center(), now),
                Some(ControlAction::Manage)
            ));
            assert!(!controls.manager_open);
        }
        Ok(())
    }

    #[test]
    fn manager_scroll_clamps_and_its_background_blocks_covered_controls() -> anyhow::Result<()> {
        let now = Instant::now();
        let mut controls = Controls::new(now);
        let mut fonts = Fonts::new()?;
        let view = View::fit(vello::kurbo::Size::new(800.0, 600.0));
        let config = config_with_disliked(40);
        controls.manager_open = true;
        draw(&mut controls, &mut fonts, &view, &config, now);
        let manager = controls.manager_rect.expect("manager is visible");
        controls.pointer(Some(manager.center()), now);
        controls.scroll(-10000.0, now);
        draw(&mut controls, &mut fonts, &view, &config, now);
        let rows = controls
            .targets
            .iter()
            .filter(|target| matches!(target.action, ControlAction::Restore(_)))
            .count();
        assert_eq!(controls.manager_offset, 40 - rows);
        let blank = Point::new(manager.x0 + 2.0, manager.center().y);
        assert!(controls.click(blank, now).is_none());
        controls.scroll(10000.0, now);
        draw(&mut controls, &mut fonts, &view, &config, now);
        assert_eq!(controls.manager_offset, 0);
        Ok(())
    }

    #[test]
    fn hovering_the_hint_keeps_controls_and_cursor_visible() -> anyhow::Result<()> {
        let now = Instant::now();
        let mut controls = Controls::new(now);
        let mut fonts = Fonts::new()?;
        let view = View::fit(vello::kurbo::Size::new(800.0, 600.0));
        let config = Config::default();
        draw(&mut controls, &mut fonts, &view, &config, now);
        let layout = Layout::new(&view);
        let hint = Point::new(
            layout.grid.center().x * layout.scale,
            (layout.grid.y0 - 30.0) * layout.scale,
        );
        controls.pointer(Some(hint), now);
        assert!(draw(
            &mut controls,
            &mut fonts,
            &view,
            &config,
            now + Duration::from_secs(20)
        ));
        assert!(controls.cursor_visible());
        assert!(controls
            .exclusion_bounds()
            .expect("controls are visible")
            .contains(hint));
        Ok(())
    }

    #[test]
    fn controls_keep_aligned_equal_width_blocks_with_square_dials_and_outward_labels(
    ) -> anyhow::Result<()> {
        let now = Instant::now();
        let mut fonts = Fonts::new()?;
        let mut config = Config::default();
        config.theme_preferences.auto_colors = true;
        for (width, height, dpi) in [
            (1920.0, 1080.0, 1.0),
            (1280.0, 720.0, 2.0),
            (800.0, 600.0, 1.0),
        ] {
            let mut view = View::fit(vello::kurbo::Size::new(width, height));
            view.set_ui_scale(dpi);
            let mut controls = Controls::new(now);
            draw(&mut controls, &mut fonts, &view, &config, now);
            let rows: std::collections::BTreeSet<_> = controls
                .targets
                .iter()
                .map(|target| target.rect.y0.to_bits())
                .collect();
            assert_eq!(rows.len(), 2);
            for target in &controls.targets {
                assert!(target.rect.x0 >= 0.0 && target.rect.x1 <= width);
                assert!(target.rect.y0 >= 0.0 && target.rect.y1 <= height);
                if matches!(target.action, ControlAction::Adjust(_, _)) {
                    assert!((target.rect.width() - target.rect.height()).abs() < 1e-6);
                }
            }
            let off = target(&controls, |action| {
                matches!(action, ControlAction::Contrast(ThemeContrast::Off))
            });
            let on = target(&controls, |action| {
                matches!(action, ControlAction::Contrast(ThemeContrast::On))
            });
            let always = target(&controls, |action| {
                matches!(action, ControlAction::Contrast(ThemeContrast::Always))
            });
            assert!((off.width() - on.width()).abs() < 1e-6);
            assert!(always.width() > on.width());
            let layout = Layout::new(&view);
            for cell in layout.cells() {
                assert!((cell.rect.width() - BLOCK_WIDTH).abs() < 1e-6);
                let buttons = cell.buttons();
                let label = cell.label();
                if cell.row == Row::Top {
                    assert!(label.y1 <= buttons.y0);
                } else {
                    assert!(label.y0 >= buttons.y1);
                }
            }
            for group in &GROUPS {
                for column in group.columns {
                    if let Some(bottom) = column.bottom {
                        assert_eq!(layout.cell(column.top).rect.x0, layout.cell(bottom).rect.x0);
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn grouped_layout_contains_each_control_once() {
        let view = View::fit(vello::kurbo::Size::new(1920.0, 1080.0));
        let layout = Layout::new(&view);
        assert_eq!(layout.cells().count(), Control::VARIANTS.len());
        for control in Control::VARIANTS {
            assert_eq!(
                layout
                    .cells()
                    .filter(|cell| cell.control == *control)
                    .count(),
                1,
                "{control:?} must have exactly one place in the controls"
            );
        }
        assert_eq!(layout.cell(Control::Boost).row, Row::Top);
        assert!(!layout
            .cells()
            .any(|cell| cell.row == Row::Bottom
                && cell.rect.x0 == layout.cell(Control::Boost).rect.x0));
    }

    #[test]
    fn boost_graph_tracks_hover_fits_the_viewport_and_excludes_the_selector() -> anyhow::Result<()>
    {
        let now = Instant::now();
        let mut fonts = Fonts::new()?;
        let config = config_with_disliked(2);
        for (width, height, dpi) in [
            (1920.0, 1080.0, 1.0),
            (1280.0, 720.0, 2.0),
            (1900.0, 170.0, 1.0),
        ] {
            let mut controls = Controls::new(now);
            let mut view = View::fit(vello::kurbo::Size::new(width, height));
            view.set_ui_scale(dpi);
            let layout = Layout::new(&view);
            let audio = layout.group(Domain::Audio);
            let expected_width = audio.width() * layout.scale;
            let graph_bounds = |controls: &Controls| {
                controls.bounds.iter().copied().find(|rect| {
                    (rect.width() - expected_width).abs() < 1e-6
                        && (rect.height() - expected_width).abs() < 1e-6
                })
            };
            draw(&mut controls, &mut fonts, &view, &config, now);
            assert!(graph_bounds(&controls).is_none());
            let boost = target(&controls, |action| {
                matches!(action, ControlAction::Boost(_))
            })
            .center();
            controls.pointer(Some(boost), now);
            draw(&mut controls, &mut fonts, &view, &config, now);
            let graph = graph_bounds(&controls).expect("boost hover opens its audio graph");
            assert_eq!(graph.x0, audio.x0 * layout.scale);
            assert_eq!(graph.x1, audio.x1 * layout.scale);
            assert!(graph.x0 >= 0.0 && graph.x1 <= width);
            assert!(graph.y0 >= 0.0 && graph.y1 <= height);
            let exclusion = controls.exclusion_bounds().expect("controls are visible");
            assert_eq!(graph.intersect(exclusion), graph);
            controls.modifiers(true, now);
            draw(&mut controls, &mut fonts, &view, &config, now);
            let developer = target(&controls, |action| {
                matches!(action, ControlAction::DeveloperMode)
            });
            assert_eq!(developer.intersect(graph).area(), 0.0);
            controls.manager_open = true;
            draw(&mut controls, &mut fonts, &view, &config, now);
            assert!(graph_bounds(&controls).is_none());
            controls.manager_open = false;
            controls.pointer(Some(Point::new(0.0, 0.0)), now);
            draw(&mut controls, &mut fonts, &view, &config, now);
            assert!(graph_bounds(&controls).is_none());
        }
        Ok(())
    }
}
