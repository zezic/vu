//! Selector FX infographic: a parts/POI loading grid and its event log.
//!
//! The grid mirrors selector focus changes. A mesh that resolves to a catalog
//! POI focuses that POI row; any other selected mesh gets a dedicated row
//! labelled with the mesh name. Without an authored POI mapping the grid lists
//! every mesh up front.

use std::collections::VecDeque;

use vello::{
    kurbo::{Affine, Point, Rect, Stroke},
    peniko::Fill,
    Scene,
};

use crate::text::{BlockAlign, Fonts, UiFace};

use super::super::{Rng, Visualizer};
use super::{
    color::ScenePalette,
    stage::Stage,
    text::{AnimatedText, PaintStyle},
};

const STATUS_WAIT: &str = "[ WAIT ]";
const STATUS_OK: &str = "[  OK  ]";
const STATUS_FAIL: &str = "[ FAIL ]";
const LOG_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Wait,
    Ok,
    Fail,
}

impl Status {
    fn text(self) -> &'static str {
        match self {
            Self::Wait => STATUS_WAIT,
            Self::Ok => STATUS_OK,
            Self::Fail => STATUS_FAIL,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Hidden,
    Appearing,
    Active,
    Exiting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GridSide {
    Left,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GridTarget {
    /// Index into the resolved showcase POIs.
    Poi(usize),
    /// Mesh part of the current model.
    Mesh(u16),
}

struct GridItem {
    target: GridTarget,
    label: String,
    id: Option<String>,
    fact: Option<String>,
    status: Status,
    progress: f32,
    catching: bool,
    focus_elapsed: f32,
    fill_time: f32,
    appear_at: f32,
    label_text: Option<AnimatedText>,
    status_text: Option<AnimatedText>,
}

struct LogLine {
    text: AnimatedText,
    added_at: f32,
}

pub struct SelectorOverlay {
    phase: Phase,
    clock: f32,
    exit_clock: f32,
    stage_unit: f64,
    stage_size: (f64, f64),
    side: GridSide,
    log_right: bool,
    items: Vec<GridItem>,
    selected: Option<usize>,
    last_part: Option<u16>,
    item_rects: Vec<Rect>,
    grid_rect: Rect,
    header: Option<AnimatedText>,
    summary: Option<AnimatedText>,
    log_rect: Rect,
    log_header: Option<AnimatedText>,
    log: VecDeque<LogLine>,
}

impl Default for SelectorOverlay {
    fn default() -> Self {
        Self {
            phase: Phase::Hidden,
            clock: 0.0,
            exit_clock: 0.0,
            stage_unit: f64::NAN,
            stage_size: (0.0, 0.0),
            side: GridSide::Right,
            log_right: false,
            items: Vec::new(),
            selected: None,
            last_part: None,
            item_rects: Vec::new(),
            grid_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            header: None,
            summary: None,
            log_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            log_header: None,
            log: VecDeque::new(),
        }
    }
}

impl SelectorOverlay {
    pub fn hide(&mut self) {
        self.phase = Phase::Hidden;
        self.items.clear();
        self.log.clear();
        self.selected = None;
        self.last_part = None;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        active: bool,
        selected_part: Option<u16>,
        visualizer: &Visualizer,
        fonts: &mut Fonts,
        stage: &Stage,
        rng: &mut Rng,
    ) {
        if active {
            if !matches!(self.phase, Phase::Appearing | Phase::Active) {
                self.start(visualizer, fonts, stage, rng);
            }
            self.phase = Phase::Active;
            self.clock += dt;
            if self.last_part != selected_part {
                if let Some(part) = selected_part {
                    self.focus_part(part, visualizer, fonts, stage, rng);
                }
                self.last_part = selected_part;
            }
            self.advance_items(dt);
            self.ensure_layout(fonts, stage, rng);
            return;
        }
        match self.phase {
            Phase::Appearing | Phase::Active => {
                self.phase = Phase::Exiting;
                self.exit_clock = 0.0;
                for item in &mut self.items {
                    if item.status == Status::Wait {
                        item.status = Status::Fail;
                    }
                }
                self.push_log(
                    format!(
                        "[+{:06.2}] FX/HALT  OK {}  FAIL {}",
                        self.clock,
                        self.count(Status::Ok),
                        self.count(Status::Fail)
                    ),
                    fonts,
                    stage,
                    rng,
                );
                self.layout(fonts, stage, rng);
            }
            Phase::Exiting => {
                self.exit_clock += dt;
                if self.exit_clock > self.exit_duration() {
                    self.hide();
                }
            }
            Phase::Hidden => {}
        }
    }

    fn start(&mut self, visualizer: &Visualizer, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng) {
        self.phase = Phase::Appearing;
        self.clock = 0.0;
        self.exit_clock = 0.0;
        self.selected = None;
        self.last_part = None;
        self.items.clear();
        self.log.clear();
        self.side = match rng.range(3) {
            0 => GridSide::Left,
            1 => GridSide::Right,
            _ => GridSide::Bottom,
        };
        self.log_right = match self.side {
            GridSide::Left => true,
            GridSide::Right => false,
            GridSide::Bottom => rng.unit() < 0.5,
        };
        let pois = visualizer.showcase_pois();
        if pois.is_empty() {
            // No authored POI mapping: list every mesh so the grid can still
            // track a part each time the selector focuses one.
            for part in 0..visualizer.part_count().unwrap_or(0) {
                let label = visualizer
                    .part_name(part)
                    .map_or_else(|| format!("PART {part}"), grid_mesh_label);
                self.items.push(GridItem {
                    target: GridTarget::Mesh(part),
                    label,
                    id: None,
                    fact: None,
                    status: Status::Wait,
                    progress: 0.0,
                    catching: false,
                    focus_elapsed: 0.0,
                    fill_time: 1.2,
                    appear_at: 0.0,
                    label_text: None,
                    status_text: None,
                });
            }
        } else {
            let weapon = visualizer.weapon();
            for (poi_index, poi) in pois.iter().enumerate() {
                let fact = weapon
                    .and_then(|weapon| weapon.poi.iter().find(|catalog| catalog.id == poi.id))
                    .map(|catalog| catalog.fact.clone());
                self.items.push(GridItem {
                    target: GridTarget::Poi(poi_index),
                    label: poi.label.to_uppercase(),
                    id: Some(poi.id.clone()),
                    fact,
                    status: Status::Wait,
                    progress: 0.0,
                    catching: false,
                    focus_elapsed: 0.0,
                    fill_time: 1.2,
                    appear_at: 0.0,
                    label_text: None,
                    status_text: None,
                });
            }
        }
        for (index, item) in self.items.iter_mut().enumerate() {
            item.appear_at = index as f32 * 0.035;
        }
        self.layout(fonts, stage, rng);
        self.push_log(
            format!("[+000.00] SELECTOR FX ONLINE  PARTS {}", self.items.len()),
            fonts,
            stage,
            rng,
        );
        self.layout(fonts, stage, rng);
    }

    fn focus_part(
        &mut self,
        part: u16,
        visualizer: &Visualizer,
        fonts: &mut Fonts,
        stage: &Stage,
        rng: &mut Rng,
    ) {
        let target = match visualizer.showcase_poi_index_for_part(part) {
            Some(poi_index) => GridTarget::Poi(poi_index),
            None => GridTarget::Mesh(part),
        };
        let mesh_name = visualizer
            .part_name(part)
            .map_or_else(|| format!("PART {part}"), str::to_owned);
        let index = match self.items.iter().position(|item| item.target == target) {
            Some(index) => index,
            None => {
                assert!(
                    matches!(target, GridTarget::Mesh(_)),
                    "the POI grid is built when the selector starts"
                );
                self.items.push(GridItem {
                    target,
                    label: grid_mesh_label(&mesh_name),
                    id: None,
                    fact: None,
                    status: Status::Wait,
                    progress: 0.0,
                    catching: false,
                    focus_elapsed: 0.0,
                    fill_time: 1.2,
                    appear_at: self.clock,
                    label_text: None,
                    status_text: None,
                });
                self.items.len() - 1
            }
        };
        if let Some(previous) = self.selected {
            if previous != index {
                self.items[previous].status = Status::Ok;
                self.items[previous].catching = true;
            }
        }
        let fill_time = 1.2 + rng.unit() * 1.3;
        let clock = self.clock;
        let item = &mut self.items[index];
        item.focus_elapsed = 0.0;
        item.progress = 0.0;
        item.catching = false;
        item.fill_time = fill_time;
        self.selected = Some(index);
        let line = match (item.target, item.id.as_deref(), item.fact.as_deref()) {
            (GridTarget::Poi(_), Some(id), Some(fact)) => format!(
                "[+{clock:06.2}] POI {id} :: MESH {mesh_name} :: {}",
                first_sentence(fact)
            ),
            _ => format!("[+{clock:06.2}] PART {mesh_name} :: NO SPEC ANCHOR"),
        };
        self.push_log(line, fonts, stage, rng);
        self.layout(fonts, stage, rng);
    }

    fn count(&self, status: Status) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == status)
            .count()
    }

    fn advance_items(&mut self, dt: f32) {
        if let Some(selected) = self.selected {
            if let Some(item) = self.items.get_mut(selected) {
                item.focus_elapsed += dt;
                let tau = (item.fill_time / 3.0).max(0.1);
                item.progress = 1.0 - (-item.focus_elapsed / tau).exp();
            }
        }
        for item in &mut self.items {
            if item.catching {
                item.progress += (1.0 - item.progress) * (1.0 - (-dt / 0.035).exp());
                if item.progress >= 0.995 {
                    item.progress = 1.0;
                    item.catching = false;
                }
            }
        }
    }

    fn push_log(&mut self, source: String, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        let width = self.log_text_width(stage);
        let mut text = AnimatedText::new(
            source,
            10.5,
            UiFace::Light,
            BlockAlign::Left,
            Some((width / stage.unit).max(8.0)),
            1.12,
            PaintStyle::Fill,
        );
        text.layout(fonts, stage, rng);
        text.set_visible();
        self.log.push_back(LogLine {
            text,
            added_at: self.clock,
        });
    }

    fn log_text_width(&self, stage: &Stage) -> f64 {
        (self.log_rect.width() - stage.px(18.0)).max(stage.px(80.0))
    }

    fn exit_duration(&self) -> f32 {
        self.items.len() as f32 * 0.035 + 0.32
    }

    fn ensure_layout(&mut self, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng) {
        if (self.stage_unit - stage.unit).abs() <= f64::EPSILON
            && (self.stage_size.0 - stage.width).abs() <= f64::EPSILON
            && (self.stage_size.1 - stage.height).abs() <= f64::EPSILON
        {
            return;
        }
        self.layout(fonts, stage, rng);
    }

    fn layout(&mut self, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng) {
        self.stage_unit = stage.unit;
        self.stage_size = (stage.width, stage.height);
        let count = self.items.len().max(1);
        let gap = stage.px(3.0);
        let item_height = stage.px(25.0);
        let header_height = stage.px(20.0);
        let (columns, _rows, grid_rect) = match self.side {
            GridSide::Left | GridSide::Right => {
                let width = stage.column_width();
                let columns = 2;
                let rows = count.div_ceil(columns);
                let height = header_height + rows as f64 * item_height + (rows - 1) as f64 * gap;
                let x = match self.side {
                    GridSide::Left => stage.left.x0,
                    GridSide::Right => stage.right.x1 - width,
                    GridSide::Bottom => unreachable!(),
                };
                let y = (stage.height * 0.5 - height * 0.5).clamp(
                    stage.margin,
                    (stage.height - stage.margin - height).max(stage.margin),
                );
                (columns, rows, Rect::new(x, y, x + width, y + height))
            }
            GridSide::Bottom => {
                let width = (stage.width * 0.68).min(stage.width - 2.0 * stage.margin);
                let columns = count.div_ceil(2).clamp(2, 6);
                let rows = count.div_ceil(columns);
                let height = header_height + rows as f64 * item_height + (rows - 1) as f64 * gap;
                let x = (stage.width - width) * 0.5;
                let y = stage.height - stage.margin - height;
                (columns, rows, Rect::new(x, y, x + width, y + height))
            }
        };
        self.grid_rect = grid_rect;
        let item_width = (grid_rect.width() - (columns - 1) as f64 * gap) / columns as f64;
        self.item_rects.clear();
        for index in 0..self.items.len() {
            let column = index % columns;
            let row = index / columns;
            let x = grid_rect.x0 + column as f64 * (item_width + gap);
            let y = grid_rect.y0 + header_height + row as f64 * (item_height + gap);
            self.item_rects
                .push(Rect::new(x, y, x + item_width, y + item_height));
        }
        let log_column = if self.log_right {
            stage.right
        } else {
            stage.left
        };
        let log_width = log_column.width().min(stage.px(440.0)).max(stage.px(150.0));
        let log_height = stage.px(250.0).min(stage.height * 0.4);
        let log_x = log_column.x0;
        let log_y = (stage.height * 0.5 - log_height * 0.5).clamp(
            stage.margin,
            (stage.height - stage.margin - log_height).max(stage.margin),
        );
        self.log_rect = Rect::new(log_x, log_y, log_x + log_width, log_y + log_height);

        self.header = Some(static_text(
            "PARTS BUS / SELECTOR FX",
            13.0,
            UiFace::SemiBold,
            None,
            BlockAlign::Left,
            fonts,
            stage,
            rng,
        ));
        self.log_header = Some(static_text(
            "EVENT LOG",
            11.0,
            UiFace::SemiBold,
            None,
            BlockAlign::Left,
            fonts,
            stage,
            rng,
        ));
        self.rebuild_item_texts(fonts, stage, rng);
        self.rebuild_log_texts(fonts, stage, rng);
        self.rebuild_summary(fonts, stage, rng);
    }

    fn rebuild_item_texts(&mut self, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng) {
        let status_width = stage.px(56.0);
        for (index, item) in self.items.iter_mut().enumerate() {
            let Some(rect) = self.item_rects.get(index).copied() else {
                continue;
            };
            let label_width = (rect.width() - status_width - stage.px(16.0)).max(stage.px(40.0));
            item.label_text = Some(static_text(
                &item.label,
                12.5,
                UiFace::SemiBold,
                Some(label_width / stage.unit),
                BlockAlign::Left,
                fonts,
                stage,
                rng,
            ));
            item.status_text = Some(static_text(
                item.status.text(),
                10.5,
                UiFace::SemiBold,
                None,
                BlockAlign::Left,
                fonts,
                stage,
                rng,
            ));
        }
    }

    fn rebuild_log_texts(&mut self, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng) {
        let width = (self.log_text_width(stage) / stage.unit).max(8.0);
        for line in &mut self.log {
            let source = line.text.source.clone();
            let mut text = AnimatedText::new(
                source,
                11.0,
                UiFace::Light,
                BlockAlign::Left,
                Some(width),
                1.12,
                PaintStyle::Fill,
            );
            text.layout(fonts, stage, rng);
            text.set_visible();
            line.text = text;
        }
    }

    fn rebuild_summary(&mut self, fonts: &mut Fonts, stage: &Stage, rng: &mut Rng) {
        let summary = format!(
            "OK {:02}  FAIL {:02}  WAIT {:02}",
            self.count(Status::Ok),
            self.count(Status::Fail),
            self.count(Status::Wait)
        );
        self.summary = Some(static_text(
            &summary,
            10.0,
            UiFace::Light,
            None,
            BlockAlign::Right,
            fonts,
            stage,
            rng,
        ));
    }

    pub fn occluders(&self, stage: &Stage) -> Vec<Rect> {
        if self.phase == Phase::Hidden {
            return Vec::new();
        }
        let pad = stage.px(10.0);
        vec![inflate(self.grid_rect, pad), inflate(self.log_rect, pad)]
    }

    pub fn draw(&self, scene: &mut Scene, stage: &Stage, palette: ScenePalette) {
        if self.phase == Phase::Hidden {
            return;
        }
        let grid_header_height = stage.px(20.0);
        if let Some(header) = &self.header {
            let alpha = self.item_alpha(0);
            draw_text(
                scene,
                header,
                Point::new(self.grid_rect.x0, self.grid_rect.y0),
                stage,
                palette.accent.with_alpha(0.75 * alpha),
            );
        }
        if let Some(summary) = &self.summary {
            let bounds = summary.bounds(stage);
            let alpha = self.item_alpha(0);
            draw_text(
                scene,
                summary,
                Point::new(
                    self.grid_rect.x1 - bounds.width(),
                    self.grid_rect.y0 + stage.px(2.0),
                ),
                stage,
                palette.muted.with_alpha(0.7 * alpha),
            );
        }
        for (index, item) in self.items.iter().enumerate() {
            let Some(rect) = self.item_rects.get(index).copied() else {
                continue;
            };
            let alpha = self.item_alpha(index);
            if alpha <= 0.001 {
                continue;
            }
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                palette.muted.with_alpha(0.1 * alpha),
                None,
                &rect,
            );
            if item.progress > 0.001 {
                let fill_width = rect.width() * f64::from(item.progress);
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    palette.accent.with_alpha(0.14 * alpha),
                    None,
                    &Rect::new(rect.x0, rect.y0, rect.x0 + fill_width, rect.y1),
                );
            }
            let status_width = stage.px(56.0);
            let status_rect = Rect::new(rect.x1 - status_width, rect.y0, rect.x1, rect.y1);
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                palette.muted.with_alpha(0.08 * alpha),
                None,
                &status_rect,
            );
            if self.selected == Some(index) {
                scene.stroke(
                    &Stroke::new((stage.unit * 1.2).max(0.8)),
                    Affine::IDENTITY,
                    palette.accent.with_alpha(0.72 * alpha),
                    None,
                    &rect,
                );
            }
            if let Some(label) = &item.label_text {
                let bounds = label.bounds(stage);
                let y = rect.y0 + (rect.height() - bounds.height()) * 0.5;
                draw_text(
                    scene,
                    label,
                    Point::new(rect.x0 + stage.px(7.0), y),
                    stage,
                    palette.ink.with_alpha(0.82 * alpha),
                );
            }
            if let Some(status) = &item.status_text {
                let bounds = status.bounds(stage);
                let x = status_rect.x0 + (status_rect.width() - bounds.width()) * 0.5;
                let y = rect.y0 + (rect.height() - bounds.height()) * 0.5;
                let color = match item.status {
                    Status::Wait => palette.muted.with_alpha(0.7 * alpha),
                    Status::Ok => palette.accent.with_alpha(0.95 * alpha),
                    Status::Fail => palette.ink.with_alpha(0.8 * alpha),
                };
                draw_text(scene, status, Point::new(x, y), stage, color);
            }
        }
        let log_alpha = self.log_alpha();
        if log_alpha > 0.001 {
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                palette.muted.with_alpha(0.006 * log_alpha),
                None,
                &self.log_rect,
            );
            scene.stroke(
                &Stroke::new((stage.unit * 0.8).max(0.6)),
                Affine::IDENTITY,
                palette.muted.with_alpha(0.3 * log_alpha),
                None,
                &self.log_rect,
            );
            if let Some(header) = &self.log_header {
                draw_text(
                    scene,
                    header,
                    Point::new(
                        self.log_rect.x0 + stage.px(8.0),
                        self.log_rect.y0 + stage.px(5.0),
                    ),
                    stage,
                    palette.accent.with_alpha(0.65 * log_alpha),
                );
            }
            let mut y = self.log_rect.y1 - stage.px(7.0);
            let left = self.log_rect.x0 + stage.px(8.0);
            for line in self.log.iter().rev() {
                let bounds = line.text.bounds(stage);
                if y - bounds.height() < self.log_rect.y0 + grid_header_height {
                    break;
                }
                let age = (self.clock - line.added_at).max(0.0);
                let fade = smoothstep(age / 0.18);
                y -= bounds.height();
                draw_text(
                    scene,
                    &line.text,
                    Point::new(left, y),
                    stage,
                    palette.ink.with_alpha(0.72 * log_alpha * fade),
                );
                y -= stage.px(1.5);
            }
        }
    }

    fn item_alpha(&self, index: usize) -> f32 {
        match self.phase {
            Phase::Hidden => 0.0,
            Phase::Appearing | Phase::Active => self
                .items
                .get(index)
                .map_or(1.0, |item| smoothstep((self.clock - item.appear_at) / 0.2)),
            Phase::Exiting => {
                let offset = index as f32 * 0.035;
                1.0 - smoothstep((self.exit_clock - offset) / 0.2)
            }
        }
    }

    fn log_alpha(&self) -> f32 {
        match self.phase {
            Phase::Hidden => 0.0,
            Phase::Appearing | Phase::Active => smoothstep((self.clock - 0.1) / 0.25),
            Phase::Exiting => {
                let start = self.items.len() as f32 * 0.035;
                1.0 - smoothstep((self.exit_clock - start) / 0.25)
            }
        }
    }
}

fn inflate(rect: Rect, amount: f64) -> Rect {
    Rect::new(
        rect.x0 - amount,
        rect.y0 - amount,
        rect.x1 + amount,
        rect.y1 + amount,
    )
}

fn static_text(
    source: &str,
    size: f64,
    face: UiFace,
    wrap: Option<f64>,
    align: BlockAlign,
    fonts: &mut Fonts,
    stage: &Stage,
    rng: &mut Rng,
) -> AnimatedText {
    let mut text = AnimatedText::new(source, size, face, align, wrap, 1.1, PaintStyle::Fill);
    text.layout(fonts, stage, rng);
    text.set_visible();
    text
}

fn draw_text(
    scene: &mut Scene,
    text: &AnimatedText,
    origin: Point,
    stage: &Stage,
    color: vello::peniko::Color,
) {
    if let Some(plan) = text.plan() {
        plan.draw_alpha(
            scene,
            Affine::translate((origin.x, origin.y)),
            color,
            1.0,
            crate::text::TextPaint::Fill,
        );
    }
    let _ = stage;
}

fn grid_mesh_label(mesh_name: &str) -> String {
    let label = mesh_name.to_uppercase();
    if label.chars().count() > 18 {
        let mut short: String = label.chars().take(15).collect();
        short.push_str("...");
        short
    } else {
        label
    }
}

fn first_sentence(fact: &str) -> String {
    let cut = fact
        .find(". ")
        .or_else(|| fact.find('\n'))
        .map_or(fact.len(), |index| index + 1);
    let mut sentence = fact[..cut].trim().to_owned();
    if sentence.chars().count() > 118 {
        sentence = sentence.chars().take(115).collect();
        sentence.push_str("...");
    }
    sentence
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}
