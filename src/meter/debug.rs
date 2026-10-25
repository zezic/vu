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
        BandComponent, BandRouting, ComputeNode, DebugHistory, DebugSample, DebugSelection,
        DebugSnapshot, Dependency, Detector, Input, OnsetDebug, Palette, Parameter, Property,
        StyleInput, TreeTarget, Visualizer, BAND_COUNT,
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

const PROPERTY_COLUMNS: usize = 2;
const PROPERTY_ROWS: usize = Property::COUNT.div_ceil(PROPERTY_COLUMNS);

const TREE_INDENT: f64 = 13.0;
const TREE_PARAM_ROW: f64 = 16.0;
const SLIDER_TRACK_HEIGHT: f64 = 7.0;
const DETECTOR_PANE_HEIGHT: f64 = 96.0;
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
    compact: bool,
}

impl Layout {
    pub fn new(view: &View, text_scale: VisualizerTextScale, compact: bool) -> Self {
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
        let body_height = if compact {
            header_height
                + BAND_COUNT as f64 * band_row_height
                + divider_gap
                + PANEL_BOTTOM_PAD * scale
        } else {
            header_height
                + BAND_COUNT as f64 * band_row_height
                + divider_gap
                + PROPERTY_ROWS as f64 * property_row_height
                + PANEL_BOTTOM_PAD * scale
                + footer_height
        };
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
        let extension_height = if compact {
            (view.viewport.height - margin - extension_top).max(0.0)
        } else {
            (view.viewport.height - margin - extension_top).clamp(0.0, EXTENSION_MAX_HEIGHT * scale)
        };
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
            compact,
        }
    }

    pub(super) fn item_at(&self, point: Point) -> Option<DebugSelection> {
        if !self.panel.contains(point) {
            return None;
        }
        if let Some(band) = self.band_rows.iter().position(|row| row.contains(point)) {
            return Some(DebugSelection::Band(band));
        }
        if self.compact {
            return None;
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
) {
    let property = match selection {
        Some(DebugSelection::Property(property)) => Some(property),
        _ => None,
    };
    let detector = match (property, tree_target) {
        (Some(_), Some(TreeTarget::Detector(detector))) => Some(detector),
        _ => None,
    };
    let compact = detector.is_some();
    let layout = Layout::new(view, text_scale, compact);
    let hovered = pointer.and_then(|point| layout.item_at(point));
    draw_panel(
        fonts,
        scene,
        &layout,
        snapshot,
        visualizer.routing(),
        palette,
        colors,
        refresh,
        selection,
        hovered,
    );

    let extension = if let Some(property) = property {
        let (_, extension) = property_panes(view, text_scale, compact);
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

    let extension = if let Some(detector) = detector {
        let detector_pane = detector_layout(view, text_scale, detector);
        let extension = Rect::new(
            extension.x0,
            extension.y0,
            extension.x1,
            detector_pane.pane.y0 - EXTENSION_GAP * layout.scale,
        );
        draw_detector_pane(
            fonts,
            scene,
            detector,
            &detector_pane,
            visualizer,
            palette,
            colors,
            layout.scale,
            pointer,
        );
        extension
    } else {
        extension
    };

    if let Some(selection) = selection {
        draw_extension(
            fonts, scene, &layout, extension, snapshot, history, visualizer, palette, colors,
            selection,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_panel(
    fonts: &mut Fonts,
    scene: &mut Scene,
    layout: &Layout,
    snapshot: &DebugSnapshot,
    routing: &BandRouting,
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
        let dependencies = band_dependency_line(index, routing);
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
    if layout.compact {
        return;
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
    let heading_height = 11.0 * scale;
    let line_height = 9.5 * scale;
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
                    band_property_values(snapshot, index, visualizer.routing())
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
                property
                    .dependencies()
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

    let graphs_top = inner.y0 + heading_height + line_height * 2.0 + 2.0 * scale;
    let graphs = graph_sources(selection);
    if graphs.is_empty() || graphs_top >= inner.y1 {
        return;
    }
    let gap = 4.0 * scale;
    let rows = graph_rows(graphs.len());
    let first_columns = graphs.len().div_ceil(rows);
    let row_height = (inner.y1 - graphs_top - gap * (rows - 1) as f64) / rows as f64;
    for row in 0..rows {
        let start = row * first_columns;
        let count = first_columns.min(graphs.len() - start);
        let width = (inner.width() - gap * (count - 1) as f64) / count as f64;
        for column in 0..count {
            let index = start + column;
            let left = inner.x0 + column as f64 * (width + gap);
            let top = graphs_top + row as f64 * (row_height + gap);
            let cell = Rect::from_origin_size(Point::new(left, top), Size::new(width, row_height));
            draw_graph(
                fonts,
                scene,
                cell,
                graphs[index],
                selection,
                snapshot,
                history,
                visualizer,
                palette,
                colors,
                scale,
                rows > 1,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_detector_pane(
    fonts: &mut Fonts,
    scene: &mut Scene,
    detector: Detector,
    layout: &DetectorLayout,
    visualizer: &Visualizer,
    palette: Palette,
    colors: MeterColors,
    scale: f64,
    pointer: Option<Point>,
) {
    let pane = layout.pane;
    let color = match detector {
        Detector::Bass => palette.vivid(6),
        Detector::Wide => palette.vivid(8),
    };
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        colors.background.with_alpha(0.95),
        None,
        &pane,
    );
    scene.stroke(
        &Stroke::new(1.0),
        Affine::IDENTITY,
        color.with_alpha(0.55),
        None,
        &pane,
    );
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color.with_alpha(0.85),
        None,
        &Rect::from_origin_size(
            Point::new(pane.x0, pane.y0),
            Size::new(2.0 * scale, pane.height()),
        ),
    );
    let inner = Rect::new(
        pane.x0 + 8.0 * scale,
        pane.y0 + 6.0 * scale,
        pane.x1 - 8.0 * scale,
        pane.y1 - 4.0 * scale,
    );
    draw_slider_content(
        fonts,
        scene,
        inner,
        detector.name(),
        &layout.sliders,
        visualizer,
        palette,
        colors,
        scale,
        pointer,
    );
}

pub(super) fn tree_rect(view: &View, text_scale: VisualizerTextScale, compact: bool) -> Rect {
    property_panes(view, text_scale, compact).0
}

/// Geometry for a selected property. The tree keeps its minimum width and the
/// inspector gives up width when the two cannot both fit.
fn property_panes(view: &View, text_scale: VisualizerTextScale, compact: bool) -> (Rect, Rect) {
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let margin = PANEL_MARGIN * scale;
    let layout = Layout::new(view, text_scale, compact);
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

pub(super) fn detector_layout(
    view: &View,
    text_scale: VisualizerTextScale,
    detector: Detector,
) -> DetectorLayout {
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let base = Layout::new(view, text_scale, true).extension;
    let height = (DETECTOR_PANE_HEIGHT * scale).min(base.height());
    let pane = Rect::new(base.x0, base.y1 - height, base.x1, base.y1);
    let inner = Rect::new(
        pane.x0 + 8.0 * scale,
        pane.y0 + 6.0 * scale,
        pane.x1 - 8.0 * scale,
        pane.y1 - 4.0 * scale,
    );
    DetectorLayout {
        pane,
        sliders: slider_rows(inner, &detector.parameters(), scale),
    }
}

pub(super) struct DetectorLayout {
    pub pane: Rect,
    pub sliders: Vec<SliderRow>,
}

#[derive(Clone, Copy)]
pub(super) struct SliderRow {
    pub parameter: Parameter,
    pub track: Rect,
    hit: Rect,
}

pub(super) struct TreeLayout {
    pub rect: Rect,
    rows: Vec<TreeRow>,
    pub sliders: Vec<SliderRow>,
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

    #[cfg(test)]
    pub fn row_center(&self, index: usize) -> Point {
        self.rows[index].rect.center()
    }

    #[cfg(test)]
    pub fn detector_center(&self, detector: Detector) -> Option<Point> {
        self.rows
            .iter()
            .find(|row| row.node.detector == Some(detector))
            .map(|row| row.rect.center())
    }
}

impl DetectorLayout {
    pub fn slider_at(&self, point: Point) -> Option<SliderRow> {
        self.sliders
            .iter()
            .find(|slider| slider.hit.contains(point))
            .copied()
    }
}

pub(super) struct TreeRow {
    pub node: &'static ComputeNode,
    pub rect: Rect,
    depth: usize,
    parent: Option<usize>,
}

pub(super) fn tree_layout(
    view: &View,
    text_scale: VisualizerTextScale,
    property: Property,
    target: Option<TreeTarget>,
    compact: bool,
) -> TreeLayout {
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let rect = tree_rect(view, text_scale, compact);
    let mut rows = Vec::new();
    flatten_tree(property.computation(), 0, None, &mut rows);
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
    let (parameter_title, parameters) = match target {
        Some(TreeTarget::Component(input)) => {
            let mut parameters = input.parameters();
            parameters.extend(
                property
                    .component_gains(input)
                    .into_iter()
                    .map(Parameter::Gain),
            );
            (input_title(input), parameters)
        }
        Some(TreeTarget::Detector(detector)) => (
            format!("{} CONTROLS ARE BELOW", detector.name()),
            Vec::new(),
        ),
        None => ("SELECT A COMPONENT ROW".to_owned(), Vec::new()),
    };
    let sliders = slider_rows(parameter_panel, &parameters, scale);
    TreeLayout {
        rect,
        rows,
        sliders,
        parameter_title,
        parameter_panel,
    }
}

fn slider_rows(panel: Rect, parameters: &[Parameter], scale: f64) -> Vec<SliderRow> {
    if parameters.is_empty() {
        return Vec::new();
    }
    let title_height = 12.0 * scale;
    let label_width = (panel.width() * 0.34).clamp(38.0 * scale, 132.0 * scale);
    let value_width = 42.0 * scale;
    parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let center = panel.y0
                + title_height
                + index as f64 * TREE_PARAM_ROW * scale
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
    let compact = matches!(target, Some(TreeTarget::Detector(_)));
    let layout = tree_layout(view, text_scale, property, target, compact);
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
        "× multiply   + add   ≥ switch   click a row to tune",
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
        let node = row.node;
        let selected = target.is_some_and(|target| row_matches(node, target));
        let is_hovered = hovered.is_some_and(|hover| std::ptr::eq(hover, row));
        let highlight = target_color(node, palette, visualizer.routing());
        if selected || (is_hovered && (node.input.is_some() || node.detector.is_some())) {
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
        let op_width = (118.0 * scale).min(rect.width() * 0.34);
        let label_width = (right - left - op_width - 4.0 * scale).max(30.0 * scale);
        let mut label = node.label.to_owned();
        if let Some(input) = node.input {
            if let Some(value) = input_value(input, snapshot, visualizer.routing()) {
                label.push_str("  ");
                label.push_str(&value);
            }
        }
        let face = if row.depth == 0 {
            UiFace::SemiBold
        } else {
            UiFace::Light
        };
        let row_color = match node.input {
            Some(input) if row.depth > 0 => input_color(input, palette, visualizer.routing()),
            _ if row.depth == 0 => color,
            _ => palette.neutral[0].with_alpha(0.9),
        };
        let label = fit(fonts, &label, (8.0 * scale) as f32, face, label_width);
        draw_left_center(scene, &label, left, center, row_color);
        if !node.op.is_empty() {
            let op_text = row_operation(node, property, visualizer);
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
                right,
                center,
                palette.neutral[1].with_alpha(0.78),
            );
        }
    }

    let panel = layout.parameter_panel;
    if panel.height() > 0.0 {
        draw_slider_content(
            fonts,
            scene,
            panel,
            &layout.parameter_title,
            &layout.sliders,
            visualizer,
            palette,
            colors,
            scale,
            pointer,
        );
    }
}

fn row_operation(node: &ComputeNode, property: Property, visualizer: &Visualizer) -> String {
    let Some(input) = node.input else {
        return node.op.to_owned();
    };
    let gains = property.component_gains(input);
    let [gain] = gains.as_slice() else {
        return node.op.to_owned();
    };
    let value = visualizer.parameter_value(Parameter::Gain(*gain));
    node.op
        .replace("gain", &Parameter::Gain(*gain).format(value))
}

fn row_matches(node: &ComputeNode, target: TreeTarget) -> bool {
    match target {
        TreeTarget::Component(input) => node.input == Some(input),
        TreeTarget::Detector(detector) => node.detector == Some(detector),
    }
}

fn target_color(node: &ComputeNode, palette: Palette, routing: &BandRouting) -> Color {
    if let Some(detector) = node.detector {
        return match detector {
            Detector::Bass => palette.vivid(6),
            Detector::Wide => palette.vivid(8),
        };
    }
    node.input.map_or(palette.neutral[1], |input| {
        input_color(input, palette, routing)
    })
}

fn input_title(input: Input) -> String {
    match input {
        Input::Band(component) => format!("{} BAND", component.label()),
        Input::BassBands => "BASS BANDS".to_owned(),
        Input::Power => "PWR ENVELOPE".to_owned(),
        Input::PowerLevel => "PWR RAW LEVEL".to_owned(),
        Input::Bass => "BASS ENVELOPE".to_owned(),
        Input::BassLevel => "BASS RAW LEVEL".to_owned(),
        Input::BassFlux => "BASS FLUX".to_owned(),
        Input::Flux => "SPECTRAL FLUX".to_owned(),
        Input::Kick => "KICK ENVELOPE".to_owned(),
        Input::BassOnset => "BASS ONSET".to_owned(),
        Input::WideOnset => "WIDE ONSET".to_owned(),
        Input::PhaseClock => "PHASE CLOCK".to_owned(),
        Input::Style(style) => Parameter::Style(style).label().to_owned(),
        Input::Property(property) => property.name().to_owned(),
        Input::Constant(value) => format!("CONSTANT {value}"),
    }
}

fn flatten_tree(
    node: &'static ComputeNode,
    depth: usize,
    parent: Option<usize>,
    rows: &mut Vec<TreeRow>,
) {
    let index = rows.len();
    rows.push(TreeRow {
        node,
        rect: Rect::ZERO,
        depth,
        parent,
    });
    for child in node.children {
        flatten_tree(child, depth + 1, Some(index), rows);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_slider_content(
    fonts: &mut Fonts,
    scene: &mut Scene,
    panel: Rect,
    title: &str,
    sliders: &[SliderRow],
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
    let show_empty_hint = !title.contains("CONTROLS ARE BELOW");
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
    if sliders.is_empty() {
        if show_empty_hint {
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
            slider.parameter.label(),
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
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            if hovered {
                colors.readout
            } else {
                palette.vivid(9)
            },
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
        let value = fit(
            fonts,
            &slider.parameter.format(value),
            (7.0 * scale) as f32,
            UiFace::SemiBold,
            42.0 * scale,
        );
        draw_right_center(
            scene,
            &value,
            slider.track.x1.max(panel.x1 - 2.0 * scale),
            center,
            palette.neutral[0],
        );
    }
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

    fn range(self) -> (f32, f32) {
        match self {
            Self::Band(_) | Self::BandRole(_) | Self::Power | Self::Bass => (0.0, 1.0),
            Self::Kick => (0.0, 1.5),
            Self::Property(property) => property.range(),
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

fn graph_sources(selection: DebugSelection) -> Vec<GraphSource> {
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
            for dependency in property.dependencies() {
                match dependency {
                    Dependency::Band(component) => push(GraphSource::BandRole(*component)),
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
) {
    if rect.width() < 12.0 * scale || rect.height() < 12.0 * scale {
        return;
    }
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
    let plot = Rect::new(
        rect.x0 + 2.0 * scale,
        plot_top,
        rect.x1 - 2.0 * scale,
        rect.y1 - 2.0 * scale,
    );
    if plot.height() < 2.0 * scale {
        return;
    }
    let range = source.range();
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

fn band_dependency_line(band: usize, routing: &BandRouting) -> String {
    Property::VARIANTS
        .iter()
        .copied()
        .filter_map(|property| {
            let route = band_route(property, band, routing);
            (!route.is_empty()).then(|| format!("{route}:{}", property.name()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn band_route(property: Property, band: usize, routing: &BandRouting) -> String {
    let mut route = String::new();
    if property.depends_on_band_value(band, routing) {
        route.push('V');
    }
    if property.depends_on_bass_onset(band) {
        route.push('B');
    }
    if property.depends_on_wide_onset() {
        route.push('W');
    }
    route
}

fn band_property_values(snapshot: &DebugSnapshot, band: usize, routing: &BandRouting) -> String {
    Property::VARIANTS
        .iter()
        .copied()
        .filter(|property| property.depends_on_band(band, routing))
        .map(|property| {
            format!(
                "{}:{} {:.2}",
                band_route(property, band, routing),
                property.name(),
                snapshot.properties[property.index()]
            )
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
        Layout::new(&view, VisualizerTextScale::default(), false)
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
        let tree = tree_rect(&view, VisualizerTextScale::default(), false);
        let layout = Layout::new(&view, VisualizerTextScale::default(), false);
        let (_, inspector) = property_panes(&view, VisualizerTextScale::default(), false);
        assert!(tree.x1 <= layout.panel.x0);
        assert_eq!(inspector.x0 - tree.x1, EXTENSION_GAP);
        assert_eq!(tree.y1, layout.extension.y1);
        assert!(tree.x0 > 150.0);
        assert!(tree.width() > 300.0);
        assert!(tree.height() > 400.0);
    }

    #[test]
    fn detector_mode_compacts_the_list_and_stacks_the_controls() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let normal = Layout::new(&view, text_scale, false);
        let compact = Layout::new(&view, text_scale, true);
        assert!(compact.panel.height() < normal.panel.height());
        let (_, extension) = property_panes(&view, text_scale, true);
        let pane = detector_layout(&view, text_scale, Detector::Wide).pane;
        assert_eq!(pane.y1, extension.y1);
        assert!(pane.y0 > extension.y0 + 80.0);
        assert_eq!(
            detector_layout(&view, text_scale, Detector::Wide)
                .sliders
                .len(),
            4
        );
    }

    #[test]
    fn tight_windows_shrink_the_inspector_before_hiding_the_tree() {
        let view = View::fit(Size::new(900.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let layout = Layout::new(&view, text_scale, false);
        let (tree, inspector) = property_panes(&view, text_scale, false);
        assert!(tree.width() > 250.0);
        assert!(inspector.width() < layout.extension.width());
        assert_eq!(inspector.x0 - tree.x1, EXTENSION_GAP);
        assert_eq!(tree.y1, inspector.y1);
        assert!(tree.x1 <= layout.panel.x0);
    }

    #[test]
    fn band_routes_name_value_and_onset_dependencies() {
        let routing = BandRouting::default();
        assert_eq!(band_route(Property::Pulse, 0, &routing), "V");
        assert_eq!(band_route(Property::Kick, 0, &routing), "VB");
        assert_eq!(band_route(Property::Crossfade, 0, &routing), "BW");
        assert_eq!(band_route(Property::Crossfade, 4, &routing), "W");
        assert_eq!(band_route(Property::Spin, 4, &routing), "V");
        assert_eq!(band_route(Property::Spin, 5, &routing), "");
        assert!(Property::Kick.depends_on_band(3, &routing));
        assert!(!Property::Kick.depends_on_band(4, &routing));
    }

    #[test]
    fn band_dependency_text_follows_rerouting() {
        let mut routing = BandRouting::default();
        assert!(band_route(Property::Spin, 4, &routing) == "V");
        assert!(band_route(Property::Spin, 7, &routing).is_empty());
        routing[BandComponent::Spin] = 7;
        assert!(band_route(Property::Spin, 4, &routing).is_empty());
        assert_eq!(band_route(Property::Spin, 7, &routing), "V");
        assert!(band_dependency_line(7, &routing).contains("V:SPIN"));
    }

    #[test]
    fn every_band_has_an_animation_dependency() {
        for index in 0..BAND_COUNT {
            assert!(
                !band_dependency_line(index, &BandRouting::default()).is_empty(),
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
    fn every_dependency_graph_explains_its_contribution() {
        for &property in Property::VARIANTS {
            let selection = DebugSelection::Property(property);
            for source in graph_sources(selection) {
                assert!(
                    !graph_usage(selection, source).is_empty(),
                    "{} needs a caption for {source:?}",
                    property.name()
                );
            }
        }
        for band in 0..BAND_COUNT {
            let selection = DebugSelection::Band(band);
            for source in graph_sources(selection) {
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
