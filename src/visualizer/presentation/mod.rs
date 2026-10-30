//! Presentation mode: viewport-relative typography and infographics drawn over
//! the wireframe while the debug HUD and its hotkeys are suppressed.

mod color;
mod decorations;
mod normal;
mod poi;
mod selector;
mod stage;
mod text;

pub use color::ScenePalette;
pub use normal::NormalScene;
pub use poi::PoiScene;
pub use selector::SelectorOverlay;
pub use stage::Stage;

use vello::{kurbo::Point, Scene};

use crate::{
    render::{PlateMaskImage, RasterAsset},
    text::Fonts,
    theme::ThemeColors,
    visualizer::Palette,
};

use super::{showcase::ShowcaseMode, Rng, TriggerCounts, Visualizer};

use self::text::{AnimatedText, PaintStyle};

const HINT: &str = "F  EXIT     A / S  THEME     [ ]  OPACITY";

pub struct Presentation {
    active: bool,
    time: f32,
    stage: Option<Stage>,
    rng: Rng,
    normal: NormalScene,
    selector: SelectorOverlay,
    poi: PoiScene,
    hint: AnimatedText,
    colors: ScenePalette,
    colors_ready: bool,
    model_id: Option<String>,
    poi_key: Option<String>,
    last_triggers: Option<TriggerCounts>,
    poi_mode: bool,
    focus_blend: f32,
    micro: Vec<MicroFx>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MicroKind {
    Tick,
    Cross,
    Bracket,
    Scan,
}

struct MicroFx {
    center: Point,
    age: f32,
    life: f32,
    kind: MicroKind,
}

impl Default for Presentation {
    fn default() -> Self {
        Self::new()
    }
}

impl Presentation {
    pub fn new() -> Self {
        Self {
            active: false,
            time: 0.0,
            stage: None,
            rng: Rng::new(0x7072_6573_656e_7421),
            normal: NormalScene::default(),
            selector: SelectorOverlay::default(),
            poi: PoiScene::default(),
            hint: AnimatedText::new(
                HINT,
                12.0,
                crate::text::UiFace::Light,
                crate::text::BlockAlign::Left,
                None,
                1.0,
                PaintStyle::Fill,
            ),
            colors: ScenePalette::placeholder(),
            colors_ready: false,
            model_id: None,
            poi_key: None,
            last_triggers: None,
            poi_mode: false,
            focus_blend: 0.0,
            micro: Vec::new(),
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        if active {
            self.last_triggers = None;
        } else {
            self.colors_ready = false;
            self.model_id = None;
            self.poi_key = None;
            self.last_triggers = None;
            self.selector.hide();
            self.poi.hide();
            self.micro.clear();
        }
    }

    pub fn stage(&self) -> Option<Stage> {
        self.stage
    }

    /// Advance the presentation scene. Layout is rebuilt only when the stage
    /// size or the model changes.
    pub fn update(
        &mut self,
        dt: f32,
        viewport: (f32, f32),
        visualizer: &Visualizer,
        fonts: &mut Fonts,
        palette: &Palette,
        colors: ThemeColors,
    ) {
        self.time += dt;
        let stage = Stage::new(vello::kurbo::Size::new(
            f64::from(viewport.0),
            f64::from(viewport.1),
        ));
        if !self
            .stage
            .is_some_and(|current| same_stage(&current, &stage))
        {
            self.stage = Some(stage);
            self.hint.layout(fonts, &stage, &mut self.rng);
            self.hint.set_visible();
        }
        if !self.active {
            return;
        }
        let model_id = visualizer.model_id();
        let mode = visualizer.showcase_mode();
        self.poi_mode = mode == ShowcaseMode::Poi;
        self.focus_blend = visualizer.showcase_focus_blend();
        let target = if mode == ShowcaseMode::Poi {
            visualizer.showcase_target_id()
        } else {
            None
        };
        if !self.colors_ready
            || self.model_id.as_deref() != model_id
            || self.poi_key.as_deref() != target
        {
            self.model_id = model_id.map(str::to_owned);
            self.poi_key = target.map(str::to_owned);
            self.colors = ScenePalette::choose(palette, colors, &mut self.rng);
            self.colors_ready = true;
        }
        self.colors.refresh(palette, colors);
        let counts = visualizer.trigger_counts();
        let previous = self.last_triggers;
        let hat_trigger = previous.is_some_and(|last| counts.hat > last.hat);
        let any_micro_trigger = previous.is_some_and(|last| {
            counts.kick > last.kick || counts.snare > last.snare || counts.drop > last.drop
        });
        self.last_triggers = Some(counts);
        let display_name = visualizer.weapon().map_or_else(
            || visualizer.scene_name(),
            |weapon| weapon.display_name.as_str(),
        );
        self.normal.update(
            dt,
            model_id,
            display_name,
            visualizer.weapon(),
            visualizer,
            fonts,
            &stage,
            &mut self.rng,
        );
        let selector_active = mode == ShowcaseMode::Normal && visualizer.showcase_selector_active();
        self.selector.update(
            dt,
            selector_active,
            visualizer.showcase_selected_part(),
            visualizer,
            fonts,
            &stage,
            &mut self.rng,
        );
        self.poi.update(
            dt,
            model_id,
            target,
            visualizer.weapon(),
            visualizer.comedy_spec(),
            visualizer,
            fonts,
            &stage,
            &mut self.rng,
            hat_trigger,
        );
        let mut occluders = self.selector.occluders(&stage);
        occluders.extend(self.poi.occluders(&stage));
        self.normal
            .set_fact_hidden(self.poi.shows_fact(), &mut self.rng);
        self.normal
            .suppress_colliding(&stage, &occluders, &mut self.rng);
        if any_micro_trigger && self.rng.unit() < 0.14 {
            self.spawn_micro(&stage);
        }
        for fx in &mut self.micro {
            fx.age += dt;
        }
        self.micro.retain(|fx| fx.age < fx.life);
    }

    fn spawn_micro(&mut self, stage: &Stage) {
        let rect = if self.poi_mode {
            self.poi.random_card_rect(&mut self.rng)
        } else {
            None
        }
        .or_else(|| self.normal.random_text_rect(stage, &mut self.rng));
        let Some(rect) = rect else {
            return;
        };
        let center = Point::new(
            rect.x0 + rect.width() * f64::from(self.rng.unit()),
            rect.y0 + rect.height() * f64::from(self.rng.unit()),
        );
        let kind = match self.rng.range(4) {
            0 => MicroKind::Tick,
            1 => MicroKind::Cross,
            2 => MicroKind::Bracket,
            _ => MicroKind::Scan,
        };
        self.micro.push(MicroFx {
            center,
            age: 0.0,
            life: 0.24 + self.rng.unit() * 0.22,
            kind,
        });
    }

    pub fn draw(
        &mut self,
        scene: &mut Scene,
        raster_assets: &mut Vec<RasterAsset>,
        stage: &Stage,
        palette: ScenePalette,
        plate_mask: Option<&PlateMaskImage>,
    ) {
        if !self.active {
            return;
        }
        let normal_alpha = if self.poi_mode { 0.55 } else { 1.0 };
        self.normal.draw(
            scene,
            raster_assets,
            stage,
            palette,
            normal_alpha,
            self.time,
        );
        self.selector.draw(scene, stage, palette);
        self.poi
            .draw(scene, stage, palette, self.focus_blend, plate_mask);
        for fx in &self.micro {
            draw_micro(scene, stage, palette, fx);
        }
        let bounds = self.hint.bounds(stage);
        let origin = Point::new(
            stage.width - stage.margin - bounds.width(),
            stage.height - stage.margin - bounds.height(),
        );
        self.hint.draw(
            scene,
            vello::kurbo::Affine::translate((origin.x, origin.y)),
            stage,
            ScenePalette {
                ink: palette.muted,
                ..palette
            },
            0.45,
            self.time,
        );
    }

    pub fn colors(&self) -> ScenePalette {
        self.colors
    }

    pub fn plate_instances(&self) -> &[crate::render::PlateInstance] {
        self.poi.plate_instances()
    }

    #[cfg(test)]
    pub fn debug_hit_marker(&mut self) {
        self.poi.debug_spawn_hit_marker(&mut self.rng);
    }
}

fn draw_micro(scene: &mut Scene, stage: &Stage, palette: ScenePalette, fx: &MicroFx) {
    let progress = (fx.age / fx.life).clamp(0.0, 1.0);
    let alpha = (1.0 - progress) * 0.7;
    let scale = stage.px(10.0) * (0.7 + 0.5 * f64::from(1.0 - progress));
    let width = (stage.unit * 1.2).max(0.8);
    let color = palette.accent.with_alpha(alpha);
    let center = fx.center;
    match fx.kind {
        MicroKind::Tick => {
            scene.stroke(
                &vello::kurbo::Stroke::new(width),
                vello::kurbo::Affine::IDENTITY,
                color,
                None,
                &vello::kurbo::Line::new(
                    Point::new(center.x - scale, center.y),
                    Point::new(center.x + scale, center.y),
                ),
            );
        }
        MicroKind::Cross => {
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                scene.stroke(
                    &vello::kurbo::Stroke::new(width),
                    vello::kurbo::Affine::IDENTITY,
                    color,
                    None,
                    &vello::kurbo::Line::new(
                        Point::new(center.x + dx * scale * 0.3, center.y + dy * scale * 0.3),
                        Point::new(center.x + dx * scale, center.y + dy * scale),
                    ),
                );
            }
        }
        MicroKind::Bracket => {
            scene.stroke(
                &vello::kurbo::Stroke::new(width),
                vello::kurbo::Affine::IDENTITY,
                color,
                None,
                &vello::kurbo::Line::new(
                    Point::new(center.x - scale, center.y - scale),
                    Point::new(center.x + scale, center.y - scale),
                ),
            );
            scene.stroke(
                &vello::kurbo::Stroke::new(width),
                vello::kurbo::Affine::IDENTITY,
                color,
                None,
                &vello::kurbo::Line::new(
                    Point::new(center.x - scale, center.y - scale),
                    Point::new(center.x - scale, center.y + scale),
                ),
            );
        }
        MicroKind::Scan => {
            scene.stroke(
                &vello::kurbo::Stroke::new(width),
                vello::kurbo::Affine::IDENTITY,
                color,
                None,
                &vello::kurbo::Line::new(
                    Point::new(center.x - scale * 1.6, center.y),
                    Point::new(center.x + scale * 1.6, center.y),
                ),
            );
        }
    }
}

fn same_stage(a: &Stage, b: &Stage) -> bool {
    (a.width - b.width).abs() <= f64::EPSILON
        && (a.height - b.height).abs() <= f64::EPSILON
        && (a.unit - b.unit).abs() <= f64::EPSILON
}
