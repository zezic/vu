//! Normal-mode presentation: the model name always leads, a rotating subset of
//! real-world fields supplies the support structure, and one authored fact is
//! typed somewhere along the edges.

use std::collections::BTreeMap;

use vello::{
    kurbo::{Affine, Line, Point, Rect},
    peniko::{Color, Fill},
    Scene,
};

use crate::{
    text::{BlockAlign, Fonts, UiFace},
    weapon::{RealWorld, Weapon},
};

use super::super::{Rng, Visualizer};
use super::{
    color::ScenePalette,
    decorations::Decoration,
    stage::Stage,
    text::{AnimatedText, Backdrop, CharEffect, PaintStyle, TextPhase},
};

/// Chance that one numeric field uses the spinner on its first appearance in a
/// scene.
const SPINNER_CHANCE: f32 = 0.7;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SpecField {
    Capacity,
    Mass,
    OverallLength,
    BarrelLength,
    Price,
    Cartridge,
    Action,
    FireMode,
    Configuration,
    Caveat,
}

impl SpecField {
    const PRIMARY: [Self; 8] = [
        Self::Capacity,
        Self::Mass,
        Self::OverallLength,
        Self::BarrelLength,
        Self::Price,
        Self::Cartridge,
        Self::Action,
        Self::FireMode,
    ];
    const NOTES: [Self; 2] = [Self::Configuration, Self::Caveat];

    fn label(self) -> &'static str {
        match self {
            Self::Capacity => "CAPACITY",
            Self::Mass => "MASS",
            Self::OverallLength => "LENGTH",
            Self::BarrelLength => "BARREL",
            Self::Price => "PRICE",
            Self::Cartridge => "CARTRIDGE",
            Self::Action => "ACTION",
            Self::FireMode => "FIRE MODE",
            Self::Configuration => "CONFIG NOTE",
            Self::Caveat => "SOURCE NOTE",
        }
    }

    fn is_numeric(self) -> bool {
        matches!(
            self,
            Self::Capacity | Self::Mass | Self::OverallLength | Self::BarrelLength | Self::Price
        )
    }

    fn numeric_value(self, real: &RealWorld) -> Option<f64> {
        match self {
            Self::Capacity => real.capacity_rounds.map(f64::from),
            Self::Mass => real.mass_kg,
            Self::OverallLength => real.overall_length_mm,
            Self::BarrelLength => real.barrel_length_mm,
            Self::Price => real.price_usd,
            _ => None,
        }
    }

    fn rail_range(self) -> Option<(f64, f64)> {
        match self {
            Self::Capacity => Some((0.0, 100.0)),
            Self::Mass => Some((0.0, 10.0)),
            Self::OverallLength => Some((0.0, 1500.0)),
            Self::BarrelLength => Some((0.0, 1000.0)),
            Self::Price => Some((0.0, 5000.0)),
            _ => None,
        }
    }

    fn value(self, real: &RealWorld) -> Option<String> {
        match self {
            Self::Capacity => real.capacity_rounds.map(|value| format!("{value} RND")),
            Self::Mass => real.mass_kg.map(|value| format!("{value:.1} KG")),
            Self::OverallLength => real.overall_length_mm.map(|value| format!("{value:.0} MM")),
            Self::BarrelLength => real.barrel_length_mm.map(|value| {
                if (value - value.round()).abs() < 0.05 {
                    format!("{value:.0} MM")
                } else {
                    format!("{value:.1} MM")
                }
            }),
            Self::Price => real.price_usd.map(|value| format!("${value:.0} USD")),
            Self::Cartridge => real.cartridge.clone(),
            Self::Action => real.action.clone(),
            Self::FireMode => real.fire_mode.clone(),
            Self::Configuration => {
                (!real.configuration_note.is_empty()).then(|| real.configuration_note.clone())
            }
            Self::Caveat => {
                (!real.measurement_caveat.is_empty()).then(|| real.measurement_caveat.clone())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NormalTemplate {
    Left,
    Right,
    Split,
    Rotated,
}

impl NormalTemplate {
    fn random(rng: &mut Rng) -> Self {
        match rng.range(4) {
            0 => Self::Left,
            1 => Self::Right,
            2 => Self::Split,
            _ => Self::Rotated,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WrapPolicy {
    Column,
}

#[derive(Clone, Copy, Debug)]
struct RailSpec {
    fraction: f32,
    hollow: bool,
    minimum: f64,
    maximum: f64,
}

#[derive(Clone, Debug)]
struct PlannedBlock {
    role: Role,
    text: String,
    size: f64,
    face: UiFace,
    align: BlockAlign,
    line_height: f32,
    wrap: Option<WrapPolicy>,
    paint: PaintStyle,
    backdrop: Option<Backdrop>,
    enter_effect: CharEffect,
    exit_effect: CharEffect,
    odometer: bool,
    rail: Option<RailSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Name,
    Tag,
    SpecLabel,
    SpecValue,
    Note,
    Fact,
}

#[derive(Clone, Debug)]
struct SpecPlan {
    label: PlannedBlock,
    value: PlannedBlock,
}

#[derive(Clone, Debug)]
struct NormalPlan {
    template: NormalTemplate,
    name: PlannedBlock,
    tag: PlannedBlock,
    specs: Vec<SpecPlan>,
    note: Option<PlannedBlock>,
    fact: Option<PlannedBlock>,
}

pub struct NormalScene {
    model_id: Option<String>,
    plan: Option<NormalPlan>,
    items: Vec<TextItem>,
    ammo: Option<AmmoTally>,
    ammo_suppressed: bool,
    fact_hidden: bool,
    layout_unit: f64,
    layout_size: (f64, f64),
    recent: BTreeMap<SpecField, u32>,
}

struct AmmoTally {
    capacity: u32,
    icon: Vec<[f32; 4]>,
    grid: Rect,
    bounds: Rect,
    columns: usize,
    cell: f64,
    caption: AnimatedText,
    strokes: Option<AmmoStrokes>,
}

struct AmmoStrokes {
    accent: Color,
    muted: Color,
    scene: Scene,
}

impl Default for NormalScene {
    fn default() -> Self {
        Self {
            model_id: None,
            plan: None,
            items: Vec::new(),
            ammo: None,
            ammo_suppressed: false,
            fact_hidden: false,
            layout_unit: f64::NAN,
            layout_size: (0.0, 0.0),
            recent: BTreeMap::new(),
        }
    }
}

impl NormalScene {
    pub fn update(
        &mut self,
        dt: f32,
        model_id: Option<&str>,
        display_name: &str,
        weapon: Option<&Weapon>,
        visualizer: &Visualizer,
        fonts: &mut Fonts,
        stage: &Stage,
        rng: &mut Rng,
    ) {
        let changed = self.model_id.as_deref() != model_id || self.plan.is_none();
        if changed {
            self.model_id = model_id.map(str::to_owned);
            self.plan = Some(build_plan(display_name, weapon, rng, &mut self.recent));
            self.items.clear();
            self.layout(fonts, stage, rng, true);
            self.ammo = build_ammo(weapon, visualizer, self.plan.as_ref(), stage, fonts, rng);
            self.ammo_suppressed = false;
        }
        if (self.layout_unit - stage.unit).abs() > f64::EPSILON
            || (self.layout_size.0 - stage.width).abs() > f64::EPSILON
            || (self.layout_size.1 - stage.height).abs() > f64::EPSILON
        {
            self.layout(fonts, stage, rng, false);
            self.ammo = build_ammo(weapon, visualizer, self.plan.as_ref(), stage, fonts, rng);
            self.ammo_suppressed = false;
        }
        for item in &mut self.items {
            item.advance(dt);
        }
    }

    fn layout(&mut self, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng, entrance: bool) {
        let Some(plan) = self.plan.clone() else {
            return;
        };
        self.layout_unit = stage.unit;
        self.layout_size = (stage.width, stage.height);
        self.items.clear();
        let rail = stage.px(48.0);
        let column_width = stage.column_width();
        let column_wrap = Some((column_width - rail) / stage.unit);
        let gap = stage.px(9.0);
        let small_gap = stage.px(4.0);

        let (main, opposite) = match plan.template {
            NormalTemplate::Left | NormalTemplate::Split | NormalTemplate::Rotated => {
                (stage.left, stage.right)
            }
            NormalTemplate::Right => (stage.right, stage.left),
        };

        let mut cursor_x = main.x0 + rail;
        let mut cursor_y = main.y0;
        let mut rotated_tag = None;
        if plan.template == NormalTemplate::Rotated {
            let tag = self.make_item(
                &plan.tag,
                fonts,
                stage,
                rng,
                column_wrap,
                Point::new(main.x0, main.y1),
            );
            let tag_bounds = tag.bounds(stage);
            rotated_tag = Some(tag);
            cursor_x += tag_bounds.height() + small_gap + rail;
        } else {
            let tag = self.make_item(&plan.tag, fonts, stage, rng, column_wrap, Point::ORIGIN);
            let tag_bounds = tag.bounds(stage);
            cursor_y += tag_bounds.height() + small_gap;
            self.items.push(TextItem {
                origin: Point::new(cursor_x, main.y0),
                ..tag
            });
        }

        let name = self.make_item(
            &plan.name,
            fonts,
            stage,
            rng,
            column_wrap,
            Point::new(cursor_x, cursor_y),
        );
        let name_height = name.bounds(stage).height();
        self.items.push(name);
        cursor_y += name_height + gap;

        let spec_column = match plan.template {
            NormalTemplate::Split | NormalTemplate::Rotated => opposite,
            _ => main,
        };
        let spec_x = spec_column.x0 + rail;
        let mut spec_y = match plan.template {
            NormalTemplate::Left | NormalTemplate::Right => cursor_y,
            NormalTemplate::Split | NormalTemplate::Rotated => spec_column.y0,
        };
        for spec in &plan.specs {
            let label = self.make_item(
                &spec.label,
                fonts,
                stage,
                rng,
                column_wrap,
                Point::new(spec_x, spec_y),
            );
            let label_height = label.bounds(stage).height();
            self.items.push(label);
            spec_y += label_height + small_gap;
            let value = self.make_item(
                &spec.value,
                fonts,
                stage,
                rng,
                column_wrap,
                Point::new(spec_x, spec_y),
            );
            let value_height = value.bounds(stage).height();
            self.items.push(value);
            spec_y += value_height + gap;
            if spec_y > spec_column.y1 {
                break;
            }
        }

        if let Some(note) = &plan.note {
            let note_item = self.make_item(note, fonts, stage, rng, column_wrap, Point::ORIGIN);
            let note_height = note_item.bounds(stage).height();
            let (note_x, note_y, fits) = if plan.specs.is_empty() {
                (main.x0 + rail, cursor_y, cursor_y + note_height <= main.y1)
            } else {
                match plan.template {
                    NormalTemplate::Left | NormalTemplate::Right => (
                        opposite.x0 + rail,
                        opposite.y0,
                        opposite.y0 + note_height + gap < opposite.y1,
                    ),
                    NormalTemplate::Split | NormalTemplate::Rotated => (spec_x, spec_y, false),
                }
            };
            if fits {
                self.items.push(TextItem {
                    origin: Point::new(note_x, note_y),
                    ..note_item
                });
            }
        }

        if let Some(fact) = &plan.fact {
            let fact_item = self.make_item(fact, fonts, stage, rng, column_wrap, Point::ORIGIN);
            let fact_height = fact_item.bounds(stage).height();
            let fact_x = match plan.template {
                NormalTemplate::Split => main.x0 + rail,
                NormalTemplate::Rotated if !plan.specs.is_empty() => main.x0 + rail,
                _ => opposite.x0 + rail,
            };
            let fact_y = (opposite.y1 - fact_height - stage.px(18.0)).max(opposite.y0);
            self.items.push(TextItem {
                origin: Point::new(fact_x, fact_y),
                ..fact_item
            });
        }
        if let Some(tag) = rotated_tag {
            self.items.push(tag);
        }
        if entrance {
            for item in &mut self.items {
                item.start(rng);
            }
        } else {
            for item in &mut self.items {
                item.set_visible();
            }
        }
    }

    fn make_item(
        &self,
        block: &PlannedBlock,
        fonts: &mut Fonts,
        stage: &Stage,
        rng: &mut Rng,
        column_wrap: Option<f64>,
        origin: Point,
    ) -> TextItem {
        let wrap = match block.wrap {
            None => None,
            Some(WrapPolicy::Column) => column_wrap,
        };
        let mut text = AnimatedText::new(
            block.text.clone(),
            block.size,
            block.face,
            block.align,
            wrap,
            block.line_height,
            block.paint,
        );
        if let Some(backdrop) = block.backdrop {
            text = text.with_backdrop(backdrop);
        }
        if block.enter_effect == CharEffect::Typewriter {
            text = text.with_caret(true);
        }
        text = text.with_odometer(block.odometer);
        text.layout(fonts, stage, rng);
        let line_height = text
            .plan()
            .map_or(block.size * 1.1, |plan| plan.line_height / stage.unit);
        let decoration = if matches!(block.role, Role::SpecValue | Role::Note | Role::Fact) {
            Decoration::random(rng, line_height, 42.0)
        } else {
            None
        };
        let rotation = if block.role == Role::Tag
            && self
                .plan
                .as_ref()
                .is_some_and(|plan| plan.template == NormalTemplate::Rotated)
        {
            -std::f64::consts::FRAC_PI_2
        } else {
            0.0
        };
        let rail_labels = block.rail.map(|rail| {
            let mut minimum = AnimatedText::new(
                format!("{:.0}", rail.minimum),
                9.0,
                UiFace::Light,
                BlockAlign::Left,
                None,
                1.0,
                PaintStyle::Fill,
            );
            let mut maximum = AnimatedText::new(
                format!("{:.0}", rail.maximum),
                9.0,
                UiFace::Light,
                BlockAlign::Right,
                None,
                1.0,
                PaintStyle::Fill,
            );
            minimum.layout(fonts, stage, rng);
            maximum.layout(fonts, stage, rng);
            minimum.set_visible();
            maximum.set_visible();
            [minimum, maximum]
        });
        TextItem {
            text,
            origin,
            rotation,
            role: block.role,
            enter_effect: block.enter_effect,
            exit_effect: block.exit_effect,
            decoration,
            rail: block.rail,
            rail_labels,
            suppressed: false,
            started: false,
        }
    }

    /// Hides only the parts that collide with an external infographic block.
    pub fn suppress_colliding(&mut self, stage: &Stage, occluders: &[Rect], rng: &mut Rng) {
        for item in &mut self.items {
            let collides = item.bounds(stage).intersect_rects(occluders)
                || (self.fact_hidden && item.role == Role::Fact);
            if collides {
                item.suppress(rng);
            } else {
                item.restore(rng);
            }
        }
        self.ammo_suppressed = self
            .ammo
            .as_ref()
            .is_some_and(|ammo| ammo.bounds.intersect_rects(occluders));
    }

    /// A random visible text block, used to place occasional trigger-driven
    /// micro-elements.
    pub fn random_text_rect(&self, stage: &Stage, rng: &mut Rng) -> Option<Rect> {
        let visible: Vec<Rect> = self
            .items
            .iter()
            .filter(|item| !item.text.is_hidden())
            .map(|item| item.bounds(stage))
            .collect();
        if visible.is_empty() {
            None
        } else {
            Some(visible[rng.range(visible.len())])
        }
    }

    /// The POI fact card replaces the model fact ticker while a POI beat shows
    /// a fact, so the same authored sentence is not repeated on screen.
    pub fn set_fact_hidden(&mut self, hidden: bool, rng: &mut Rng) {
        if self.fact_hidden == hidden {
            return;
        }
        self.fact_hidden = hidden;
        for item in &mut self.items {
            if item.role != Role::Fact {
                continue;
            }
            if hidden {
                item.suppress(rng);
            } else {
                item.restore(rng);
            }
        }
    }

    pub fn draw(
        &mut self,
        scene: &mut Scene,
        stage: &Stage,
        palette: ScenePalette,
        alpha: f32,
        time: f32,
    ) {
        for item in &self.items {
            item.draw(scene, stage, palette, alpha, time);
        }
        if !self.ammo_suppressed {
            if let Some(ammo) = &mut self.ammo {
                draw_ammo(scene, stage, palette, ammo, alpha);
            }
        }
    }
}

struct TextItem {
    text: AnimatedText,
    origin: Point,
    rotation: f64,
    role: Role,
    enter_effect: CharEffect,
    exit_effect: CharEffect,
    decoration: Option<Decoration>,
    rail: Option<RailSpec>,
    rail_labels: Option<[AnimatedText; 2]>,
    suppressed: bool,
    started: bool,
}

impl TextItem {
    fn start(&mut self, rng: &mut Rng) {
        if self.started {
            return;
        }
        self.started = true;
        self.text.enter(self.enter_effect, rng);
    }

    fn set_visible(&mut self) {
        self.started = true;
        self.text.set_visible();
    }

    fn advance(&mut self, dt: f32) {
        if self.started {
            self.text.advance(dt);
        }
    }

    fn suppress(&mut self, rng: &mut Rng) {
        if self.suppressed {
            return;
        }
        self.suppressed = true;
        if !self.text.is_hidden() {
            self.text.leave(self.exit_effect, rng);
        }
    }

    fn restore(&mut self, rng: &mut Rng) {
        if !self.suppressed {
            return;
        }
        self.suppressed = false;
        if matches!(self.text.phase(), TextPhase::Hidden | TextPhase::Disappear) {
            // The spinner belongs to the item's first appearance in a scene.
            self.text.disable_odometer();
            self.text.enter(self.enter_effect, rng);
        }
    }

    fn transform(&self) -> Affine {
        Affine::translate((self.origin.x, self.origin.y)) * Affine::rotate(self.rotation)
    }

    fn bounds(&self, stage: &Stage) -> Rect {
        let mut bounds = self.text.bounds(stage);
        if let Some(decoration) = &self.decoration {
            let extra = decoration.width(stage) + stage.px(6.0);
            bounds = Rect::new(
                bounds.x0 - extra,
                bounds.y0,
                bounds.x1,
                bounds.y1.max(bounds.y0 + decoration.height(stage)),
            );
        }
        if self.rail.is_some() {
            bounds = Rect::new(bounds.x0, bounds.y0, bounds.x1, bounds.y1 + stage.px(8.0));
        }
        let transform = self.transform();
        let points = [
            transform * Point::new(bounds.x0, bounds.y0),
            transform * Point::new(bounds.x1, bounds.y0),
            transform * Point::new(bounds.x1, bounds.y1),
            transform * Point::new(bounds.x0, bounds.y1),
        ];
        let mut rect = Rect::new(points[0].x, points[0].y, points[0].x, points[0].y);
        for point in &points[1..] {
            rect = rect.union(Rect::new(point.x, point.y, point.x, point.y));
        }
        rect
    }

    fn draw(&self, scene: &mut Scene, stage: &Stage, palette: ScenePalette, alpha: f32, time: f32) {
        if self.text.is_hidden() {
            return;
        }
        if let Some(decoration) = &self.decoration {
            decoration.draw(
                scene,
                stage,
                palette,
                Point::new(self.origin.x, self.origin.y),
                alpha,
            );
        }
        self.text
            .draw(scene, self.transform(), stage, palette, alpha, time);
        if let Some(rail) = self.rail {
            draw_rail(scene, stage, palette, self, rail, alpha);
        }
    }
}

trait RectExt {
    fn intersect_rects(&self, others: &[Rect]) -> bool;
}

impl RectExt for Rect {
    fn intersect_rects(&self, others: &[Rect]) -> bool {
        others.iter().any(|other| {
            self.x0 < other.x1 && self.x1 > other.x0 && self.y0 < other.y1 && self.y1 > other.y0
        })
    }
}

fn draw_rail(
    scene: &mut Scene,
    stage: &Stage,
    palette: ScenePalette,
    item: &TextItem,
    rail: RailSpec,
    alpha: f32,
) {
    let Some(plan) = item.text.plan() else {
        return;
    };
    let width = plan.max_width;
    let x0 = item.origin.x;
    let x1 = x0 + width;
    let y = item.origin.y + item.text.bounds(stage).height() + stage.px(5.0);
    let line_width = (stage.unit * 0.9).max(0.6);
    scene.stroke(
        &vello::kurbo::Stroke::new(line_width),
        Affine::IDENTITY,
        palette.muted.with_alpha(0.55 * alpha),
        None,
        &Line::new(Point::new(x0, y), Point::new(x1, y)),
    );
    for step in 0..=4 {
        let x = x0 + width * f64::from(step) / 4.0;
        let tick = if step == 0 || step == 4 {
            stage.px(5.0)
        } else {
            stage.px(3.0)
        };
        scene.stroke(
            &vello::kurbo::Stroke::new(line_width),
            Affine::IDENTITY,
            palette.muted.with_alpha(0.5 * alpha),
            None,
            &Line::new(Point::new(x, y - tick), Point::new(x, y + tick)),
        );
    }
    if let Some(labels) = &item.rail_labels {
        let label_y = y + stage.px(3.0);
        labels[0].draw(
            scene,
            Affine::translate((x0, label_y)),
            stage,
            palette,
            0.55 * alpha,
            0.0,
        );
        let max_bounds = labels[1].bounds(stage);
        labels[1].draw(
            scene,
            Affine::translate((x1 - max_bounds.width(), label_y)),
            stage,
            palette,
            0.55 * alpha,
            0.0,
        );
    }
    let marker_x = x0 + width * f64::from(rail.fraction);
    if rail.hollow {
        scene.stroke(
            &vello::kurbo::Stroke::new(line_width),
            Affine::IDENTITY,
            palette.muted.with_alpha(0.5 * alpha),
            None,
            &vello::kurbo::Circle::new(Point::new(marker_x, y), stage.px(4.0)),
        );
    } else {
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            palette.accent.with_alpha(0.9 * alpha),
            None,
            &Rect::new(
                marker_x - stage.px(2.0),
                y - stage.px(5.0),
                marker_x + stage.px(2.0),
                y + stage.px(5.0),
            ),
        );
    }
}

fn build_ammo(
    weapon: Option<&Weapon>,
    visualizer: &Visualizer,
    plan: Option<&NormalPlan>,
    stage: &Stage,
    fonts: &mut Fonts,
    rng: &mut Rng,
) -> Option<AmmoTally> {
    let capacity = weapon?.specs.real_world.capacity_rounds?;
    if capacity == 0 {
        return None;
    }
    let icon = visualizer.magazine_ammo_icon()?;
    let template = plan?.template;
    let rail = stage.px(48.0);
    let side = match template {
        NormalTemplate::Left => stage.left,
        NormalTemplate::Right | NormalTemplate::Split => stage.right,
        NormalTemplate::Rotated if plan.is_some_and(|plan| plan.specs.is_empty()) => stage.left,
        NormalTemplate::Rotated => stage.right,
    };
    let available = (side.width() - rail).clamp(stage.px(120.0), stage.px(300.0));
    let columns = (capacity as f64).sqrt().ceil() as usize;
    let columns = columns.clamp(4, 12);
    let rows = (capacity as usize).div_ceil(columns).max(1);
    let cell = (available / columns as f64)
        .min(stage.px(18.0))
        .max(stage.px(6.0));
    let grid_width = columns as f64 * cell;
    let grid_height = rows as f64 * cell;
    let x = side.x0 + rail;
    let bottom = stage.height - stage.margin - stage.px(10.0);
    let grid_y = bottom - grid_height;
    let grid = Rect::new(x, grid_y, x + grid_width, grid_y + grid_height);
    let mut caption = AnimatedText::new(
        format!("MAGAZINE / {capacity} RND"),
        11.5,
        UiFace::Light,
        BlockAlign::Left,
        None,
        1.1,
        PaintStyle::Fill,
    );
    caption.layout(fonts, stage, rng);
    caption.set_visible();
    let caption_height = caption.bounds(stage).height();
    let caption_width = caption.bounds(stage).width();
    let caption_y = grid_y - caption_height - stage.px(3.0);
    let bounds = Rect::new(x, caption_y, x + grid_width.max(caption_width), bottom);
    Some(AmmoTally {
        capacity,
        icon,
        grid,
        bounds,
        columns,
        cell,
        caption,
        strokes: None,
    })
}

fn draw_ammo(
    scene: &mut Scene,
    stage: &Stage,
    palette: ScenePalette,
    ammo: &mut AmmoTally,
    alpha: f32,
) {
    let caption_origin = Point::new(ammo.grid.x0, ammo.bounds.y0);
    ammo.caption.draw(
        scene,
        Affine::translate((caption_origin.x, caption_origin.y)),
        stage,
        palette,
        0.65 * alpha,
        0.0,
    );
    let accent = palette.accent.with_alpha(0.7 * alpha);
    let muted = palette.muted.with_alpha(0.55 * alpha);
    let cached = match ammo.strokes.take() {
        Some(cached) if cached.accent == accent && cached.muted == muted => cached,
        previous => {
            let mut cached = previous.unwrap_or_else(|| AmmoStrokes {
                accent,
                muted,
                scene: Scene::new(),
            });
            cached.accent = accent;
            cached.muted = muted;
            cached.scene.reset();
            let line_width = (ammo.cell * 0.06).max(0.6);
            let scale = ammo.cell * 0.82;
            for index in 0..ammo.capacity as usize {
                let column = index % ammo.columns;
                let row = index / ammo.columns;
                let center = Point::new(
                    ammo.grid.x0 + (column as f64 + 0.5) * ammo.cell,
                    ammo.grid.y0 + (row as f64 + 0.5) * ammo.cell,
                );
                let color = if index % 10 == 0 { accent } else { muted };
                for line in &ammo.icon {
                    let a = Point::new(
                        center.x + f64::from(line[0]) * scale,
                        center.y + f64::from(line[1]) * scale,
                    );
                    let b = Point::new(
                        center.x + f64::from(line[2]) * scale,
                        center.y + f64::from(line[3]) * scale,
                    );
                    cached.scene.stroke(
                        &vello::kurbo::Stroke::new(line_width),
                        Affine::IDENTITY,
                        color,
                        None,
                        &Line::new(a, b),
                    );
                }
            }
            cached
        }
    };
    scene.append(&cached.scene, None);
    ammo.strokes = Some(cached);
}

fn build_plan(
    display_name: &str,
    weapon: Option<&Weapon>,
    rng: &mut Rng,
    recent: &mut BTreeMap<SpecField, u32>,
) -> NormalPlan {
    let template = NormalTemplate::random(rng);
    let fact = weapon.and_then(|weapon| {
        (!weapon.interesting_facts.is_empty())
            .then(|| weapon.interesting_facts[rng.range(weapon.interesting_facts.len())].clone())
    });
    let name_size = if display_name.len() > 19 { 72.0 } else { 96.0 };
    let name_paint = match rng.range(3) {
        0 => PaintStyle::Stroke,
        1 => PaintStyle::Hole,
        _ => PaintStyle::Fill,
    };
    let name = PlannedBlock {
        role: Role::Name,
        text: display_name.to_uppercase(),
        size: name_size,
        face: UiFace::SemiBold,
        align: BlockAlign::Left,
        line_height: 0.92,
        wrap: Some(WrapPolicy::Column),
        paint: name_paint,
        backdrop: (name_paint == PaintStyle::Hole).then(|| Backdrop::random(rng, name_size)),
        enter_effect: CharEffect::random(rng),
        exit_effect: CharEffect::random(rng),
        odometer: false,
        rail: None,
    };
    let tag_text = match weapon {
        Some(weapon) => {
            let variant = weapon.asset_variant.as_deref().unwrap_or("STANDARD");
            format!("{} / {}", weapon.category.label(), variant.to_uppercase())
        }
        None => "NO CATALOG ENTRY".to_owned(),
    };
    let tag = PlannedBlock {
        role: Role::Tag,
        text: tag_text,
        size: 14.0,
        face: UiFace::Light,
        align: BlockAlign::Left,
        line_height: 1.1,
        wrap: None,
        paint: PaintStyle::Fill,
        backdrop: None,
        enter_effect: CharEffect::random(rng),
        exit_effect: CharEffect::random(rng),
        odometer: false,
        rail: None,
    };

    let mut specs = Vec::new();
    let mut note = None;
    if let Some(weapon) = weapon {
        let available: Vec<SpecField> = SpecField::PRIMARY
            .into_iter()
            .filter(|field| field.value(&weapon.specs.real_world).is_some())
            .collect();
        let notes: Vec<SpecField> = SpecField::NOTES
            .into_iter()
            .filter(|field| field.value(&weapon.specs.real_world).is_some())
            .collect();
        let count = (3 + rng.range(3)).min(available.len());
        let mut chosen = Vec::new();
        let mut pool = available.clone();
        for _ in 0..count {
            if pool.is_empty() {
                break;
            }
            let total: u32 = pool
                .iter()
                .map(|field| 1 + recent.get(field).copied().unwrap_or(0))
                .sum();
            let mut roll = rng.range(total as usize) as u32;
            let mut selected = 0;
            for (index, field) in pool.iter().enumerate() {
                let weight = 1 + recent.get(field).copied().unwrap_or(0);
                if roll < weight {
                    selected = index;
                    break;
                }
                roll -= weight;
            }
            let field = pool.remove(selected);
            *recent.entry(field).or_default() += 1;
            chosen.push(field);
        }
        if !chosen.is_empty() && !chosen.iter().any(|field| field.is_numeric()) {
            if let Some(field) = available.iter().find(|field| field.is_numeric()) {
                chosen.pop();
                chosen.push(*field);
                *recent.entry(*field).or_default() += 1;
            }
        }
        let hero = chosen
            .iter()
            .position(|field| field.is_numeric())
            .unwrap_or(0);
        for (index, field) in chosen.iter().copied().enumerate() {
            let Some(value) = field.value(&weapon.specs.real_world) else {
                continue;
            };
            let numeric = field.is_numeric();
            let label_paint = PaintStyle::Fill;
            let odometer = numeric && rng.unit() < SPINNER_CHANCE;
            let mut value_paint = if numeric && index == hero {
                match rng.range(3) {
                    0 => PaintStyle::Stroke,
                    1 => PaintStyle::Hole,
                    _ => PaintStyle::Fill,
                }
            } else {
                PaintStyle::Fill
            };
            if odometer {
                value_paint = PaintStyle::Fill;
            }
            let rail = if numeric && index == hero && rng.unit() < 0.75 {
                field
                    .numeric_value(&weapon.specs.real_world)
                    .zip(field.rail_range())
                    .map(|(value, (minimum, maximum))| RailSpec {
                        fraction: ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0) as f32,
                        hollow: false,
                        minimum,
                        maximum,
                    })
            } else {
                None
            };
            let label = PlannedBlock {
                role: Role::SpecLabel,
                text: field.label().to_owned(),
                size: 13.0,
                face: UiFace::Light,
                align: BlockAlign::Left,
                line_height: 1.05,
                wrap: Some(WrapPolicy::Column),
                paint: label_paint,
                backdrop: None,
                enter_effect: CharEffect::random(rng),
                exit_effect: CharEffect::random(rng),
                odometer: false,
                rail: None,
            };
            let value_size = if numeric && index == hero {
                66.0
            } else if numeric {
                42.0
            } else {
                24.0
            };
            let value = PlannedBlock {
                role: Role::SpecValue,
                text: value,
                size: value_size,
                face: UiFace::SemiBold,
                align: BlockAlign::Left,
                line_height: 1.02,
                wrap: Some(WrapPolicy::Column),
                paint: value_paint,
                backdrop: (value_paint == PaintStyle::Hole)
                    .then(|| Backdrop::random(rng, value_size)),
                enter_effect: CharEffect::random(rng),
                exit_effect: CharEffect::random(rng),
                odometer,
                rail,
            };
            specs.push(SpecPlan { label, value });
        }
        if weapon.specs.real_world.price_usd.is_none() && rng.unit() < 0.2 {
            let label = PlannedBlock {
                role: Role::SpecLabel,
                text: "PRICE".to_owned(),
                size: 13.0,
                face: UiFace::Light,
                align: BlockAlign::Left,
                line_height: 1.05,
                wrap: Some(WrapPolicy::Column),
                paint: PaintStyle::Fill,
                backdrop: None,
                enter_effect: CharEffect::random(rng),
                exit_effect: CharEffect::random(rng),
                odometer: false,
                rail: None,
            };
            let value = PlannedBlock {
                role: Role::SpecValue,
                text: "NO DATA".to_owned(),
                size: 24.0,
                face: UiFace::Light,
                align: BlockAlign::Left,
                line_height: 1.05,
                wrap: Some(WrapPolicy::Column),
                paint: PaintStyle::Fill,
                backdrop: None,
                enter_effect: CharEffect::random(rng),
                exit_effect: CharEffect::random(rng),
                odometer: false,
                rail: Some(RailSpec {
                    fraction: 0.0,
                    hollow: true,
                    minimum: 0.0,
                    maximum: 5000.0,
                }),
            };
            specs.push(SpecPlan { label, value });
        }
        let note_wanted = chosen.is_empty() || (chosen.len() < 5 && rng.unit() < 0.28);
        if note_wanted && !notes.is_empty() {
            let field = notes[rng.range(notes.len())];
            if let Some(value) = field.value(&weapon.specs.real_world) {
                note = Some(PlannedBlock {
                    role: Role::Note,
                    text: format!("{}: {}", field.label(), value),
                    size: 15.5,
                    face: UiFace::Light,
                    align: BlockAlign::Left,
                    line_height: 1.16,
                    wrap: Some(WrapPolicy::Column),
                    paint: PaintStyle::Fill,
                    backdrop: None,
                    enter_effect: CharEffect::random(rng),
                    exit_effect: CharEffect::Fade,
                    odometer: false,
                    rail: None,
                });
            }
        }
    }
    let fact = fact.map(|text| PlannedBlock {
        role: Role::Fact,
        text,
        size: 20.0,
        face: UiFace::Light,
        align: BlockAlign::Left,
        line_height: 1.2,
        wrap: Some(WrapPolicy::Column),
        paint: PaintStyle::Fill,
        backdrop: None,
        enter_effect: CharEffect::Typewriter,
        exit_effect: CharEffect::Fade,
        odometer: false,
        rail: None,
    });
    NormalPlan {
        template,
        name,
        tag,
        specs,
        note,
        fact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello::kurbo::Size;

    #[test]
    fn cached_ammunition_matches_fresh_draws_after_ink_changes() {
        let fixture = || AmmoTally {
            capacity: 2,
            icon: vec![[-0.5, -0.5, 0.5, 0.5], [-0.5, 0.5, 0.5, -0.5]],
            grid: Rect::new(0.0, 0.0, 20.0, 10.0),
            bounds: Rect::new(0.0, 0.0, 20.0, 10.0),
            columns: 2,
            cell: 10.0,
            caption: AnimatedText::new(
                "",
                12.0,
                UiFace::Light,
                BlockAlign::Left,
                None,
                1.0,
                PaintStyle::Fill,
            ),
            strokes: None,
        };
        let stage = Stage::new(Size::new(1920.0, 1080.0));
        let initial = ScenePalette {
            accent: Color::from_rgb8(240, 30, 20),
            muted: Color::from_rgb8(70, 80, 90),
            ..ScenePalette::placeholder()
        };
        let muted = ScenePalette {
            muted: Color::from_rgb8(20, 180, 80),
            ..initial
        };
        let accent = ScenePalette {
            accent: Color::from_rgb8(30, 40, 240),
            ..muted
        };
        let mut cached = fixture();
        for (palette, alpha) in [
            (initial, 1.0),
            (initial, 1.0),
            (muted, 1.0),
            (accent, 1.0),
            (accent, 0.55),
        ] {
            let mut actual = Scene::new();
            draw_ammo(&mut actual, &stage, palette, &mut cached, alpha);
            let mut fresh = Scene::new();
            draw_ammo(&mut fresh, &stage, palette, &mut fixture(), alpha);
            assert!(!actual.encoding().draw_data.is_empty());
            assert_eq!(actual.encoding().draw_data, fresh.encoding().draw_data);
            assert!(actual.encoding().path_tags == fresh.encoding().path_tags);
            assert_eq!(actual.encoding().path_data, fresh.encoding().path_data);
        }
    }

    #[test]
    fn a_restored_item_does_not_spin_again() -> anyhow::Result<()> {
        let stage = Stage::new(Size::new(1920.0, 1080.0));
        let mut fonts = Fonts::new()?;
        let mut rng = Rng::new(7);
        let mut text = AnimatedText::new(
            "4299 MM",
            64.0,
            UiFace::SemiBold,
            BlockAlign::Left,
            None,
            1.02,
            PaintStyle::Fill,
        )
        .with_odometer(true);
        text.layout(&mut fonts, &stage, &mut rng);
        let mut item = TextItem {
            text,
            origin: Point::ORIGIN,
            rotation: 0.0,
            role: Role::SpecValue,
            enter_effect: CharEffect::Fade,
            exit_effect: CharEffect::Fade,
            decoration: None,
            rail: None,
            rail_labels: None,
            suppressed: false,
            started: false,
        };

        item.start(&mut rng);
        assert!(item.text.has_odometer(), "the first appearance spins");

        for cycle in 0..2 {
            item.suppress(&mut rng);
            item.restore(&mut rng);
            assert!(
                !item.text.has_odometer(),
                "reappearance {cycle} in the same scene must not spin"
            );
        }
        Ok(())
    }
}
