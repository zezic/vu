use strum::{EnumCount, VariantArray};
use vello::{
    kurbo::{Affine, BezPath, Cap, Circle, Line, Point, Rect, Size, Stroke},
    peniko::{Color, Fill},
    Scene,
};

use super::{DebugRefresh, View};
use crate::{
    config::VisualizerTextScale,
    text::{Fonts, Text, UiFace},
    theme::MeterColors,
    visualizer::{
        BandComponent, BandRouting, ComputeNode, Contribution, DebugHistory, DebugSample,
        DebugSelection, DebugSnapshot, Dependency, Detector, EnvelopeTarget, ExtraRef, Input,
        InputShape, Mix, MixPoint, Mixer, OnsetDebug, Palette, Parameter, Property, StyleInput,
        TreeTarget, Visualizer, BAND_COUNT,
    },
};

const PANEL_WIDTH: f64 = 440.0;
const PANEL_MARGIN: f64 = 16.0;
const HEADER_HEIGHT: f64 = 16.0;
const BAND_ROW_HEIGHT: f64 = 10.0;
const DIVIDER_GAP: f64 = 7.0;
const PROPERTY_ROW_HEIGHT: f64 = 10.0;
const PANEL_BOTTOM_PAD: f64 = 3.0;
const FOOTER_HEIGHT: f64 = 28.0;
const EXTENSION_GAP: f64 = 6.0;
const EXTENSION_EXTRA_WIDTH: f64 = 300.0;
const EXTENSION_MAX_HEIGHT: f64 = 180.0;
const EXTENSION_MIN_HEIGHT: f64 = 70.0;
const EXTENSION_HEADING_HEIGHT: f64 = 11.0;
const EXTENSION_LINE_HEIGHT: f64 = 9.5;

const PROPERTY_COLUMNS: usize = 2;
const PROPERTY_ROWS: usize = Property::COUNT.div_ceil(PROPERTY_COLUMNS);

const TREE_INDENT: f64 = 13.0;
const TREE_PARAM_ROW: f64 = 16.0;
const CURVE_PREVIEW_HEIGHT: f64 = 116.0;
const SLIDER_TRACK_HEIGHT: f64 = 7.0;
/// Keep the bottom-left hotkey cheatsheet visible beside the tree.
const TREE_LEFT_INSET: f64 = 136.0;
/// Width the tree keeps before the inspector gives up space.
const TREE_MIN_WIDTH: f64 = 270.0;
/// Very narrow trees are still more useful than hiding them entirely.
pub(super) const TREE_MIN_VISIBLE_WIDTH: f64 = 180.0;

/// Panel geometry shared by drawing and hit testing. Keeping one layout keeps
/// the selection targets exactly under the visible rows.
pub(super) struct Layout {
    pub panel: Rect,
    pub extension: Rect,
    band_rows: [Rect; BAND_COUNT],
    property_cells: [Rect; Property::COUNT],
    footer_top: f64,
    scale: f64,
}

impl Layout {
    pub fn new(view: &View, text_scale: VisualizerTextScale) -> Self {
        let scale = view.ui_scale() * f64::from(text_scale.factor());
        let margin = PANEL_MARGIN * scale;
        let panel_width = (PANEL_WIDTH * scale)
            .min(view.viewport.width - margin * 2.0)
            .max(1.0);
        let x0 = view.viewport.width - margin - panel_width;
        let y0 = margin;
        let header_height = HEADER_HEIGHT * scale;
        let band_row_height = BAND_ROW_HEIGHT * scale;
        let divider_gap = DIVIDER_GAP * scale;
        let property_row_height = PROPERTY_ROW_HEIGHT * scale;
        let footer_height = FOOTER_HEIGHT * scale;
        let body_height = header_height
            + BAND_COUNT as f64 * band_row_height
            + divider_gap
            + PROPERTY_ROWS as f64 * property_row_height
            + PANEL_BOTTOM_PAD * scale
            + footer_height;
        let panel_height = body_height
            .min(view.viewport.height - margin * 2.0)
            .max(1.0);
        let panel =
            Rect::from_origin_size(Point::new(x0, y0), Size::new(panel_width, panel_height));

        let band_top = y0 + header_height;
        let band_rows = std::array::from_fn(|index| {
            Rect::from_origin_size(
                Point::new(x0 + 6.0 * scale, band_top + index as f64 * band_row_height),
                Size::new(panel_width - 12.0 * scale, band_row_height - 1.0 * scale),
            )
        });
        let property_top = band_top + BAND_COUNT as f64 * band_row_height + divider_gap;
        let cell_gap = 4.0 * scale;
        let column_width = (panel_width - 16.0 * scale - cell_gap) / PROPERTY_COLUMNS as f64;
        let property_cells = std::array::from_fn(|index| {
            let row = index / PROPERTY_COLUMNS;
            let column = index % PROPERTY_COLUMNS;
            let left = x0 + 8.0 * scale + column as f64 * (column_width + cell_gap);
            let top = property_top + row as f64 * property_row_height;
            Rect::from_origin_size(
                Point::new(left, top),
                Size::new(column_width, property_row_height - 1.0 * scale),
            )
        });
        let footer_top = panel.y1 - footer_height - PANEL_BOTTOM_PAD * scale;

        let extension_width = (panel_width + EXTENSION_EXTRA_WIDTH * scale)
            .min(view.viewport.width - margin * 2.0)
            .max(1.0);
        let extension_top = panel.y1 + EXTENSION_GAP * scale;
        let extension_height = (view.viewport.height - margin - extension_top)
            .clamp(0.0, EXTENSION_MAX_HEIGHT * scale);
        let extension = Rect::from_origin_size(
            Point::new(panel.x1 - extension_width, extension_top),
            Size::new(extension_width, extension_height),
        );

        Self {
            panel,
            extension,
            band_rows,
            property_cells,
            footer_top,
            scale,
        }
    }

    pub(super) fn item_at(&self, point: Point) -> Option<DebugSelection> {
        if !self.panel.contains(point) {
            return None;
        }
        if let Some(band) = self.band_rows.iter().position(|row| row.contains(point)) {
            return Some(DebugSelection::Band(band));
        }
        self.property_cells
            .iter()
            .position(|cell| cell.contains(point))
            .map(|index| DebugSelection::Property(Property::VARIANTS[index]))
    }

    #[cfg(test)]
    pub(super) fn property_center(&self, property: Property) -> Point {
        self.property_cells[property.index()].center()
    }

    #[cfg(test)]
    pub(super) fn band_center(&self, band: usize) -> Point {
        self.band_rows[band].center()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    fonts: &mut Fonts,
    scene: &mut Scene,
    view: &View,
    snapshot: &DebugSnapshot,
    history: &DebugHistory,
    visualizer: &Visualizer,
    palette: Palette,
    colors: MeterColors,
    refresh: DebugRefresh,
    text_scale: VisualizerTextScale,
    selection: Option<DebugSelection>,
    pointer: Option<Point>,
    tree_target: Option<TreeTarget>,
    graph_max: &[Option<GraphMax>; Property::COUNT],
) {
    let property = match selection {
        Some(DebugSelection::Property(property)) => Some(property),
        _ => None,
    };
    let layout = Layout::new(view, text_scale);
    let hovered = pointer.and_then(|point| layout.item_at(point));
    draw_panel(
        fonts, scene, &layout, snapshot, visualizer, palette, colors, refresh, selection, hovered,
    );

    let extension = if let Some(property) = property {
        let (_, extension) = property_panes(view, text_scale);
        draw_computation_tree(
            fonts,
            scene,
            view,
            snapshot,
            visualizer,
            palette,
            colors,
            property,
            text_scale,
            tree_target,
            pointer,
        );
        extension
    } else {
        layout.extension
    };

    if let Some(selection) = selection {
        draw_extension(
            fonts, scene, &layout, extension, snapshot, history, visualizer, palette, colors,
            selection, graph_max,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_panel(
    fonts: &mut Fonts,
    scene: &mut Scene,
    layout: &Layout,
    snapshot: &DebugSnapshot,
    visualizer: &Visualizer,
    palette: Palette,
    colors: MeterColors,
    refresh: DebugRefresh,
    selection: Option<DebugSelection>,
    hovered: Option<DebugSelection>,
) {
    let scale = layout.scale;
    let panel = layout.panel;
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        colors.background.with_alpha(0.92),
        None,
        &panel,
    );
    scene.stroke(
        &Stroke::new(1.0),
        Affine::IDENTITY,
        palette.neutral[2].with_alpha(0.55),
        None,
        &panel,
    );
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        palette.vivid(9).with_alpha(0.55),
        None,
        &Rect::from_origin_size(
            Point::new(panel.x0, panel.y0),
            Size::new(2.0 * scale, panel.height()),
        ),
    );

    let header = fit(
        fonts,
        &format!(
            "ANALYZER {} BANDS / {}  V VALUE  B BASS-ONSET  W WIDE-ONSET",
            BAND_COUNT,
            refresh.name()
        ),
        (9.5 * scale) as f32,
        UiFace::SemiBold,
        panel.width() - 16.0 * scale,
    );
    let header_center = panel.y0 + HEADER_HEIGHT * scale * 0.5;
    draw_left_center(
        scene,
        &header,
        panel.x0 + 8.0 * scale,
        header_center,
        palette.vivid(9),
    );

    for (index, band) in snapshot.bands.iter().enumerate() {
        let row = layout.band_rows[index];
        if row.y1 > layout.footer_top {
            break;
        }
        let item = DebugSelection::Band(index);
        let selected = selection == Some(item);
        let highlighted = hovered == Some(item);
        let color = palette.vivid(index);
        draw_row_highlight(scene, row, color, highlighted, selected, scale);

        let center = row.center().y;
        let label = format!(
            "{:02} {:>5.0}Hz {:<6} {:.2}/{:.2}",
            index,
            (band.low_hz + band.high_hz) * 0.5,
            band.role.name(),
            band.value,
            band.raw
        );
        let label = fit(
            fonts,
            &label,
            (9.0 * scale) as f32,
            UiFace::Light,
            148.0 * scale,
        );
        draw_left_center(
            scene,
            &label,
            row.x0,
            center,
            color.with_alpha(if selected || highlighted { 1.0 } else { 0.9 }),
        );

        let bar_width = 60.0 * scale;
        let bar_height = 3.0 * scale;
        let bar_x = row.x1 - 6.0 * scale - bar_width;
        let dependency_x = row.x0 + 152.0 * scale;
        let dependency_width = bar_x - dependency_x - 5.0 * scale;
        let dependencies = band_dependency_line(index, visualizer);
        let dependency = fit(
            fonts,
            &dependencies,
            (7.5 * scale) as f32,
            UiFace::Light,
            dependency_width,
        );
        draw_left_center(
            scene,
            &dependency,
            dependency_x,
            center,
            palette.neutral[1].with_alpha(if selected || highlighted { 0.95 } else { 0.62 }),
        );

        let bar_top = center - bar_height * 0.5;
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            palette.neutral[0].with_alpha(0.18),
            None,
            &Rect::from_origin_size(Point::new(bar_x, bar_top), Size::new(bar_width, bar_height)),
        );
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            color.with_alpha(0.85),
            None,
            &Rect::from_origin_size(
                Point::new(bar_x, bar_top),
                Size::new(
                    bar_width * f64::from(band.value.clamp(0.0, 1.0)),
                    bar_height,
                ),
            ),
        );
        scene.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            palette.neutral[0].with_alpha(0.9),
            None,
            &Line::new(
                Point::new(
                    bar_x + bar_width * f64::from(band.raw.clamp(0.0, 1.0)),
                    bar_top - scale,
                ),
                Point::new(
                    bar_x + bar_width * f64::from(band.raw.clamp(0.0, 1.0)),
                    bar_top + bar_height + scale,
                ),
            ),
        );
    }
    let property_top = layout
        .band_rows
        .last()
        .map_or(panel.y0, |row| row.y1 + DIVIDER_GAP * scale);
    scene.stroke(
        &Stroke::new(1.0),
        Affine::IDENTITY,
        palette.neutral[2].with_alpha(0.55),
        None,
        &Line::new(
            Point::new(panel.x0 + 8.0 * scale, property_top - 3.0 * scale),
            Point::new(panel.x1 - 8.0 * scale, property_top - 3.0 * scale),
        ),
    );

    for (index, &property) in Property::VARIANTS.iter().enumerate() {
        let cell = layout.property_cells[index];
        if cell.y1 > layout.footer_top {
            break;
        }
        let item = DebugSelection::Property(property);
        let selected = selection == Some(item);
        let highlighted = hovered == Some(item);
        let color = palette.vivid(property.index());
        draw_row_highlight(scene, cell, color, highlighted, selected, scale);

        let center = cell.center().y;
        let value = snapshot.properties[property.index()];
        let name = fit(
            fonts,
            property.name(),
            (8.5 * scale) as f32,
            UiFace::Light,
            cell.width() - 38.0 * scale,
        );
        draw_left_center(
            scene,
            &name,
            cell.x0 + 3.0 * scale,
            center,
            color.with_alpha(if selected || highlighted { 1.0 } else { 0.9 }),
        );
        let value = fit(
            fonts,
            &format!("{value:>5.2}"),
            (8.5 * scale) as f32,
            UiFace::SemiBold,
            32.0 * scale,
        );
        draw_right_center(
            scene,
            &value,
            cell.x1 - 3.0 * scale,
            center,
            palette.neutral[0].with_alpha(0.9),
        );
    }

    let footer = layout.footer_top;
    let first = format!(
        "PWR S{:.2} R{:.2} F{:.2}  BASS S{:.2} R{:.2} F{:.2}  TREB S{:.2}",
        snapshot.power.smooth,
        snapshot.power.raw,
        snapshot.flux,
        snapshot.bass.smooth,
        snapshot.bass.raw,
        snapshot.bass_flux,
        snapshot.treble.smooth
    );
    let first = fit(
        fonts,
        &first,
        (8.0 * scale) as f32,
        UiFace::Light,
        panel.width() - 16.0 * scale,
    );
    draw_left_center(
        scene,
        &first,
        panel.x0 + 8.0 * scale,
        footer + 5.0 * scale,
        palette.neutral[1],
    );

    let second = match hovered {
        Some(DebugSelection::Property(property)) => property.description().to_owned(),
        _ => format!(
            "HIT {:<4} {:.2} HF{} TOT {:>3}:{:<3} CD{:.2} {} L{} R{}",
            snapshot.last_hit.map_or("--", |hit| hit.kind),
            snapshot.last_hit.map_or(0.0, |hit| hit.strength),
            snapshot.hits_last_frame,
            snapshot.bass_hits,
            snapshot.broadband_hits,
            snapshot.transition_cooldown,
            snapshot.scene,
            snapshot.lines,
            snapshot.ribbons
        ),
    };
    let second = fit(
        fonts,
        &second,
        (8.0 * scale) as f32,
        UiFace::Light,
        panel.width() - 16.0 * scale,
    );
    draw_left_center(
        scene,
        &second,
        panel.x0 + 8.0 * scale,
        footer + 15.0 * scale,
        palette.neutral[1].with_alpha(0.75),
    );

    let third = format!(
        "AX{:+5.1},{:+5.1},{:+5.1} TW{:+5.1} DF{:>4.1} SH{:+5.1} ZM{:>4.1} PR{:>4.1}",
        snapshot.style.axis[0],
        snapshot.style.axis[1],
        snapshot.style.axis[2],
        snapshot.style.twist_sign,
        snapshot.style.deform_bias,
        snapshot.style.shear_sign,
        snapshot.style.zoom_bias,
        snapshot.style.phase_rate
    );
    let third = fit(
        fonts,
        &third,
        (8.0 * scale) as f32,
        UiFace::Light,
        panel.width() - 16.0 * scale,
    );
    draw_left_center(
        scene,
        &third,
        panel.x0 + 8.0 * scale,
        footer + 25.0 * scale,
        palette.neutral[1].with_alpha(0.65),
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_extension(
    fonts: &mut Fonts,
    scene: &mut Scene,
    layout: &Layout,
    rect: Rect,
    snapshot: &DebugSnapshot,
    history: &DebugHistory,
    visualizer: &Visualizer,
    palette: Palette,
    colors: MeterColors,
    selection: DebugSelection,
    graph_max: &[Option<GraphMax>; Property::COUNT],
) {
    let scale = layout.scale;
    if rect.height() < EXTENSION_MIN_HEIGHT * scale {
        return;
    }
    let color = selection.color(palette);
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        colors.background.with_alpha(0.94),
        None,
        &rect,
    );
    scene.stroke(
        &Stroke::new(1.0),
        Affine::IDENTITY,
        color.with_alpha(0.55),
        None,
        &rect,
    );
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color.with_alpha(0.85),
        None,
        &Rect::from_origin_size(
            Point::new(rect.x0, rect.y0),
            Size::new(2.0 * scale, rect.height()),
        ),
    );

    let inner = Rect::new(
        rect.x0 + 8.0 * scale,
        rect.y0 + 6.0 * scale,
        rect.x1 - 8.0 * scale,
        rect.y1 - 5.0 * scale,
    );
    let heading_height = EXTENSION_HEADING_HEIGHT * scale;
    let line_height = EXTENSION_LINE_HEIGHT * scale;
    let (heading, description, dependencies) = match selection {
        DebugSelection::Band(index) => {
            let band = &snapshot.bands[index];
            let onset = if index < 4 {
                "bass onset and wide onset"
            } else {
                "wide onset"
            };
            (
                format!(
                    "BAND {:02}  {:.0}-{:.0}Hz  {}",
                    index,
                    band.low_hz,
                    band.high_hz,
                    band.role.name()
                ),
                format!("Smoothed level and raw FFT area; it feeds the {onset}.",),
                format!(
                    "DRIVES {}",
                    band_property_values(snapshot, index, visualizer)
                ),
            )
        }
        DebugSelection::Property(property) => (
            format!(
                "PROPERTY {}  {:.2}",
                property.name(),
                snapshot.properties[property.index()]
            ),
            property.description().to_owned(),
            format!(
                "DEPENDS {}",
                visualizer
                    .property_dependencies(property)
                    .iter()
                    .map(|dependency| dependency_label(*dependency, visualizer.routing()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
    };

    let heading = fit(
        fonts,
        &heading,
        (10.0 * scale) as f32,
        UiFace::SemiBold,
        inner.width(),
    );
    draw_left_center(
        scene,
        &heading,
        inner.x0,
        inner.y0 + heading_height * 0.5,
        color,
    );
    let description = fit(
        fonts,
        &description,
        (8.5 * scale) as f32,
        UiFace::Light,
        inner.width(),
    );
    draw_left_center(
        scene,
        &description,
        inner.x0,
        inner.y0 + heading_height + line_height * 0.5,
        palette.neutral[0].with_alpha(0.9),
    );
    let dependencies = fit(
        fonts,
        &dependencies,
        (8.0 * scale) as f32,
        UiFace::Light,
        inner.width(),
    );
    draw_left_center(
        scene,
        &dependencies,
        inner.x0,
        inner.y0 + heading_height + line_height * 1.5,
        palette.neutral[1].with_alpha(0.85),
    );

    let selected_max = match selection {
        DebugSelection::Property(property) => graph_max[property.index()],
        DebugSelection::Band(_) => None,
    };
    let (cells, rows) = extension_graph_cells(rect, scale, selection, visualizer);
    for (source, cell) in cells {
        draw_graph(
            fonts,
            scene,
            cell,
            source,
            selection,
            snapshot,
            history,
            visualizer,
            palette,
            colors,
            scale,
            rows > 1,
            selected_max,
        );
    }
}

pub(super) fn tree_rect(view: &View, text_scale: VisualizerTextScale) -> Rect {
    property_panes(view, text_scale).0
}

/// Geometry for a selected property. The tree keeps its minimum width and the
/// inspector gives up width when the two cannot both fit.
fn property_panes(view: &View, text_scale: VisualizerTextScale) -> (Rect, Rect) {
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let margin = PANEL_MARGIN * scale;
    let layout = Layout::new(view, text_scale);
    let preferred_left = margin + TREE_LEFT_INSET * scale;
    let left = if layout.panel.x0 - preferred_left >= TREE_MIN_VISIBLE_WIDTH * scale {
        preferred_left
    } else {
        margin
    };
    let base = layout.extension;
    let gap = EXTENSION_GAP * scale;
    let minimum_right = (left + TREE_MIN_WIDTH * scale).min(layout.panel.x0 - gap);
    let right = if base.x0 - left >= TREE_MIN_WIDTH * scale + gap {
        base.x0 - gap
    } else {
        minimum_right.max(left + 1.0)
    };
    let tree = Rect::new(left, margin, right, base.y1);
    let extension = Rect::new(right + gap, base.y0, base.x1, base.y1);
    (tree, extension)
}

#[derive(Clone, Copy)]
pub(super) struct SliderRow {
    pub parameter: Parameter,
    pub track: Rect,
    hit: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TreeAction {
    ToggleMix(Contribution),
    AddExtra(MixPoint),
    RemoveExtra(ExtraRef),
    ToggleAdaptive(EnvelopeTarget),
}

#[derive(Clone, Copy)]
pub(super) struct ActionRow {
    pub action: TreeAction,
    rect: Rect,
}

pub(super) struct TreeLayout {
    pub rect: Rect,
    pub rows: Vec<TreeRow>,
    pub sliders: Vec<SliderRow>,
    pub actions: Vec<ActionRow>,
    parameter_title: String,
    parameter_panel: Rect,
}

impl TreeLayout {
    pub fn row_at(&self, point: Point) -> Option<&TreeRow> {
        self.rows.iter().find(|row| row.rect.contains(point))
    }

    pub fn slider_at(&self, point: Point) -> Option<SliderRow> {
        self.sliders
            .iter()
            .find(|slider| slider.hit.contains(point))
            .copied()
    }

    pub fn action_at(&self, point: Point) -> Option<TreeAction> {
        self.actions
            .iter()
            .find(|row| row.rect.contains(point))
            .map(|row| row.action)
    }

    #[cfg(test)]
    pub fn row_center_for(&self, target: TreeTarget) -> Point {
        self.rows
            .iter()
            .find(|row| row.target() == Some(target))
            .map(|row| row.rect.center())
            .expect("the tree has a row for the target")
    }

    #[cfg(test)]
    pub fn detector_center(&self, detector: Detector) -> Option<Point> {
        self.rows
            .iter()
            .find(|row| row.target() == Some(TreeTarget::Detector(detector)))
            .map(|row| row.rect.center())
    }

    #[cfg(test)]
    pub fn action_center(&self, action: TreeAction) -> Option<Point> {
        self.actions
            .iter()
            .find(|row| row.action == action)
            .map(|row| row.rect.center())
    }
}

#[derive(Clone, Copy)]
pub(super) enum Row {
    Static(&'static ComputeNode),
    Contribution(Contribution),
}

pub(super) struct TreeRow {
    pub row: Row,
    pub rect: Rect,
    depth: usize,
    parent: Option<usize>,
}

impl TreeRow {
    pub fn target(&self) -> Option<TreeTarget> {
        match self.row {
            Row::Static(node) => {
                if let Some(point) = node.mix_point {
                    return Some(TreeTarget::MixPoint(point));
                }
                if let Some(detector) = node
                    .detector
                    .or_else(|| node.input.and_then(Input::detector))
                {
                    return Some(TreeTarget::Detector(detector));
                }
                node.input.map(TreeTarget::Input)
            }
            Row::Contribution(contribution) => Some(TreeTarget::Contribution(contribution)),
        }
    }

    pub fn contribution(&self) -> Option<Contribution> {
        match self.row {
            Row::Contribution(contribution) => Some(contribution),
            Row::Static(_) => None,
        }
    }

    fn is_selectable(&self) -> bool {
        self.target().is_some()
    }
}

pub(super) fn tree_layout(
    view: &View,
    text_scale: VisualizerTextScale,
    property: Property,
    target: Option<TreeTarget>,
    mixer: &Mixer,
) -> TreeLayout {
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let rect = tree_rect(view, text_scale);
    let mut rows = Vec::new();
    flatten_tree(property.computation(), 0, None, &mut rows, mixer);
    let rows_top = rect.y0 + 25.0 * scale;
    let row_height = BAND_ROW_HEIGHT * scale;
    let rows_end = rows_top + rows.len() as f64 * row_height;
    for (index, row) in rows.iter_mut().enumerate() {
        row.rect = Rect::new(
            rect.x0,
            rows_top + index as f64 * row_height,
            rect.x1,
            rows_top + (index + 1) as f64 * row_height,
        );
    }
    let parameter_panel = Rect::new(
        rect.x0 + 8.0 * scale,
        rows_end + 5.0 * scale,
        rect.x1 - 8.0 * scale,
        rect.y1 - 5.0 * scale,
    );
    let (parameter_title, parameters, actions) = parameter_content(target, mixer);
    let action_rows = action_rows(parameter_panel, &actions, scale);
    let sliders = slider_rows(parameter_panel, &parameters, scale, action_rows.len());
    TreeLayout {
        rect,
        rows,
        sliders,
        actions: action_rows,
        parameter_title,
        parameter_panel,
    }
}

fn parameter_content(
    target: Option<TreeTarget>,
    mixer: &Mixer,
) -> (String, Vec<Parameter>, Vec<TreeAction>) {
    match target {
        Some(TreeTarget::Contribution(Contribution::Fixed(point, index))) => {
            let spec = point.contributions()[index];
            let mut parameters = spec.input().map(Input::parameters).unwrap_or_default();
            if let Some(gain) = spec.gain {
                parameters.push(Parameter::Gain(gain));
            }
            if let Some(style) = spec.style_scale {
                parameters.push(Parameter::Style(style));
            }
            let mut actions = vec![TreeAction::ToggleMix(Contribution::Fixed(point, index))];
            if let Some(shape) = spec.input().and_then(Input::shape_target) {
                parameters.push(Parameter::InputAmount(shape));
                parameters.push(Parameter::InputCurve(shape));
                actions.push(TreeAction::ToggleAdaptive(shape));
            }
            (spec.label.to_owned(), parameters, actions)
        }
        Some(TreeTarget::Contribution(Contribution::Extra(extra))) => {
            let input = mixer.extra(extra).input;
            let mut parameters = vec![Parameter::ExtraSource(extra), Parameter::ExtraGain(extra)];
            parameters.extend(input.parameters());
            let mut actions = vec![TreeAction::ToggleMix(Contribution::Extra(extra))];
            if let Some(shape) = input.shape_target() {
                parameters.push(Parameter::InputAmount(shape));
                parameters.push(Parameter::InputCurve(shape));
                actions.push(TreeAction::ToggleAdaptive(shape));
            }
            actions.push(TreeAction::RemoveExtra(extra));
            (format!("EXTRA {}", input.title()), parameters, actions)
        }
        Some(TreeTarget::MixPoint(point)) => (
            format!("{} MIX", point.title()),
            Vec::new(),
            vec![TreeAction::AddExtra(point)],
        ),
        Some(TreeTarget::Input(input)) => {
            let mut parameters = input.parameters();
            let mut actions = Vec::new();
            if let Some(shape) = input.shape_target() {
                parameters.push(Parameter::InputAmount(shape));
                parameters.push(Parameter::InputCurve(shape));
                actions.push(TreeAction::ToggleAdaptive(shape));
            }
            (input.title(), parameters, actions)
        }
        Some(TreeTarget::Detector(detector)) => (
            detector.name().to_owned(),
            detector.parameters().to_vec(),
            Vec::new(),
        ),
        None => (
            "SELECT A MIX POINT OR COMPONENT ROW".to_owned(),
            Vec::new(),
            Vec::new(),
        ),
    }
}

fn action_rows(panel: Rect, actions: &[TreeAction], scale: f64) -> Vec<ActionRow> {
    let title_height = 12.0 * scale;
    actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let top = panel.y0 + title_height + index as f64 * TREE_PARAM_ROW * scale;
            ActionRow {
                action: *action,
                rect: Rect::new(
                    panel.x0,
                    top + 1.0 * scale,
                    panel.x1,
                    top + (TREE_PARAM_ROW - 2.0) * scale,
                ),
            }
        })
        .collect()
}

fn slider_rows(
    panel: Rect,
    parameters: &[Parameter],
    scale: f64,
    rows_before: usize,
) -> Vec<SliderRow> {
    if parameters.is_empty() {
        return Vec::new();
    }
    let title_height = 12.0 * scale;
    let label_width = (panel.width() * 0.34).clamp(38.0 * scale, 132.0 * scale);
    // The extra-input picker shows a full input name, which needs more room
    // than a numeric parameter value.
    let value_width = if parameters
        .iter()
        .any(|parameter| matches!(parameter, Parameter::ExtraSource(_)))
    {
        86.0 * scale
    } else {
        42.0 * scale
    };
    parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let center = panel.y0
                + title_height
                + (rows_before + index) as f64 * TREE_PARAM_ROW * scale
                + TREE_PARAM_ROW * 0.5 * scale;
            let track = Rect::new(
                panel.x0 + label_width,
                center - SLIDER_TRACK_HEIGHT * 0.5 * scale,
                panel.x1 - value_width,
                center + SLIDER_TRACK_HEIGHT * 0.5 * scale,
            );
            SliderRow {
                parameter: *parameter,
                track,
                hit: Rect::new(
                    track.x0,
                    center - 8.0 * scale,
                    track.x1,
                    center + 8.0 * scale,
                ),
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn draw_computation_tree(
    fonts: &mut Fonts,
    scene: &mut Scene,
    view: &View,
    snapshot: &DebugSnapshot,
    visualizer: &Visualizer,
    palette: Palette,
    colors: MeterColors,
    property: Property,
    text_scale: VisualizerTextScale,
    target: Option<TreeTarget>,
    pointer: Option<Point>,
) {
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let layout = tree_layout(view, text_scale, property, target, visualizer.mixer());
    let rect = layout.rect;
    if rect.width() < TREE_MIN_VISIBLE_WIDTH * scale || rect.height() < 200.0 * scale {
        return;
    }
    let color = palette.vivid(property.index());
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        colors.background.with_alpha(0.94),
        None,
        &rect,
    );
    scene.stroke(
        &Stroke::new(1.0),
        Affine::IDENTITY,
        color.with_alpha(0.55),
        None,
        &rect,
    );
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color.with_alpha(0.85),
        None,
        &Rect::from_origin_size(
            Point::new(rect.x0, rect.y0),
            Size::new(2.0 * scale, rect.height()),
        ),
    );

    let heading = fit(
        fonts,
        &format!("COMPUTATION  {}", property.name()),
        (9.5 * scale) as f32,
        UiFace::SemiBold,
        rect.width() - 70.0 * scale,
    );
    draw_left_center(
        scene,
        &heading,
        rect.x0 + 8.0 * scale,
        rect.y0 + 8.0 * scale,
        color,
    );
    let value = fit(
        fonts,
        &format!("{:.2}", snapshot.properties[property.index()]),
        (9.0 * scale) as f32,
        UiFace::SemiBold,
        58.0 * scale,
    );
    draw_right_center(
        scene,
        &value,
        rect.x1 - 8.0 * scale,
        rect.y0 + 8.0 * scale,
        palette.neutral[0],
    );
    let subtitle = fit(
        fonts,
        "+ add   × mul   ≥ switch   click badge to flip, row to tune",
        (7.0 * scale) as f32,
        UiFace::LightItalic,
        rect.width() - 16.0 * scale,
    );
    draw_left_center(
        scene,
        &subtitle,
        rect.x0 + 8.0 * scale,
        rect.y0 + 18.0 * scale,
        palette.neutral[1].with_alpha(0.75),
    );

    let connector_x = |depth: usize| rect.x0 + 8.0 * scale + depth as f64 * TREE_INDENT * scale;
    let hovered = pointer.and_then(|point| layout.row_at(point));
    for row in &layout.rows {
        if let Some(parent) = row.parent {
            let Some(parent_row) = layout.rows.get(parent) else {
                continue;
            };
            let parent_x = connector_x(parent_row.depth);
            let child_x = connector_x(row.depth);
            let parent_y = parent_row.rect.center().y;
            let child_y = row.rect.center().y;
            scene.stroke(
                &Stroke::new(0.8 * scale),
                Affine::IDENTITY,
                color.with_alpha(0.28),
                None,
                &Line::new(
                    Point::new(parent_x, parent_y),
                    Point::new(parent_x, child_y),
                ),
            );
            scene.stroke(
                &Stroke::new(0.8 * scale),
                Affine::IDENTITY,
                color.with_alpha(0.28),
                None,
                &Line::new(Point::new(parent_x, child_y), Point::new(child_x, child_y)),
            );
        }
    }

    for row in &layout.rows {
        let selected = target.is_some_and(|target| row.target() == Some(target));
        let is_hovered = hovered.is_some_and(|hover| std::ptr::eq(hover, row));
        let highlight = target_color(row, palette, visualizer.routing(), visualizer.mixer());
        if selected || (is_hovered && row.is_selectable()) {
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                highlight.with_alpha(if selected { 0.2 } else { 0.1 }),
                None,
                &row.rect,
            );
            if selected {
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    highlight,
                    None,
                    &Rect::from_origin_size(
                        Point::new(row.rect.x0, row.rect.y0),
                        Size::new(2.0 * scale, row.rect.height()),
                    ),
                );
            }
        }
        let center = row.rect.center().y;
        let left = connector_x(row.depth) + 6.0 * scale;
        let right = rect.x1 - 8.0 * scale;
        let badge_width = 10.0 * scale;
        let badge_gap = 3.0 * scale;
        let op_width = (118.0 * scale).min(rect.width() * 0.34);
        let label_width =
            (right - left - op_width - badge_width - badge_gap - 4.0 * scale).max(30.0 * scale);
        let label = row_label(row, snapshot, visualizer.routing(), visualizer.mixer());
        let face = if row.depth == 0 {
            UiFace::SemiBold
        } else {
            UiFace::Light
        };
        let row_color = match row_input(row, visualizer.mixer()) {
            Some(input) if row.depth > 0 => input_color(input, palette, visualizer.routing()),
            _ if row.depth == 0 => color,
            _ => palette.neutral[0].with_alpha(0.9),
        };
        let label = fit(fonts, &label, (8.0 * scale) as f32, face, label_width);
        draw_left_center(scene, &label, left, center, row_color);
        let op_text = row_operation(row, visualizer);
        if !op_text.is_empty() {
            let op = fit(
                fonts,
                &op_text,
                (7.0 * scale) as f32,
                UiFace::LightItalic,
                op_width,
            );
            draw_right_center(
                scene,
                &op,
                right - badge_width - badge_gap,
                center,
                palette.neutral[1].with_alpha(0.78),
            );
        }
        if let Some(mix) = row_mix(row, visualizer.mixer()) {
            let badge = Rect::new(
                right - badge_width,
                row.rect.y0 + 1.0 * scale,
                right,
                row.rect.y1 - 1.0 * scale,
            );
            let badge_color = match mix {
                Mix::Add => palette.vivid(2),
                Mix::Mul => palette.vivid(9),
            };
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                badge_color.with_alpha(0.18),
                None,
                &badge,
            );
            scene.stroke(
                &Stroke::new(0.8 * scale),
                Affine::IDENTITY,
                badge_color.with_alpha(0.7),
                None,
                &badge,
            );
            let symbol = fonts.layout_ui(
                match mix {
                    Mix::Add => "+",
                    Mix::Mul => "×",
                },
                8.0 * scale as f32,
                UiFace::SemiBold,
            );
            draw_left_center(
                scene,
                &symbol,
                badge.center().x - symbol.width() * 0.5,
                center,
                badge_color,
            );
        }
    }

    let panel = layout.parameter_panel;
    if panel.height() > 0.0 {
        let shape = target
            .and_then(|target| match target {
                TreeTarget::Contribution(Contribution::Extra(extra)) => {
                    Some(visualizer.mixer().extra(extra).input)
                }
                _ => target_input(target),
            })
            .and_then(|input| visualizer.input_shape(input));
        draw_slider_content(
            fonts,
            scene,
            panel,
            &layout.parameter_title,
            &layout.sliders,
            &layout.actions,
            shape,
            visualizer,
            palette,
            colors,
            scale,
            pointer,
        );
    }
}

fn target_input(target: TreeTarget) -> Option<Input> {
    match target {
        TreeTarget::Contribution(Contribution::Fixed(point, index)) => {
            point.contributions()[index].input()
        }
        TreeTarget::Input(input) => Some(input),
        TreeTarget::Contribution(Contribution::Extra(_))
        | TreeTarget::MixPoint(_)
        | TreeTarget::Detector(_) => None,
    }
}

fn row_label(
    row: &TreeRow,
    snapshot: &DebugSnapshot,
    routing: &BandRouting,
    mixer: &Mixer,
) -> String {
    match row.row {
        Row::Static(node) => {
            let mut label = node.label.to_owned();
            if let Some(input) = node.input {
                if let Some(value) = input_value(input, snapshot, routing) {
                    label.push_str("  ");
                    label.push_str(&value);
                }
            }
            label
        }
        Row::Contribution(contribution) => {
            let (mut label, input) = match contribution {
                Contribution::Fixed(point, index) => {
                    let spec = point.contributions()[index];
                    (spec.label.to_owned(), spec.input())
                }
                Contribution::Extra(extra) => {
                    let extra = mixer.extra(extra);
                    (format!("EXTRA {}", extra.input.title()), Some(extra.input))
                }
            };
            if let Some(input) = input {
                if let Some(value) = input_value(input, snapshot, routing) {
                    label.push_str("  ");
                    label.push_str(&value);
                }
            }
            label
        }
    }
}

fn row_operation(row: &TreeRow, visualizer: &Visualizer) -> String {
    match row.row {
        Row::Static(node) => node.op.to_owned(),
        Row::Contribution(Contribution::Fixed(point, index)) => {
            let spec = point.contributions()[index];
            match spec.gain {
                Some(gain) => {
                    let value = visualizer.parameter_value(Parameter::Gain(gain));
                    spec.note
                        .replace("gain", &Parameter::Gain(gain).format(value))
                }
                None => spec.note.to_owned(),
            }
        }
        Row::Contribution(Contribution::Extra(extra)) => {
            let extra = visualizer.mixer().extra(extra);
            format!("×{:.2}", extra.gain)
        }
    }
}

fn row_mix(row: &TreeRow, mixer: &Mixer) -> Option<Mix> {
    match row.row {
        Row::Contribution(contribution) => Some(mixer.mode(contribution)),
        Row::Static(_) => None,
    }
}

fn row_input(row: &TreeRow, mixer: &Mixer) -> Option<Input> {
    match row.row {
        Row::Static(node) => node.input,
        Row::Contribution(Contribution::Fixed(point, index)) => {
            point.contributions()[index].input()
        }
        Row::Contribution(Contribution::Extra(extra)) => Some(mixer.extra(extra).input),
    }
}

fn row_detector(row: &TreeRow) -> Option<Detector> {
    match row.row {
        Row::Static(node) => node.detector,
        Row::Contribution(_) => None,
    }
}

fn target_color(row: &TreeRow, palette: Palette, routing: &BandRouting, mixer: &Mixer) -> Color {
    if let Some(detector) = row_detector(row) {
        return match detector {
            Detector::Bass => palette.vivid(6),
            Detector::Wide => palette.vivid(8),
        };
    }
    row_input(row, mixer).map_or(palette.neutral[1], |input| {
        input_color(input, palette, routing)
    })
}

fn flatten_tree(
    node: &'static ComputeNode,
    depth: usize,
    parent: Option<usize>,
    rows: &mut Vec<TreeRow>,
    mixer: &Mixer,
) {
    let index = rows.len();
    rows.push(TreeRow {
        row: Row::Static(node),
        rect: Rect::ZERO,
        depth,
        parent,
    });
    if let Some(point) = node.mix_point {
        debug_assert!(
            node.children.is_empty(),
            "mix point rows come from the point definition, not static children"
        );
        for (contribution, _) in point.contributions().iter().enumerate() {
            rows.push(TreeRow {
                row: Row::Contribution(Contribution::Fixed(point, contribution)),
                rect: Rect::ZERO,
                depth: depth + 1,
                parent: Some(index),
            });
        }
        for (extra, _) in mixer.extras(point) {
            rows.push(TreeRow {
                row: Row::Contribution(Contribution::Extra(extra)),
                rect: Rect::ZERO,
                depth: depth + 1,
                parent: Some(index),
            });
        }
        return;
    }
    for child in node.children {
        flatten_tree(child, depth + 1, Some(index), rows, mixer);
    }
}

fn parameter_label(parameter: Parameter, routing: &BandRouting) -> String {
    if let Parameter::Gain(gain) = parameter {
        if let Some(component) = gain.band_component() {
            return format!("B{:02} {} GAIN", routing[component], component.label());
        }
    }
    parameter.label().to_owned()
}

#[allow(clippy::too_many_arguments)]
fn action_label(action: TreeAction, visualizer: &Visualizer) -> String {
    match action {
        TreeAction::ToggleMix(contribution) => {
            format!("MIX MODE: {}", visualizer.mixer().mode(contribution).name())
        }
        TreeAction::AddExtra(point) => format!("+ ADD INPUT TO {}", point.title()),
        TreeAction::RemoveExtra(_) => "REMOVE EXTRA INPUT".to_owned(),
        TreeAction::ToggleAdaptive(target) => {
            let adaptive =
                envelope_shape(target, visualizer).is_some_and(|shape| shape.is_adaptive());
            format!("AUTO REFS: {}", if adaptive { "ON" } else { "OFF" })
        }
    }
}

fn envelope_shape(target: EnvelopeTarget, visualizer: &Visualizer) -> Option<InputShape> {
    let input = match target {
        EnvelopeTarget::Band(component) => Input::Band(component),
        EnvelopeTarget::Power => Input::Power,
        EnvelopeTarget::Bass => Input::Bass,
        EnvelopeTarget::Treble => return None,
    };
    visualizer.input_shape(input)
}

#[allow(clippy::too_many_arguments)]
fn draw_slider_content(
    fonts: &mut Fonts,
    scene: &mut Scene,
    panel: Rect,
    title: &str,
    sliders: &[SliderRow],
    actions: &[ActionRow],
    shape: Option<InputShape>,
    visualizer: &Visualizer,
    palette: Palette,
    colors: MeterColors,
    scale: f64,
    pointer: Option<Point>,
) {
    scene.stroke(
        &Stroke::new(1.0),
        Affine::IDENTITY,
        palette.neutral[2].with_alpha(0.35),
        None,
        &Line::new(
            Point::new(panel.x0, panel.y0 - 3.0 * scale),
            Point::new(panel.x1, panel.y0 - 3.0 * scale),
        ),
    );
    let title = fit(
        fonts,
        title,
        (8.0 * scale) as f32,
        UiFace::SemiBold,
        panel.width(),
    );
    draw_left_center(
        scene,
        &title,
        panel.x0,
        panel.y0 + 5.0 * scale,
        palette.neutral[0],
    );
    for action in actions {
        let hovered = pointer.is_some_and(|point| action.rect.contains(point));
        let fill = if hovered {
            colors.readout.with_alpha(0.24)
        } else {
            palette.neutral[0].with_alpha(0.08)
        };
        scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &action.rect);
        scene.stroke(
            &Stroke::new(0.8 * scale),
            Affine::IDENTITY,
            palette.neutral[2].with_alpha(0.45),
            None,
            &action.rect,
        );
        let label = fit(
            fonts,
            &action_label(action.action, visualizer),
            (7.0 * scale) as f32,
            UiFace::SemiBold,
            action.rect.width() - 8.0 * scale,
        );
        draw_left_center(
            scene,
            &label,
            action.rect.x0 + 4.0 * scale,
            action.rect.center().y,
            palette.neutral[0],
        );
    }
    if sliders.is_empty() {
        if actions.is_empty() {
            let hint = fit(
                fonts,
                "no adjustable parameters",
                (7.0 * scale) as f32,
                UiFace::LightItalic,
                panel.width(),
            );
            draw_left_center(
                scene,
                &hint,
                panel.x0,
                panel.y0 + 17.0 * scale,
                palette.neutral[1].with_alpha(0.7),
            );
        }
        return;
    }
    for slider in sliders {
        let center = slider.track.center().y;
        let (minimum, maximum) = slider.parameter.range();
        let value = visualizer
            .parameter_value(slider.parameter)
            .clamp(minimum, maximum);
        let fraction = f64::from(slider.parameter.fraction(value));
        let hovered = pointer.is_some_and(|point| slider.hit.contains(point));
        let label = fit(
            fonts,
            &parameter_label(slider.parameter, visualizer.routing()),
            (7.0 * scale) as f32,
            UiFace::Light,
            (slider.track.x0 - panel.x0 - 4.0 * scale).max(1.0),
        );
        draw_left_center(scene, &label, panel.x0, center, palette.neutral[1]);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            palette.neutral[0].with_alpha(0.16),
            None,
            &slider.track,
        );
        let knob_x = slider.track.x0 + slider.track.width() * fraction;
        let fill_color = if hovered {
            colors.readout
        } else {
            palette.vivid(9)
        };
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            fill_color.with_alpha(0.33),
            None,
            &Rect::new(slider.track.x0, slider.track.y0, knob_x, slider.track.y1),
        );
        scene.stroke(
            &Stroke::new(1.8 * scale).with_caps(Cap::Butt),
            Affine::IDENTITY,
            if hovered {
                palette.neutral[0]
            } else {
                palette.neutral[1]
            },
            None,
            &Line::new(
                Point::new(knob_x, slider.track.y0),
                Point::new(knob_x, slider.track.y1),
            ),
        );
        let value_width = (panel.x1 - slider.track.x1 - 2.0 * scale).max(20.0);
        let value = fit(
            fonts,
            &slider.parameter.format(value),
            (7.0 * scale) as f32,
            UiFace::SemiBold,
            value_width,
        );
        draw_right_center(
            scene,
            &value,
            panel.x1 - 2.0 * scale,
            center,
            palette.neutral[0],
        );
    }
    if let Some(shape) = shape {
        if let Some(last) = sliders.last() {
            let top = last.track.y1 + 6.0 * scale;
            let height = (CURVE_PREVIEW_HEIGHT * scale).min(panel.y1 - top);
            if height >= 24.0 * scale {
                let preview = Rect::new(last.track.x0, top, last.track.x1, top + height);
                draw_shape_preview(fonts, scene, preview, shape, palette, scale);
            }
        }
    }
}

fn draw_shape_preview(
    fonts: &mut Fonts,
    scene: &mut Scene,
    rect: Rect,
    shape: InputShape,
    palette: Palette,
    scale: f64,
) {
    let title = fit(
        fonts,
        if shape.is_adaptive() {
            "INPUT SHAPE  AUTO"
        } else {
            "INPUT SHAPE"
        },
        (7.0 * scale) as f32,
        UiFace::SemiBold,
        rect.width() * 0.55,
    );
    draw_left_center(
        scene,
        &title,
        rect.x0,
        rect.y0 + 5.0 * scale,
        palette.neutral[1],
    );
    let curve = fit(
        fonts,
        shape.curve.name(),
        (7.0 * scale) as f32,
        UiFace::LightItalic,
        rect.width() * 0.4,
    );
    draw_right_center(
        scene,
        &curve,
        rect.x1,
        rect.y0 + 5.0 * scale,
        palette.neutral[0],
    );
    let plot = Rect::new(
        rect.x0,
        rect.y0 + 10.0 * scale,
        rect.x1,
        rect.y1 - 1.0 * scale,
    );
    if plot.height() < 8.0 * scale {
        return;
    }
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.1),
        None,
        &plot,
    );
    scene.stroke(
        &Stroke::new(0.7 * scale),
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.08),
        None,
        &Line::new(
            Point::new(plot.x0, plot.center().y),
            Point::new(plot.x1, plot.center().y),
        ),
    );
    scene.stroke(
        &Stroke::new(0.7 * scale),
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.08),
        None,
        &Line::new(
            Point::new(plot.center().x, plot.y0),
            Point::new(plot.center().x, plot.y1),
        ),
    );
    scene.stroke(
        &Stroke::new(0.8 * scale),
        Affine::IDENTITY,
        palette.neutral[1].with_alpha(0.45),
        None,
        &Line::new(Point::new(plot.x0, plot.y1), Point::new(plot.x1, plot.y0)),
    );
    let mut path = BezPath::new();
    let samples = 48;
    let span = shape.high - shape.low;
    for index in 0..=samples {
        let fraction = index as f32 / samples as f32;
        let output = shape.apply(shape.low + span * fraction);
        let x = plot.x0 + plot.width() * f64::from(fraction);
        let y = plot.y1 - plot.height() * f64::from(output);
        if index == 0 {
            path.move_to(Point::new(x, y));
        } else {
            path.line_to(Point::new(x, y));
        }
    }
    scene.stroke(
        &Stroke::new(1.4 * scale),
        Affine::IDENTITY,
        palette.vivid(9),
        None,
        &path,
    );
}

fn input_value(input: Input, snapshot: &DebugSnapshot, routing: &BandRouting) -> Option<String> {
    match input {
        Input::Band(component) => {
            let index = routing[component];
            Some(format!("B{index:02} {:.2}", snapshot.bands[index].value))
        }
        Input::BassBands => {
            let max = snapshot.bands[..4]
                .iter()
                .map(|band| band.raw)
                .fold(0.0_f32, f32::max);
            Some(format!("max {max:.2}"))
        }
        Input::Power => Some(format!("{:.2}", snapshot.power.smooth)),
        Input::PowerLevel => Some(format!("{:.2}", snapshot.broadband_onset.level)),
        Input::Bass => Some(format!("{:.2}", snapshot.bass.smooth)),
        Input::BassLevel => Some(format!("{:.2}", snapshot.bass_onset.level)),
        Input::BassFlux => Some(format!("{:.2}", snapshot.bass_flux)),
        Input::Flux => Some(format!("{:.2}", snapshot.flux)),
        Input::Kick => Some(format!("{:.2}", snapshot.kick)),
        Input::BassOnset => Some(onset_state(&snapshot.bass_onset).to_owned()),
        Input::WideOnset => Some(onset_state(&snapshot.broadband_onset).to_owned()),
        Input::PhaseClock => Some(format!("{:.2}", snapshot.phase)),
        Input::Style(style) => Some(format!("{:.2}", style_value(style, snapshot))),
        Input::Property(property) => Some(format!("{:.2}", snapshot.properties[property.index()])),
        Input::Constant(value) => Some(value.to_owned()),
    }
}

fn style_value(style: StyleInput, snapshot: &DebugSnapshot) -> f32 {
    match style {
        StyleInput::AxisX => snapshot.style.axis[0],
        StyleInput::AxisY => snapshot.style.axis[1],
        StyleInput::AxisZ => snapshot.style.axis[2],
        StyleInput::PhaseRate => snapshot.style.phase_rate,
        StyleInput::TwistSign => snapshot.style.twist_sign,
        StyleInput::ShearSign => snapshot.style.shear_sign,
        StyleInput::DeformBias => snapshot.style.deform_bias,
        StyleInput::ZoomBias => snapshot.style.zoom_bias,
        StyleInput::JitterBias => snapshot.style.jitter_bias,
    }
}

fn onset_state(onset: &OnsetDebug) -> &'static str {
    if onset.cooldown > 0.0 {
        "COOLDOWN"
    } else if onset.level >= onset.threshold
        && onset.flux >= onset.minimum_flux
        && onset.level >= onset.baseline * onset.baseline_ratio + 0.015
    {
        "ARMED"
    } else {
        "IDLE"
    }
}

fn input_color(input: Input, palette: Palette, routing: &BandRouting) -> Color {
    match input {
        Input::Band(component) => palette.vivid(routing[component]),
        Input::BassBands | Input::Bass | Input::BassLevel | Input::BassFlux | Input::BassOnset => {
            palette.vivid(6)
        }
        Input::Power | Input::PowerLevel | Input::Flux | Input::WideOnset => palette.vivid(8),
        Input::Kick => palette.vivid(0),
        Input::PhaseClock => palette.vivid(3),
        Input::Property(property) => palette.vivid(property.index()),
        Input::Style(_) => palette.neutral[1],
        Input::Constant(_) => palette.neutral[2],
    }
}

/// Selectable upper bound for a property output graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GraphMax {
    One,
    Two,
    Four,
    Eight,
    Sixteen,
}

const GRAPH_MAX_COUNT: usize = 5;

impl GraphMax {
    pub const ALL: [Self; GRAPH_MAX_COUNT] =
        [Self::One, Self::Two, Self::Four, Self::Eight, Self::Sixteen];
    const MIN_SPAN: f32 = 0.01;

    fn value(self) -> f32 {
        match self {
            Self::One => 1.0,
            Self::Two => 2.0,
            Self::Four => 4.0,
            Self::Eight => 8.0,
            Self::Sixteen => 16.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::One => "1",
            Self::Two => "2",
            Self::Four => "4",
            Self::Eight => "8",
            Self::Sixteen => "16",
        }
    }

    /// A maximum is only usable when it leaves a visible span above the
    /// property's lower bound. Zoom and Morph both start at 1.0, for example.
    fn is_available(self, low: f32) -> bool {
        self.value() > low + Self::MIN_SPAN
    }
}

const GRAPH_MAX_BUTTON_HEIGHT: f64 = 7.0;
const GRAPH_MAX_BUTTON_GAP: f64 = 2.0;
/// Keep the five maximum buttons from collapsing into unclickable slivers.
const GRAPH_MAX_ROW_MIN_WIDTH: f64 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GraphSource {
    Band(usize),
    BandRole(BandComponent),
    Power,
    Bass,
    Kick,
    Property(Property),
}

impl GraphSource {
    fn has_raw(self) -> bool {
        matches!(
            self,
            Self::Band(_) | Self::BandRole(_) | Self::Power | Self::Bass
        )
    }

    fn range(self, graph_max: Option<GraphMax>) -> (f32, f32) {
        match self {
            Self::Band(_) | Self::BandRole(_) | Self::Power | Self::Bass => (0.0, 1.0),
            Self::Kick => (0.0, 1.5),
            Self::Property(property) => {
                let (low, high) = property.range();
                match graph_max {
                    Some(max) => (low, max.value().max(low + GraphMax::MIN_SPAN)),
                    None => (low, high),
                }
            }
        }
    }

    fn color(self, palette: Palette, routing: &BandRouting) -> Color {
        match self {
            Self::Band(index) => palette.vivid(index),
            Self::BandRole(component) => palette.vivid(routing[component]),
            Self::Power => palette.vivid(8),
            Self::Bass => palette.vivid(6),
            Self::Kick => palette.vivid(0),
            Self::Property(property) => palette.vivid(property.index()),
        }
    }

    fn title(self, snapshot: &DebugSnapshot, routing: &BandRouting) -> String {
        match self {
            Self::Band(index) => format!(
                "B{:02} {} {:.2}",
                index,
                snapshot.bands[index].role.name(),
                snapshot.bands[index].value
            ),
            Self::BandRole(component) => {
                let index = routing[component];
                format!(
                    "B{:02} {} {:.2}",
                    index,
                    component.label(),
                    snapshot.bands[index].value
                )
            }
            Self::Power => format!(
                "PWR {:.2} THR{:.2}",
                snapshot.power.smooth, snapshot.broadband_onset.threshold
            ),
            Self::Bass => format!(
                "BASS {:.2} THR{:.2}",
                snapshot.bass.smooth, snapshot.bass_onset.threshold
            ),
            Self::Kick => format!("KICK {:.2}", snapshot.kick),
            Self::Property(property) => format!(
                "{} {:.2}",
                property.name(),
                snapshot.properties[property.index()]
            ),
        }
    }

    fn value(self, sample: &DebugSample, raw: bool, routing: &BandRouting) -> Option<f32> {
        match self {
            Self::Band(index) => Some(if raw {
                sample.raw_bands[index]
            } else {
                sample.bands[index]
            }),
            Self::BandRole(component) => {
                let index = routing[component];
                Some(if raw {
                    sample.raw_bands[index]
                } else {
                    sample.bands[index]
                })
            }
            Self::Power => Some(if raw { sample.raw_power } else { sample.power }),
            Self::Bass => Some(if raw { sample.raw_bass } else { sample.bass }),
            Self::Kick => (!raw).then_some(sample.kick),
            Self::Property(property) => (!raw).then(|| sample.property(property)),
        }
    }

    fn baseline(self, sample: &DebugSample) -> Option<f32> {
        match self {
            Self::Power => Some(sample.wide_onset_baseline),
            Self::Bass => Some(sample.bass_onset_baseline),
            _ => None,
        }
    }

    fn threshold(self, sample: &DebugSample) -> Option<f32> {
        match self {
            Self::Power => Some(sample.wide_onset_threshold),
            Self::Bass => Some(sample.bass_onset_threshold),
            _ => None,
        }
    }
}

fn graph_rows(count: usize) -> usize {
    if count >= 4 {
        2
    } else {
        1
    }
}

fn graph_sources(selection: DebugSelection, visualizer: &Visualizer) -> Vec<GraphSource> {
    let mut graphs = Vec::new();
    let mut push = |source: GraphSource| {
        if !graphs.contains(&source) {
            graphs.push(source);
        }
    };
    match selection {
        DebugSelection::Band(index) => {
            push(GraphSource::Band(index));
            if index < 4 {
                push(GraphSource::Bass);
            }
            push(GraphSource::Power);
        }
        DebugSelection::Property(property) => {
            push(GraphSource::Property(property));
            for dependency in visualizer.property_dependencies(property) {
                match dependency {
                    Dependency::Band(component) => push(GraphSource::BandRole(component)),
                    Dependency::BassBands => {
                        for index in 0..4 {
                            push(GraphSource::Band(index));
                        }
                    }
                    Dependency::Power | Dependency::WideOnset => push(GraphSource::Power),
                    Dependency::Bass | Dependency::BassOnset => push(GraphSource::Bass),
                    Dependency::Kick => push(GraphSource::Kick),
                }
            }
        }
    }
    graphs
}

/// Graph cells inside the inspector extension, shared by drawing and hit
/// testing so the bottom maximum buttons stay under their visible rectangles.
fn extension_graph_cells(
    rect: Rect,
    scale: f64,
    selection: DebugSelection,
    visualizer: &Visualizer,
) -> (Vec<(GraphSource, Rect)>, usize) {
    if rect.height() < EXTENSION_MIN_HEIGHT * scale {
        return (Vec::new(), 0);
    }
    let inner = Rect::new(
        rect.x0 + 8.0 * scale,
        rect.y0 + 6.0 * scale,
        rect.x1 - 8.0 * scale,
        rect.y1 - 5.0 * scale,
    );
    let graphs_top = inner.y0
        + EXTENSION_HEADING_HEIGHT * scale
        + EXTENSION_LINE_HEIGHT * 2.0 * scale
        + 2.0 * scale;
    let graphs = graph_sources(selection, visualizer);
    if graphs.is_empty() || graphs_top >= inner.y1 {
        return (Vec::new(), 0);
    }
    let gap = 4.0 * scale;
    let rows = graph_rows(graphs.len());
    let first_columns = graphs.len().div_ceil(rows);
    let row_height = (inner.y1 - graphs_top - gap * (rows - 1) as f64) / rows as f64;
    let mut cells = Vec::with_capacity(graphs.len());
    for row in 0..rows {
        let start = row * first_columns;
        let count = first_columns.min(graphs.len() - start);
        let width = (inner.width() - gap * (count - 1) as f64) / count as f64;
        for column in 0..count {
            let index = start + column;
            let left = inner.x0 + column as f64 * (width + gap);
            let top = graphs_top + row as f64 * (row_height + gap);
            cells.push((
                graphs[index],
                Rect::from_origin_size(Point::new(left, top), Size::new(width, row_height)),
            ));
        }
    }
    (cells, rows)
}

fn graph_max_buttons(cell: Rect, scale: f64) -> Option<[Rect; GRAPH_MAX_COUNT]> {
    // Leave the title/usage line and a visible plot above the button row.
    if cell.height() < 30.0 * scale {
        return None;
    }
    let left = cell.x0 + 2.0 * scale;
    let right = cell.x1 - 2.0 * scale;
    if right - left < GRAPH_MAX_ROW_MIN_WIDTH * scale {
        return None;
    }
    let bottom = cell.y1 - 2.0 * scale;
    let top = bottom - GRAPH_MAX_BUTTON_HEIGHT * scale;
    let gap = GRAPH_MAX_BUTTON_GAP * scale;
    let width =
        (right - left - gap * (GraphMax::ALL.len() - 1) as f64) / GraphMax::ALL.len() as f64;
    if width < 6.0 * scale {
        return None;
    }
    Some(std::array::from_fn(|index| {
        let x = left + index as f64 * (width + gap);
        Rect::new(x, top, x + width, bottom)
    }))
}

fn graph_max_buttons_for(
    view: &View,
    text_scale: VisualizerTextScale,
    selection: DebugSelection,
    visualizer: &Visualizer,
) -> Option<(Property, [Rect; GRAPH_MAX_COUNT])> {
    let DebugSelection::Property(property) = selection else {
        return None;
    };
    let rect = property_panes(view, text_scale).1;
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let (cells, _) = extension_graph_cells(rect, scale, selection, visualizer);
    let (_, cell) = cells
        .into_iter()
        .find(|(source, _)| *source == GraphSource::Property(property))?;
    Some((property, graph_max_buttons(cell, scale)?))
}

pub(super) fn graph_max_button_at(
    view: &View,
    text_scale: VisualizerTextScale,
    selection: DebugSelection,
    visualizer: &Visualizer,
    position: Point,
) -> Option<(Property, GraphMax)> {
    let (property, buttons) = graph_max_buttons_for(view, text_scale, selection, visualizer)?;
    let (low, _) = property.range();
    GraphMax::ALL
        .iter()
        .copied()
        .zip(buttons)
        .find(|(max, rect)| max.is_available(low) && rect.contains(position))
        .map(|(max, _)| (property, max))
}

#[cfg(test)]
pub(super) fn graph_max_button_center(
    view: &View,
    text_scale: VisualizerTextScale,
    selection: DebugSelection,
    visualizer: &Visualizer,
    max: GraphMax,
) -> Option<Point> {
    let (property, buttons) = graph_max_buttons_for(view, text_scale, selection, visualizer)?;
    let (low, _) = property.range();
    if !max.is_available(low) {
        return None;
    }
    let index = GraphMax::ALL
        .iter()
        .position(|candidate| *candidate == max)?;
    Some(buttons[index].center())
}

/// How one graph's result reaches the selected property. The captions are
/// deliberately terse: graph cells are narrow, so each one names the single
/// contribution that matters rather than repeating the property description.
fn graph_usage(selection: DebugSelection, source: GraphSource) -> &'static str {
    match selection {
        DebugSelection::Band(_) => match source {
            GraphSource::Band(_) | GraphSource::BandRole(_) => "smoothed + raw FFT",
            GraphSource::Bass => "switch: level & flux, gates KICK",
            GraphSource::Power => "switch: wide level & flux, fires hit",
            GraphSource::Kick | GraphSource::Property(_) => "",
        },
        DebugSelection::Property(property) => match source {
            GraphSource::Property(_) => "resulting value",
            GraphSource::Band(_) => match property {
                Property::Pulse => "max → bass envelope",
                Property::Kick => "max → bass level",
                _ => "feeds the property",
            },
            GraphSource::BandRole(component) => match component {
                BandComponent::Zoom => "×gain add → zoom",
                BandComponent::Spin => "×gain add, ×PHASE_RATE",
                BandComponent::Pitch => "×gain ×AXIS_Y add",
                BandComponent::Roll => "×gain ×AXIS_Z add",
                BandComponent::Twist => "×gain ×DEFORM ×SIGN",
                BandComponent::Bulge | BandComponent::Ripple => "×gain ×DEFORM",
                BandComponent::Shear => "×gain ×SIGN",
                BandComponent::Accent => "clamp; tint at spawn",
                BandComponent::Line => "0.6 + gain×value",
                BandComponent::Jitter => "×gain ×BIAS",
                BandComponent::Phase => "×gain add, ×PHASE_RATE",
            },
            GraphSource::Power => match property {
                Property::Pulse => "×gain add to energy",
                Property::Spin => "×gain add, ×PHASE_RATE",
                Property::Pitch => "rate ×gain add",
                Property::Roll => "rate ×gain add",
                Property::Phase => "×gain add, ×PHASE_RATE",
                Property::Crossfade => "×gain add rate; wide switch",
                Property::Morph => "×gain add 1.1",
                Property::Entrance => "rate ×gain add 1.4",
                _ => "feeds the property",
            },
            GraphSource::Bass => match property {
                Property::Pulse => "×gain add to energy",
                Property::Kick => "switch + strength add",
                Property::Crossfade => "switch: gates hits",
                _ => "feeds the property",
            },
            GraphSource::Kick => match property {
                Property::Pulse => "×gain add to energy",
                Property::Crossfade => "×gain add to rate",
                _ => "feeds the property",
            },
        },
    }
}

fn graph_usage_text(
    selection: DebugSelection,
    source: GraphSource,
    visualizer: &Visualizer,
) -> String {
    let usage = graph_usage(selection, source);
    let DebugSelection::Property(property) = selection else {
        return usage.to_owned();
    };
    let input = match source {
        GraphSource::Band(_) => return usage.to_owned(),
        GraphSource::BandRole(component) => Input::Band(component),
        GraphSource::Power => Input::Power,
        GraphSource::Bass => Input::Bass,
        GraphSource::Kick => Input::Kick,
        GraphSource::Property(_) => return usage.to_owned(),
    };
    let gains = property.component_gains(input);
    let Some(gain) = gains.first() else {
        return usage.to_owned();
    };
    let value = visualizer.parameter_value(Parameter::Gain(*gain));
    usage.replace("gain", &Parameter::Gain(*gain).format(value))
}

#[allow(clippy::too_many_arguments)]
fn draw_graph(
    fonts: &mut Fonts,
    scene: &mut Scene,
    rect: Rect,
    source: GraphSource,
    selection: DebugSelection,
    snapshot: &DebugSnapshot,
    history: &DebugHistory,
    visualizer: &Visualizer,
    palette: Palette,
    colors: MeterColors,
    scale: f64,
    compact: bool,
    graph_max: Option<GraphMax>,
) {
    if rect.width() < 12.0 * scale || rect.height() < 12.0 * scale {
        return;
    }
    let graph_max = match (selection, source) {
        (DebugSelection::Property(selected), GraphSource::Property(property))
            if selected == property =>
        {
            graph_max
        }
        _ => None,
    };
    let routing = visualizer.routing();
    let color = source.color(palette, routing);
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        colors.background.with_alpha(0.55),
        None,
        &rect,
    );
    scene.stroke(
        &Stroke::new(1.0),
        Affine::IDENTITY,
        color.with_alpha(0.3),
        None,
        &rect,
    );
    let usage = graph_usage_text(selection, source, visualizer);
    let title_size = if compact { 7.0 } else { 7.5 };
    let usage_width = if compact {
        rect.width() * 0.45
    } else {
        rect.width() - 4.0 * scale
    };
    let title = fit(
        fonts,
        &source.title(snapshot, routing),
        (title_size * scale) as f32,
        UiFace::Light,
        rect.width() - usage_width - 4.0 * scale,
    );
    draw_left_center(
        scene,
        &title,
        rect.x0 + 2.0 * scale,
        rect.y0 + 4.5 * scale,
        color,
    );
    let plot_top = if compact {
        let usage = fit(
            fonts,
            &usage,
            (6.5 * scale) as f32,
            UiFace::LightItalic,
            usage_width.max(1.0),
        );
        draw_right_center(
            scene,
            &usage,
            rect.x1 - 2.0 * scale,
            rect.y0 + 4.5 * scale,
            palette.neutral[1].with_alpha(0.72),
        );
        rect.y0 + 9.0 * scale
    } else if rect.height() >= 26.0 * scale {
        let usage = fit(
            fonts,
            &usage,
            (7.0 * scale) as f32,
            UiFace::LightItalic,
            rect.width() - 4.0 * scale,
        );
        draw_left_center(
            scene,
            &usage,
            rect.x0 + 2.0 * scale,
            rect.y0 + 11.5 * scale,
            palette.neutral[1].with_alpha(0.72),
        );
        rect.y0 + 16.0 * scale
    } else {
        rect.y0 + 9.0 * scale
    };
    let max_buttons = if matches!(source, GraphSource::Property(_)) {
        graph_max_buttons(rect, scale)
    } else {
        None
    };
    let plot_bottom =
        max_buttons.map_or(rect.y1 - 2.0 * scale, |buttons| buttons[0].y0 - 1.0 * scale);
    let plot = Rect::new(
        rect.x0 + 2.0 * scale,
        plot_top,
        rect.x1 - 2.0 * scale,
        plot_bottom,
    );
    if plot.height() < 2.0 * scale {
        return;
    }
    let range = source.range(graph_max);
    scene.stroke(
        &Stroke::new(0.7),
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.08),
        None,
        &Line::new(
            Point::new(plot.x0, plot.center().y),
            Point::new(plot.x1, plot.center().y),
        ),
    );

    if let Some(threshold) = source.threshold(history.latest().unwrap_or(&EMPTY_SAMPLE)) {
        draw_level(
            scene,
            plot,
            range,
            threshold,
            color.with_alpha(0.7),
            1.0 * scale,
        );
    }
    if let Some(baseline) = source.baseline(history.latest().unwrap_or(&EMPTY_SAMPLE)) {
        if let Some(start) = history.samples().next() {
            draw_trace(
                scene,
                history,
                plot,
                range,
                TraceStyle {
                    color: palette.neutral[0].with_alpha(0.35),
                    width: 0.8 * scale,
                    start: start.at,
                },
                |sample| source.baseline(sample).unwrap_or(baseline).max(0.0),
            );
        }
    }
    if source.has_raw() {
        draw_trace(
            scene,
            history,
            plot,
            range,
            TraceStyle {
                color: color.with_alpha(0.35),
                width: 0.8 * scale,
                start: 0.0,
            },
            |sample| source.value(sample, true, routing).unwrap_or(0.0),
        );
    }
    draw_trace(
        scene,
        history,
        plot,
        range,
        TraceStyle {
            color,
            width: 1.2 * scale,
            start: 0.0,
        },
        |sample| source.value(sample, false, routing).unwrap_or(0.0),
    );
    if let (GraphSource::Property(property), Some(buttons)) = (source, max_buttons) {
        draw_graph_max_buttons(
            fonts, scene, property, buttons, graph_max, color, palette, scale,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_graph_max_buttons(
    fonts: &mut Fonts,
    scene: &mut Scene,
    property: Property,
    buttons: [Rect; GRAPH_MAX_COUNT],
    selected: Option<GraphMax>,
    color: Color,
    palette: Palette,
    scale: f64,
) {
    let (low, _) = property.range();
    for (max, rect) in GraphMax::ALL.into_iter().zip(buttons) {
        let enabled = max.is_available(low);
        let active = selected == Some(max);
        let fill = if active {
            color.with_alpha(0.3)
        } else if enabled {
            palette.neutral[0].with_alpha(0.08)
        } else {
            palette.neutral[0].with_alpha(0.03)
        };
        scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &rect);
        let stroke = if active {
            color.with_alpha(0.9)
        } else if enabled {
            palette.neutral[2].with_alpha(0.45)
        } else {
            palette.neutral[2].with_alpha(0.18)
        };
        scene.stroke(
            &Stroke::new(0.7 * scale),
            Affine::IDENTITY,
            stroke,
            None,
            &rect,
        );
        let text = if active {
            color
        } else if enabled {
            palette.neutral[1]
        } else {
            palette.neutral[1].with_alpha(0.25)
        };
        let label = fit(
            fonts,
            max.label(),
            (6.0 * scale) as f32,
            UiFace::SemiBold,
            (rect.width() - 2.0 * scale).max(1.0),
        );
        draw_left_center(
            scene,
            &label,
            rect.center().x - label.width() * 0.5,
            rect.center().y,
            text,
        );
    }
}

const EMPTY_SAMPLE: DebugSample = DebugSample {
    at: 0.0,
    bands: [0.0; BAND_COUNT],
    raw_bands: [0.0; BAND_COUNT],
    power: 0.0,
    raw_power: 0.0,
    bass: 0.0,
    raw_bass: 0.0,
    kick: 0.0,
    bass_onset_baseline: 0.0,
    bass_onset_threshold: 0.0,
    wide_onset_baseline: 0.0,
    wide_onset_threshold: 0.0,
    properties: [0.0; Property::COUNT],
};

struct TraceStyle {
    color: Color,
    width: f64,
    /// `start` overrides the left edge when a trace only applies to a suffix of
    /// the history; zero means use the oldest retained sample.
    start: f32,
}

fn draw_trace(
    scene: &mut Scene,
    history: &DebugHistory,
    plot: Rect,
    range: (f32, f32),
    style: TraceStyle,
    value: impl Fn(&DebugSample) -> f32,
) {
    let Some((first, last)) = history.span() else {
        return;
    };
    let start = style.start.max(first);
    if last <= start {
        return;
    }
    let span = last - start;
    let mut path = BezPath::new();
    let mut count = 0;
    let mut endpoint = None;
    for sample in history.samples() {
        if sample.at < start {
            continue;
        }
        let x = plot.x0 + f64::from((sample.at - start) / span) * plot.width();
        let y = map_level(plot, range, value(sample));
        if count == 0 {
            path.move_to(Point::new(x, y));
        } else {
            path.line_to(Point::new(x, y));
        }
        endpoint = Some(Point::new(x, y));
        count += 1;
    }
    if count >= 2 {
        scene.stroke(
            &Stroke::new(style.width),
            Affine::IDENTITY,
            style.color,
            None,
            &path,
        );
    }
    if let Some(point) = endpoint {
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            style.color,
            None,
            &Circle::new(point, (style.width * 1.4).max(1.2)),
        );
    }
}

fn draw_level(
    scene: &mut Scene,
    plot: Rect,
    range: (f32, f32),
    value: f32,
    color: Color,
    width: f64,
) {
    let y = map_level(plot, range, value);
    if y < plot.y0 || y > plot.y1 {
        return;
    }
    scene.stroke(
        &Stroke::new(width),
        Affine::IDENTITY,
        color,
        None,
        &Line::new(Point::new(plot.x0, y), Point::new(plot.x1, y)),
    );
}

fn map_level(plot: Rect, range: (f32, f32), value: f32) -> f64 {
    let span = (range.1 - range.0).max(1e-6);
    let normalized = ((value - range.0) / span).clamp(0.0, 1.0);
    plot.y1 - f64::from(normalized) * plot.height()
}

fn draw_row_highlight(
    scene: &mut Scene,
    rect: Rect,
    color: Color,
    hovered: bool,
    selected: bool,
    scale: f64,
) {
    if !hovered && !selected {
        return;
    }
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color.with_alpha(if selected { 0.2 } else { 0.1 }),
        None,
        &rect,
    );
    if selected {
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            color,
            None,
            &Rect::from_origin_size(
                Point::new(rect.x0, rect.y0),
                Size::new(2.0 * scale, rect.height()),
            ),
        );
    }
}

fn draw_left_center(scene: &mut Scene, text: &Text, x: f64, center: f64, color: Color) {
    let baseline = center + text.cap_height() * 0.5;
    text.draw_left(scene, Affine::translate((x, baseline)), color);
}

fn draw_right_center(scene: &mut Scene, text: &Text, right: f64, center: f64, color: Color) {
    let baseline = center + text.cap_height() * 0.5;
    text.draw_left(
        scene,
        Affine::translate((right - text.width(), baseline)),
        color,
    );
}

/// Lays out one line, shortening it with a trailing ellipsis when it would
/// cross into the next column.
fn fit(fonts: &mut Fonts, text: &str, size: f32, face: UiFace, max_width: f64) -> Text {
    let layout = fonts.layout_ui(text, size, face);
    if layout.width() <= max_width || max_width <= 0.0 {
        return layout;
    }
    let ratio = max_width / layout.width();
    let keep = ((text.chars().count() as f64 * ratio).floor() as usize).min(text.chars().count());
    let mut shortened: String = text.chars().take(keep).collect();
    shortened.push_str("..");
    while !shortened.is_empty() {
        let layout = fonts.layout_ui(&shortened, size, face);
        if layout.width() <= max_width {
            return layout;
        }
        shortened.pop();
    }
    fonts.layout_ui("", size, face)
}

fn band_dependency_line(band: usize, visualizer: &Visualizer) -> String {
    Property::VARIANTS
        .iter()
        .copied()
        .filter_map(|property| {
            let route = band_route(property, band, visualizer);
            (!route.is_empty()).then(|| format!("{route}:{}", property.name()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn band_route(property: Property, band: usize, visualizer: &Visualizer) -> String {
    let routing = visualizer.routing();
    let dependencies = visualizer.property_dependencies(property);
    let mut route = String::new();
    if dependencies.iter().any(|dependency| match dependency {
        Dependency::Band(component) => routing[*component] == band,
        Dependency::BassBands => band < 4,
        _ => false,
    }) {
        route.push('V');
    }
    if band < 4 && dependencies.contains(&Dependency::BassOnset) {
        route.push('B');
    }
    if dependencies.contains(&Dependency::WideOnset) {
        route.push('W');
    }
    route
}

fn band_property_values(snapshot: &DebugSnapshot, band: usize, visualizer: &Visualizer) -> String {
    Property::VARIANTS
        .iter()
        .copied()
        .filter_map(|property| {
            let route = band_route(property, band, visualizer);
            (!route.is_empty()).then(|| {
                format!(
                    "{}:{} {:.2}",
                    route,
                    property.name(),
                    snapshot.properties[property.index()]
                )
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn dependency_label(dependency: Dependency, routing: &BandRouting) -> String {
    match dependency {
        Dependency::Band(component) => format!("B{:02}", routing[component]),
        Dependency::BassBands => "B00-03".to_owned(),
        Dependency::Power => "PWR".to_owned(),
        Dependency::Bass => "BASS".to_owned(),
        Dependency::Kick => "KICK".to_owned(),
        Dependency::BassOnset => "B-ONSET".to_owned(),
        Dependency::WideOnset => "W-ONSET".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        let view = View::fit(Size::new(1280.0, 440.0));
        Layout::new(&view, VisualizerTextScale::default())
    }

    #[test]
    fn every_band_and_property_row_has_a_hit_target() {
        let layout = layout();
        for index in 0..BAND_COUNT {
            assert_eq!(
                layout.item_at(layout.band_center(index)),
                Some(DebugSelection::Band(index))
            );
        }
        for &property in Property::VARIANTS {
            assert_eq!(
                layout.item_at(layout.property_center(property)),
                Some(DebugSelection::Property(property))
            );
        }
        assert_eq!(layout.item_at(Point::new(0.0, 0.0)), None);
    }

    #[test]
    fn inspector_sits_below_the_panel_and_fits_the_viewport() {
        let layout = layout();
        assert!(layout.extension.y0 >= layout.panel.y1);
        assert!(layout.extension.y1 <= 440.0);
        assert!(layout.extension.x0 >= 0.0);
        assert!(layout.extension.x1 <= 1280.0);
    }

    #[test]
    fn computation_tree_sits_left_of_the_analysis_panel() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let tree = tree_rect(&view, VisualizerTextScale::default());
        let layout = Layout::new(&view, VisualizerTextScale::default());
        let (_, inspector) = property_panes(&view, VisualizerTextScale::default());
        assert!(tree.x1 <= layout.panel.x0);
        assert_eq!(inspector.x0 - tree.x1, EXTENSION_GAP);
        assert_eq!(tree.y1, layout.extension.y1);
        assert!(tree.x0 > 150.0);
        assert!(tree.width() > 300.0);
        assert!(tree.height() > 400.0);
    }

    #[test]
    fn band_gain_labels_follow_the_selected_source_band() {
        use crate::visualizer::Gain;
        let mut routing = BandRouting::default();
        let gain = Parameter::Gain(Gain::SpinBand);
        assert_eq!(parameter_label(gain, &routing), "B04 SPIN GAIN");
        routing[BandComponent::Spin] = 7;
        assert_eq!(parameter_label(gain, &routing), "B07 SPIN GAIN");
    }

    #[test]
    fn detector_controls_live_in_the_tree_parameter_panel() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let tree = tree_layout(
            &view,
            text_scale,
            Property::Crossfade,
            Some(TreeTarget::Detector(Detector::Wide)),
            &Mixer::default(),
        );
        assert_eq!(tree.sliders.len(), 4);
        assert_eq!(
            tree.sliders
                .iter()
                .map(|slider| slider.parameter)
                .collect::<Vec<_>>(),
            Detector::Wide.parameters().to_vec()
        );
    }

    #[test]
    fn extra_mix_inputs_have_rows_and_actions() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let point = MixPoint::PulseEnergy;
        let mut mixer = Mixer::default();
        let extra = mixer.add_extra(point);
        let extra_target = TreeTarget::Contribution(Contribution::Extra(extra));

        let tree = tree_layout(
            &view,
            text_scale,
            Property::Pulse,
            Some(TreeTarget::MixPoint(point)),
            &mixer,
        );
        assert!(tree
            .rows
            .iter()
            .any(|row| row.target() == Some(extra_target)));
        assert!(tree.action_center(TreeAction::AddExtra(point)).is_some());

        let tree = tree_layout(
            &view,
            text_scale,
            Property::Pulse,
            Some(extra_target),
            &mixer,
        );
        assert!(tree.action_center(TreeAction::RemoveExtra(extra)).is_some());
        assert!(tree
            .action_center(TreeAction::ToggleMix(Contribution::Extra(extra)))
            .is_some());
    }

    #[test]
    fn tight_windows_shrink_the_inspector_before_hiding_the_tree() {
        let view = View::fit(Size::new(900.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let layout = Layout::new(&view, text_scale);
        let (tree, inspector) = property_panes(&view, text_scale);
        assert!(tree.width() > 250.0);
        assert!(inspector.width() < layout.extension.width());
        assert_eq!(inspector.x0 - tree.x1, EXTENSION_GAP);
        assert_eq!(tree.y1, inspector.y1);
        assert!(tree.x1 <= layout.panel.x0);
    }

    #[test]
    fn band_routes_name_value_and_onset_dependencies() {
        let visualizer = Visualizer::new();
        let routing = visualizer.routing();
        assert_eq!(band_route(Property::Pulse, 0, &visualizer), "V");
        assert_eq!(band_route(Property::Kick, 0, &visualizer), "VB");
        assert_eq!(band_route(Property::Crossfade, 0, &visualizer), "BW");
        assert_eq!(band_route(Property::Crossfade, 4, &visualizer), "W");
        assert_eq!(band_route(Property::Spin, 4, &visualizer), "V");
        assert_eq!(band_route(Property::Spin, 5, &visualizer), "");
        assert!(Property::Kick.depends_on_band(3, routing));
        assert!(!Property::Kick.depends_on_band(4, routing));
    }

    #[test]
    fn band_dependency_text_follows_rerouting() {
        let mut visualizer = Visualizer::new();
        assert!(band_route(Property::Spin, 4, &visualizer) == "V");
        assert!(band_route(Property::Spin, 7, &visualizer).is_empty());
        visualizer.set_parameter(Parameter::BandSource(BandComponent::Spin), 7.0);
        assert!(band_route(Property::Spin, 4, &visualizer).is_empty());
        assert_eq!(band_route(Property::Spin, 7, &visualizer), "V");
        assert!(band_dependency_line(7, &visualizer).contains("V:SPIN"));
    }

    #[test]
    fn extra_inputs_extend_the_band_dependencies() {
        let mut visualizer = Visualizer::new();
        let extra = visualizer.add_extra(MixPoint::SpinSpeed);
        let source = Input::Band(BandComponent::Bulge);
        visualizer.set_parameter(
            Parameter::ExtraSource(extra),
            source.selectable_index().expect("bulge is selectable") as f32,
        );
        assert!(visualizer
            .property_dependencies(Property::Spin)
            .contains(&Dependency::Band(BandComponent::Bulge)));
        let band = visualizer.band_for(BandComponent::Bulge);
        assert_eq!(band_route(Property::Spin, band, &visualizer), "V");
        assert!(
            graph_sources(DebugSelection::Property(Property::Spin), &visualizer)
                .contains(&GraphSource::BandRole(BandComponent::Bulge))
        );
    }

    #[test]
    fn every_band_has_an_animation_dependency() {
        let visualizer = Visualizer::new();
        for index in 0..BAND_COUNT {
            assert!(
                !band_dependency_line(index, &visualizer).is_empty(),
                "band {index} must name at least one dependent property"
            );
        }
    }

    #[test]
    fn four_or_more_graphs_use_two_rows() {
        assert_eq!(graph_rows(1), 1);
        assert_eq!(graph_rows(3), 1);
        assert_eq!(graph_rows(4), 2);
        assert_eq!(graph_rows(9), 2);
    }

    #[test]
    fn output_graph_max_changes_only_the_upper_bound() {
        let source = GraphSource::Property(Property::Spin);
        assert_eq!(source.range(None), Property::Spin.range());
        assert_eq!(source.range(Some(GraphMax::Eight)), (0.0, 8.0));
        assert_eq!(
            GraphSource::Property(Property::Pitch).range(Some(GraphMax::Four)),
            (Property::Pitch.range().0, 4.0)
        );
    }

    #[test]
    fn output_graph_max_buttons_leave_room_above_the_low_bound() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let visualizer = Visualizer::new();
        let selection = DebugSelection::Property(Property::Zoom);
        let text_scale = VisualizerTextScale::default();
        assert!(
            graph_max_button_center(&view, text_scale, selection, &visualizer, GraphMax::One)
                .is_none()
        );
        assert!(
            graph_max_button_center(&view, text_scale, selection, &visualizer, GraphMax::Two)
                .is_some()
        );
    }

    #[test]
    fn every_dependency_graph_explains_its_contribution() {
        let visualizer = Visualizer::new();
        for &property in Property::VARIANTS {
            let selection = DebugSelection::Property(property);
            for source in graph_sources(selection, &visualizer) {
                assert!(
                    !graph_usage(selection, source).is_empty(),
                    "{} needs a caption for {source:?}",
                    property.name()
                );
            }
        }
        for band in 0..BAND_COUNT {
            let selection = DebugSelection::Band(band);
            for source in graph_sources(selection, &visualizer) {
                assert!(
                    !graph_usage(selection, source).is_empty(),
                    "band {band} needs a caption for {source:?}"
                );
            }
        }
        assert_eq!(
            graph_usage(
                DebugSelection::Property(Property::Pulse),
                GraphSource::Power
            ),
            "×gain add to energy"
        );
        assert_eq!(
            graph_usage(
                DebugSelection::Property(Property::Crossfade),
                GraphSource::Power
            ),
            "×gain add rate; wide switch"
        );
    }
}
