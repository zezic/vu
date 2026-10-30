//! POI focus presentation: floating callout cards around a single model vertex.
//!
//! Each POI beat chooses a new card split, typography, side arrangement and
//! color trio. Cards are anchored to one vertex chosen once per beat and drift
//! slowly around their targets, so the infographic never becomes static.

use std::{collections::BTreeSet, f32::consts::TAU};

use vello::{
    kurbo::{Affine, BezPath, Point, Rect, Stroke},
    peniko::Fill,
    Scene,
};

use crate::{
    text::{BlockAlign, Fonts, UiFace},
    weapon::{ComedySpec, Intent, Joke, Poi, Style, Weapon},
};

use super::super::{detection::Spring, Rng, Visualizer};
use super::{
    color::ScenePalette,
    stage::Stage,
    text::{AnimatedText, Backdrop, CharEffect, PaintStyle},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

struct TextSpec {
    source: String,
    size: f64,
    face: UiFace,
    paint: PaintStyle,
    backdrop: Option<Backdrop>,
    enter_effect: CharEffect,
    exit_effect: CharEffect,
    gap_units: f64,
}

struct CardSpec {
    side: Side,
    width_units: f64,
    pad_units: f64,
    texts: Vec<TextSpec>,
    barcode: Option<u64>,
}

struct PoiContent {
    cards: Vec<CardSpec>,
    has_mlg: bool,
    is_fact: bool,
}

struct PoiText {
    text: AnimatedText,
    gap: f64,
    exit_effect: CharEffect,
}

struct PoiCard {
    texts: Vec<PoiText>,
    pad: f64,
    side: Side,
    width: f64,
    height: f64,
    spring_x: Spring,
    spring_y: Spring,
    drift_phase: f32,
    drift_amp: (f64, f64),
    barcode: Option<u64>,
}

impl PoiCard {
    fn any_visible(&self) -> bool {
        self.texts.iter().any(|text| !text.text.is_hidden())
    }

    fn rect(&self) -> Rect {
        let x = f64::from(self.spring_x.value());
        let y = f64::from(self.spring_y.value());
        Rect::new(
            x - self.width * 0.5,
            y - self.height * 0.5,
            x + self.width * 0.5,
            y + self.height * 0.5,
        )
    }
}

struct HitMarker {
    center: Point,
    age: f32,
    life: f32,
}

pub struct PoiScene {
    key: Option<String>,
    model_id: Option<String>,
    content: Option<PoiContent>,
    vertex: Option<u16>,
    cards: Vec<PoiCard>,
    outgoing: Vec<PoiCard>,
    outgoing_fact: bool,
    stage_unit: f64,
    stage_size: (f64, f64),
    time: f32,
    anchor: Point,
    markers: Vec<HitMarker>,
}

impl Default for PoiScene {
    fn default() -> Self {
        Self {
            key: None,
            model_id: None,
            content: None,
            vertex: None,
            cards: Vec::new(),
            outgoing: Vec::new(),
            outgoing_fact: false,
            stage_unit: f64::NAN,
            stage_size: (0.0, 0.0),
            time: 0.0,
            anchor: Point::new(0.0, 0.0),
            markers: Vec::new(),
        }
    }
}

impl PoiScene {
    pub fn is_visible(&self) -> bool {
        !self.cards.is_empty() || !self.outgoing.is_empty()
    }

    pub fn shows_fact(&self) -> bool {
        self.outgoing_fact || self.content.as_ref().is_some_and(|content| content.is_fact)
    }

    pub fn hide(&mut self) {
        self.key = None;
        self.content = None;
        self.vertex = None;
        self.cards.clear();
        self.outgoing.clear();
        self.outgoing_fact = false;
        self.markers.clear();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        dt: f32,
        model_id: Option<&str>,
        target_id: Option<&str>,
        weapon: Option<&Weapon>,
        comedy_spec: Option<&ComedySpec>,
        visualizer: &Visualizer,
        fonts: &mut Fonts,
        stage: &Stage,
        rng: &mut Rng,
        hat_trigger: bool,
    ) {
        self.time += dt;
        if self.model_id.as_deref() != model_id {
            self.hide();
            self.model_id = model_id.map(str::to_owned);
        }
        let target = target_id.filter(|_| weapon.is_some());
        if let Some(target) = target {
            if self.key.as_deref() != Some(target) {
                self.exit_cards(rng);
                self.key = Some(target.to_owned());
                self.vertex = choose_vertex(visualizer, target, rng);
                self.content = weapon
                    .and_then(|weapon| build_content(weapon, target, comedy_spec, stage, rng));
                self.cards = self
                    .content
                    .as_ref()
                    .map(|content| build_cards(content, stage, fonts, rng, true))
                    .unwrap_or_default();
                self.place_cards(visualizer, stage, true);
            } else if (self.stage_unit - stage.unit).abs() > f64::EPSILON
                || (self.stage_size.0 - stage.width).abs() > f64::EPSILON
                || (self.stage_size.1 - stage.height).abs() > f64::EPSILON
            {
                self.stage_unit = stage.unit;
                self.stage_size = (stage.width, stage.height);
                if let Some(content) = &self.content {
                    let spec = content;
                    self.cards = build_cards(spec, stage, fonts, rng, false);
                    self.place_cards(visualizer, stage, true);
                }
                self.outgoing.clear();
            }
        } else if self.key.is_some() {
            self.key = None;
            self.content = None;
            self.vertex = None;
            self.exit_cards(rng);
        }

        self.stage_unit = stage.unit;
        self.stage_size = (stage.width, stage.height);
        for card in &mut self.cards {
            for text in &mut card.texts {
                text.text.advance(dt);
            }
        }
        for card in &mut self.outgoing {
            for text in &mut card.texts {
                text.text.advance(dt);
            }
        }
        self.outgoing.retain(PoiCard::any_visible);
        if self.outgoing.is_empty() {
            self.outgoing_fact = false;
        }
        self.update_flow(dt, visualizer, stage);
        if hat_trigger {
            if let Some(content) = &self.content {
                if content.has_mlg {
                    self.spawn_hit_marker(rng);
                }
            }
        }
        for marker in &mut self.markers {
            marker.age += dt;
        }
        self.markers.retain(|marker| marker.age < marker.life);
    }

    fn exit_cards(&mut self, rng: &mut Rng) {
        if self.content.as_ref().is_some_and(|content| content.is_fact) {
            self.outgoing_fact = true;
        }
        for mut card in self.cards.drain(..) {
            for text in &mut card.texts {
                if !text.text.is_hidden() {
                    text.text.leave(text.exit_effect, rng);
                }
            }
            self.outgoing.push(card);
        }
    }

    fn anchor_point(&self, visualizer: &Visualizer, stage: &Stage) -> Point {
        let anchor = self
            .vertex
            .and_then(|vertex| visualizer.vertex_screen(vertex))
            .map_or(Point::new(stage.width * 0.5, stage.height * 0.5), |point| {
                Point::new(f64::from(point[0]), f64::from(point[1]))
            });
        Point::new(
            anchor.x.clamp(stage.margin, stage.width - stage.margin),
            anchor.y.clamp(stage.margin, stage.height - stage.margin),
        )
    }

    fn place_cards(&mut self, visualizer: &Visualizer, stage: &Stage, reset: bool) {
        let anchor = self.anchor_point(visualizer, stage);
        self.anchor = anchor;
        let air = stage.px(112.0);
        for card in &mut self.cards {
            let (x, y) = card_target(stage, anchor, card, air);
            if reset {
                card.spring_x.reset(x as f32);
                card.spring_y.reset(y as f32);
            } else {
                card.spring_x.set_target(x as f32);
                card.spring_y.set_target(y as f32);
            }
        }
    }

    fn update_flow(&mut self, dt: f32, visualizer: &Visualizer, stage: &Stage) {
        let anchor = self.anchor_point(visualizer, stage);
        self.anchor = anchor;
        let air = stage.px(112.0);
        let stiffness = 3.0;
        let damping = 3.0;
        let time = f64::from(self.time);
        for card in &mut self.cards {
            let (base_x, base_y) = card_target(stage, anchor, card, air);
            let drift_x = (time * 0.31 + f64::from(card.drift_phase)).sin() * card.drift_amp.0;
            let drift_y =
                (time * 0.24 + f64::from(card.drift_phase) * 1.7).cos() * card.drift_amp.1;
            card.spring_x.set_target((base_x + drift_x) as f32);
            card.spring_y.set_target((base_y + drift_y) as f32);
            card.spring_x.advance(dt, stiffness, damping);
            card.spring_y.advance(dt, stiffness, damping);
        }
        for card in &mut self.outgoing {
            card.spring_x.advance(dt, stiffness, damping);
            card.spring_y.advance(dt, stiffness, damping);
        }
    }

    #[cfg(test)]
    pub fn debug_spawn_hit_marker(&mut self, rng: &mut Rng) {
        self.spawn_hit_marker(rng);
    }

    fn spawn_hit_marker(&mut self, rng: &mut Rng) {
        let Some(card) = self.cards.first() else {
            return;
        };
        let rect = card.rect();
        let center = Point::new(
            rect.x0 + rect.width() * (0.25 + f64::from(rng.unit()) * 0.5),
            rect.y0 + rect.height() * (0.25 + f64::from(rng.unit()) * 0.5),
        );
        self.markers.push(HitMarker {
            center,
            age: 0.0,
            life: 0.3 + rng.unit() * 0.12,
        });
    }

    pub fn random_card_rect(&self, rng: &mut Rng) -> Option<Rect> {
        if self.cards.is_empty() {
            None
        } else {
            Some(self.cards[rng.range(self.cards.len())].rect())
        }
    }

    pub fn occluders(&self, stage: &Stage) -> Vec<Rect> {
        let anchor = self.anchor;
        let pad = stage.px(12.0);
        let mut rects = Vec::new();
        for card in self.cards.iter().chain(&self.outgoing) {
            rects.push(inflate(card.rect(), pad));
        }
        if self.is_visible() {
            let air = stage.px(104.0);
            rects.push(Rect::new(
                anchor.x - air,
                anchor.y - air,
                anchor.x + air,
                anchor.y + air,
            ));
        }
        rects
    }

    pub fn draw(&self, scene: &mut Scene, stage: &Stage, palette: ScenePalette, focus_blend: f32) {
        if !self.is_visible() {
            return;
        }
        let alpha = smoothstep(focus_blend);
        if alpha <= 0.001 {
            return;
        }
        let anchor = self.anchor;
        for card in &self.outgoing {
            if card.any_visible() {
                draw_card(scene, stage, palette, anchor, card, alpha * 0.8, self.time);
            }
        }
        for card in &self.cards {
            draw_leader(scene, stage, palette, anchor, card.rect(), alpha);
            draw_card(scene, stage, palette, anchor, card, alpha, self.time);
        }
        draw_anchor(scene, stage, palette, anchor, alpha);
        for marker in &self.markers {
            draw_hit_marker(scene, stage, palette, marker, alpha);
        }
    }
}

fn choose_vertex(visualizer: &Visualizer, target_id: &str, rng: &mut Rng) -> Option<u16> {
    let vertices = visualizer.poi_vertices(target_id);
    if vertices.is_empty() {
        None
    } else {
        Some(vertices[rng.range(vertices.len())])
    }
}

fn build_content(
    weapon: &Weapon,
    target_id: &str,
    comedy_spec: Option<&ComedySpec>,
    stage: &Stage,
    rng: &mut Rng,
) -> Option<PoiContent> {
    let poi = weapon.poi.iter().find(|poi| poi.id == target_id)?;
    let joke = (rng.unit() < 0.5)
        .then(|| (!poi.jokes.is_empty()).then(|| &poi.jokes[rng.range(poi.jokes.len())]))
        .flatten();
    let (body, is_joke, styles, intent) = match joke {
        Some(joke) => (
            joke.text.clone(),
            true,
            joke.styles.clone(),
            Some(joke.intent),
        ),
        None => (poi.fact.clone(), false, BTreeSet::new(), None),
    };
    let meta = build_meta(poi, joke, comedy_spec, is_joke, &styles, intent, rng);
    let barcode = joke.map(|joke| hash_str(&joke.id));
    let mlg = styles.contains(&Style::Mlg);
    let has_mlg = mlg;
    let vertical = stage.is_tall();
    let available = if meta.is_some() { 3 } else { 2 };
    let split = match rng.range(3) {
        0 => 1,
        1 => 2,
        _ => available,
    }
    .min(available)
    .max(1);
    let mut sides = Vec::new();
    let horizontal = |rng: &mut Rng| {
        if rng.unit() < 0.5 {
            Side::Left
        } else {
            Side::Right
        }
    };
    let vertical_side = |rng: &mut Rng| {
        if rng.unit() < 0.5 {
            Side::Top
        } else {
            Side::Bottom
        }
    };
    match split {
        1 => sides.push(if vertical {
            vertical_side(rng)
        } else {
            horizontal(rng)
        }),
        2 => {
            if vertical {
                sides.push(vertical_side(rng));
                sides.push(if rng.unit() < 0.5 {
                    Side::Left
                } else {
                    Side::Right
                });
            } else {
                sides.push(horizontal(rng));
                sides.push(if sides[0] == Side::Left {
                    Side::Right
                } else {
                    Side::Left
                });
            }
        }
        _ => {
            if vertical {
                sides.push(vertical_side(rng));
            } else {
                sides.push(horizontal(rng));
            }
            sides.push(if vertical {
                if rng.unit() < 0.5 {
                    Side::Left
                } else {
                    Side::Right
                }
            } else if sides[0] == Side::Left {
                Side::Right
            } else {
                Side::Left
            });
            sides.push(if vertical {
                Side::Left
            } else {
                vertical_side(rng)
            });
        }
    }
    let mut cards = Vec::new();
    let label = TextSpec {
        source: poi.label.to_uppercase(),
        size: 56.0,
        face: UiFace::SemiBold,
        paint: random_paint(rng),
        backdrop: None,
        enter_effect: CharEffect::random(rng),
        exit_effect: CharEffect::random(rng),
        gap_units: 12.0,
    };
    let body_size = if is_joke {
        if mlg {
            32.0
        } else {
            28.0
        }
    } else {
        22.0
    };
    let body_face = if is_joke {
        if mlg {
            UiFace::SemiBold
        } else {
            UiFace::Light
        }
    } else {
        UiFace::Light
    };
    let body = TextSpec {
        source: body,
        size: body_size,
        face: body_face,
        paint: PaintStyle::Fill,
        backdrop: None,
        enter_effect: CharEffect::random(rng),
        exit_effect: CharEffect::random(rng),
        gap_units: 8.0,
    };
    let meta = meta.map(|meta| TextSpec {
        source: meta,
        size: 12.5,
        face: UiFace::Light,
        paint: PaintStyle::Fill,
        backdrop: None,
        enter_effect: CharEffect::random(rng),
        exit_effect: CharEffect::random(rng),
        gap_units: 0.0,
    });
    let mut label = label;
    if label.paint == PaintStyle::Hole {
        label.backdrop = Some(Backdrop::random(rng, label.size));
    }
    match split {
        1 => {
            let mut texts = vec![label, body];
            if let Some(meta) = meta {
                texts.push(meta);
            }
            cards.push(CardSpec {
                side: sides[0],
                width_units: 420.0,
                pad_units: 18.0,
                texts,
                barcode,
            });
        }
        2 => {
            let mut label_card = vec![label];
            let mut body_card = vec![body];
            let mut label_barcode = None;
            if let Some(meta) = meta {
                if rng.unit() < 0.5 {
                    label_card.push(meta);
                    label_barcode = barcode;
                } else {
                    body_card.push(meta);
                }
            }
            cards.push(CardSpec {
                side: sides[0],
                width_units: 370.0,
                pad_units: 16.0,
                texts: label_card,
                barcode: label_barcode,
            });
            cards.push(CardSpec {
                side: sides[1],
                width_units: 410.0,
                pad_units: 18.0,
                texts: body_card,
                barcode,
            });
        }
        _ => {
            cards.push(CardSpec {
                side: sides[0],
                width_units: 360.0,
                pad_units: 16.0,
                texts: vec![label],
                barcode: None,
            });
            cards.push(CardSpec {
                side: sides[1],
                width_units: 410.0,
                pad_units: 18.0,
                texts: vec![body],
                barcode: None,
            });
            if let Some(meta) = meta {
                cards.push(CardSpec {
                    side: sides[2],
                    width_units: 330.0,
                    pad_units: 14.0,
                    texts: vec![meta],
                    barcode,
                });
            }
        }
    }
    Some(PoiContent {
        cards,
        has_mlg,
        is_fact: !is_joke,
    })
}

fn build_meta(
    poi: &Poi,
    joke: Option<&Joke>,
    comedy_spec: Option<&ComedySpec>,
    is_joke: bool,
    styles: &BTreeSet<Style>,
    intent: Option<Intent>,
    rng: &mut Rng,
) -> Option<String> {
    if !is_joke {
        return Some(format!("FACT / {}", poi.id.to_uppercase()));
    }
    let joke = joke?;
    let mut lines = Vec::new();
    if rng.unit() < 0.5 {
        lines.push(format!("ID {}", joke.id));
    }
    if rng.unit() < 0.5 && !styles.is_empty() {
        lines.push(format!(
            "STYLES {}",
            styles
                .iter()
                .map(|style| style.label())
                .collect::<Vec<_>>()
                .join(" / ")
        ));
    }
    if rng.unit() < 0.5 {
        if let Some(intent) = intent {
            lines.push(format!("INTENT {}", intent.label()));
        }
    }
    if rng.unit() < 0.6 {
        if let Some(spec) = comedy_spec {
            let mut keywords = Vec::new();
            for style in styles {
                if let Some(guide) = spec.styles.get(style) {
                    keywords.extend(guide.keywords.iter().take(2).cloned());
                }
            }
            if !keywords.is_empty() {
                lines.push(format!("TAGS {}", keywords.join(" / ").to_uppercase()));
            }
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn build_cards(
    content: &PoiContent,
    stage: &Stage,
    fonts: &mut Fonts,
    rng: &mut Rng,
    entrance: bool,
) -> Vec<PoiCard> {
    let mut cards = Vec::new();
    for spec in &content.cards {
        let pad = stage.px(spec.pad_units);
        let max_width = stage
            .px(spec.width_units)
            .min(stage.width * 0.34)
            .max(stage.px(150.0));
        let wrap = Some(((max_width - 2.0 * pad) / stage.unit).max(8.0));
        let mut texts = Vec::new();
        let mut content_width = 0.0f64;
        let mut content_height = 0.0f64;
        for text_spec in &spec.texts {
            let mut text = AnimatedText::new(
                text_spec.source.clone(),
                text_spec.size,
                text_spec.face,
                BlockAlign::Left,
                wrap,
                1.08,
                text_spec.paint,
            );
            if let Some(backdrop) = text_spec.backdrop {
                text = text.with_backdrop(backdrop);
            }
            text.layout(fonts, stage, rng);
            if entrance {
                text.enter(text_spec.enter_effect, rng);
            } else {
                text.set_visible();
            }
            let bounds = text.bounds(stage);
            content_width = content_width.max(bounds.width());
            content_height += bounds.height();
            let gap = stage.px(text_spec.gap_units);
            content_height += gap;
            texts.push(PoiText {
                text,
                gap,
                exit_effect: text_spec.exit_effect,
            });
        }
        let barcode_height = if spec.barcode.is_some() {
            stage.px(18.0)
        } else {
            0.0
        };
        let width = content_width + 2.0 * pad;
        let height = content_height + 2.0 * pad + barcode_height;
        cards.push(PoiCard {
            texts,
            pad,
            side: spec.side,
            width,
            height,
            spring_x: Spring::default(),
            spring_y: Spring::default(),
            drift_phase: rng.unit() * TAU,
            drift_amp: (
                stage.px(3.0 + f64::from(rng.unit()) * 4.0),
                stage.px(2.0 + f64::from(rng.unit()) * 4.0),
            ),
            barcode: spec.barcode,
        });
    }
    cards
}

fn card_target(stage: &Stage, anchor: Point, card: &PoiCard, air: f64) -> (f64, f64) {
    let (mut x, mut y) = match card.side {
        Side::Left => (
            anchor.x - air - card.width * 0.5,
            anchor.y - card.height * 0.5,
        ),
        Side::Right => (
            anchor.x + air + card.width * 0.5,
            anchor.y - card.height * 0.5,
        ),
        Side::Top => (
            anchor.x - card.width * 0.5,
            anchor.y - air - card.height * 0.5,
        ),
        Side::Bottom => (
            anchor.x - card.width * 0.5,
            anchor.y + air + card.height * 0.5,
        ),
    };
    let half_w = card.width * 0.5;
    let half_h = card.height * 0.5;
    x = x.clamp(
        stage.margin + half_w,
        (stage.width - stage.margin - half_w).max(stage.margin + half_w),
    );
    y = y.clamp(
        stage.margin + half_h,
        (stage.height - stage.margin - half_h).max(stage.margin + half_h),
    );
    (x, y)
}

fn draw_card(
    scene: &mut Scene,
    stage: &Stage,
    palette: ScenePalette,
    anchor: Point,
    card: &PoiCard,
    alpha: f32,
    time: f32,
) {
    let rect = card.rect();
    let has_hole = card
        .texts
        .iter()
        .any(|text| text.text.paint == PaintStyle::Hole);
    if !has_hole {
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            palette.muted.with_alpha(0.07 * alpha),
            None,
            &rect,
        );
    }
    draw_decorations(scene, stage, palette, rect, anchor, alpha);
    let x = rect.x0 + card.pad;
    let mut y = rect.y0 + card.pad;
    for text in &card.texts {
        let bounds = text.text.bounds(stage);
        text.text.draw(
            scene,
            Affine::translate((x, y)),
            stage,
            palette,
            alpha,
            time,
        );
        y += bounds.height() + text.gap;
    }
    if let Some(seed) = card.barcode {
        let bar_rect = Rect::new(
            rect.x0 + card.pad,
            rect.y1 - card.pad - stage.px(9.0),
            (rect.x0 + card.pad + stage.px(150.0)).min(rect.x1 - card.pad),
            rect.y1 - card.pad,
        );
        draw_barcode(scene, stage, palette, bar_rect, seed, alpha);
    }
}

fn draw_decorations(
    scene: &mut Scene,
    stage: &Stage,
    palette: ScenePalette,
    rect: Rect,
    anchor: Point,
    alpha: f32,
) {
    let arm = stage.px(18.0);
    let offset = stage.px(3.0);
    let width = (stage.unit * 1.1).max(0.7);
    let color = palette.accent.with_alpha(0.5 * alpha);
    let corners = [
        (rect.x0 - offset, rect.y0 - offset, 1.0, 1.0),
        (rect.x1 + offset, rect.y0 - offset, -1.0, 1.0),
        (rect.x1 + offset, rect.y1 + offset, -1.0, -1.0),
        (rect.x0 - offset, rect.y1 + offset, 1.0, -1.0),
    ];
    for (x, y, dx, dy) in corners {
        scene.stroke(
            &Stroke::new(width),
            Affine::IDENTITY,
            color,
            None,
            &vello::kurbo::Line::new(Point::new(x, y), Point::new(x + dx * arm, y)),
        );
        scene.stroke(
            &Stroke::new(width),
            Affine::IDENTITY,
            color,
            None,
            &vello::kurbo::Line::new(Point::new(x, y), Point::new(x, y + dy * arm)),
        );
    }
    // Crosses on the two corners nearest the POI.
    let near_x = if rect.x1 < anchor.x { rect.x1 } else { rect.x0 };
    let near_y = if rect.y1 < anchor.y { rect.y1 } else { rect.y0 };
    let cross = stage.px(4.5);
    let cross_color = palette.ink.with_alpha(0.55 * alpha);
    for (x, y) in [(near_x, rect.y0), (rect.x0, near_y)] {
        scene.stroke(
            &Stroke::new(width),
            Affine::IDENTITY,
            cross_color,
            None,
            &vello::kurbo::Line::new(Point::new(x - cross, y), Point::new(x + cross, y)),
        );
        scene.stroke(
            &Stroke::new(width),
            Affine::IDENTITY,
            cross_color,
            None,
            &vello::kurbo::Line::new(Point::new(x, y - cross), Point::new(x, y + cross)),
        );
    }
    // Short registration ticks on the side facing the POI.
    let tick_color = palette.muted.with_alpha(0.45 * alpha);
    let side_x = if rect.x1 < anchor.x {
        rect.x1
    } else if rect.x0 > anchor.x {
        rect.x0
    } else {
        rect.x0 + rect.width() * 0.5
    };
    for index in 0..3 {
        let y = rect.y0 + rect.height() * (0.25 + index as f64 * 0.25);
        scene.stroke(
            &Stroke::new(width),
            Affine::IDENTITY,
            tick_color,
            None,
            &vello::kurbo::Line::new(
                Point::new(side_x - stage.px(5.0), y),
                Point::new(side_x + stage.px(5.0), y),
            ),
        );
    }
}

fn draw_barcode(
    scene: &mut Scene,
    stage: &Stage,
    palette: ScenePalette,
    rect: Rect,
    seed: u64,
    alpha: f32,
) {
    let mut value = seed;
    let mut x = rect.x0;
    let width = (stage.unit * 1.4).max(0.8);
    while x < rect.x1 {
        value = value.wrapping_mul(6364136223846793005).wrapping_add(1);
        let bar = 1 + (value >> 60) as u32;
        let gap = 1 + ((value >> 56) & 0x3) as u32;
        let bar_width = f64::from(bar) * width;
        if x + bar_width > rect.x1 {
            break;
        }
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            palette.ink.with_alpha(0.55 * alpha),
            None,
            &Rect::new(x, rect.y0, x + bar_width, rect.y1),
        );
        x += bar_width + f64::from(gap) * width;
    }
}

fn draw_leader(
    scene: &mut Scene,
    stage: &Stage,
    palette: ScenePalette,
    anchor: Point,
    rect: Rect,
    alpha: f32,
) {
    let edge = Point::new(
        anchor.x.clamp(rect.x0, rect.x1),
        anchor.y.clamp(rect.y0, rect.y1),
    );
    let dx = edge.x - anchor.x;
    let dy = edge.y - anchor.y;
    if dx.abs().hypot(dy.abs()) < stage.px(12.0) {
        return;
    }
    let start = Point::new(
        anchor.x + dx.signum() * stage.px(10.0).min(dx.abs() * 0.5),
        anchor.y + dy.signum() * stage.px(10.0).min(dy.abs() * 0.5),
    );
    let mut path = BezPath::new();
    path.move_to((start.x, start.y));
    if dx.abs() > dy.abs() {
        let mid = (start.x + edge.x) * 0.5;
        path.line_to((mid, start.y));
        path.line_to((mid, edge.y));
    } else {
        let mid = (start.y + edge.y) * 0.5;
        path.line_to((start.x, mid));
        path.line_to((edge.x, mid));
    }
    path.line_to((edge.x, edge.y));
    scene.stroke(
        &Stroke::new((stage.unit * 1.0).max(0.7)),
        Affine::IDENTITY,
        palette.accent.with_alpha(0.35 * alpha),
        None,
        &path,
    );
    let tick = stage.px(4.0);
    scene.stroke(
        &Stroke::new((stage.unit * 1.0).max(0.7)),
        Affine::IDENTITY,
        palette.accent.with_alpha(0.45 * alpha),
        None,
        &vello::kurbo::Line::new(
            Point::new(edge.x - tick, edge.y),
            Point::new(edge.x + tick, edge.y),
        ),
    );
}

fn draw_anchor(scene: &mut Scene, stage: &Stage, palette: ScenePalette, anchor: Point, alpha: f32) {
    let radius = stage.px(8.0);
    let width = (stage.unit * 1.2).max(0.8);
    scene.stroke(
        &Stroke::new(width),
        Affine::IDENTITY,
        palette.accent.with_alpha(0.7 * alpha),
        None,
        &vello::kurbo::Circle::new(anchor, radius),
    );
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        scene.stroke(
            &Stroke::new(width),
            Affine::IDENTITY,
            palette.accent.with_alpha(0.7 * alpha),
            None,
            &vello::kurbo::Line::new(
                Point::new(anchor.x + dx * radius * 1.3, anchor.y + dy * radius * 1.3),
                Point::new(anchor.x + dx * radius * 2.2, anchor.y + dy * radius * 2.2),
            ),
        );
    }
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        palette.ink.with_alpha(0.9 * alpha),
        None,
        &vello::kurbo::Circle::new(anchor, (stage.unit * 1.4).max(0.8)),
    );
}

fn draw_hit_marker(
    scene: &mut Scene,
    stage: &Stage,
    palette: ScenePalette,
    marker: &HitMarker,
    alpha: f32,
) {
    let progress = (marker.age / marker.life).clamp(0.0, 1.0);
    let fade = (1.0 - progress) * alpha;
    let progress = f64::from(progress);
    let inner = stage.px(7.0) + stage.px(4.0) * progress;
    let outer = stage.px(22.0) + stage.px(8.0) * progress;
    let width = (stage.unit * 1.8).max(1.0);
    let color = palette.ink.with_alpha(0.85 * fade);
    for (dx, dy) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        scene.stroke(
            &Stroke::new(width),
            Affine::IDENTITY,
            color,
            None,
            &vello::kurbo::Line::new(
                Point::new(marker.center.x + dx * inner, marker.center.y + dy * inner),
                Point::new(marker.center.x + dx * outer, marker.center.y + dy * outer),
            ),
        );
    }
}

fn random_paint(rng: &mut Rng) -> PaintStyle {
    match rng.range(3) {
        0 => PaintStyle::Stroke,
        1 => PaintStyle::Hole,
        _ => PaintStyle::Fill,
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

fn hash_str(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}
