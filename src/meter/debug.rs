use std::collections::VecDeque;

use strum::{EnumCount, VariantArray};
use vello::{
    kurbo::{Affine, BezPath, Cap, Line, Point, Rect, Size, Stroke, Triangle},
    peniko::{Color, Fill},
    Scene,
};

use super::{DebugRefresh, View};
use crate::{
    config::VisualizerTextScale,
    text::{Fonts, Text, UiFace},
    theme::{mix, MeterColors},
    visualizer::{
        BandComponent, BandRouting, ComputeNode, Contribution, DebugHistory, DebugSample,
        DebugSelection, DebugSnapshot, Dependency, Detector, DropKernel, EnvelopeTarget, ExtraRef,
        Input, InputShape, Mix, MixPoint, Mixer, OnsetFluxStatus, Palette, Parameter,
        PatternKernel, PatternLayer, Property, StyleInput, TemporalPattern, TreeTarget,
        TriggerStatus, Visualizer, BAND_COUNT, DROP_GAP_THRESHOLD_MAX, DROP_GAP_THRESHOLD_MIN,
    },
};

const PANEL_WIDTH: f64 = 440.0;
const PANEL_MARGIN: f64 = 16.0;
const HEADER_HEIGHT: f64 = 16.0;
const BAND_ROW_HEIGHT: f64 = 10.0;
const DIVIDER_GAP: f64 = 7.0;
const PROPERTY_ROW_HEIGHT: f64 = 10.0;
const PROPERTY_TOGGLE_SIZE: f64 = 7.0;
const PROPERTY_TOGGLE_GAP: f64 = 2.0;
const PROPERTY_VALUE_WIDTH: f64 = 32.0;
const PROPERTY_EDGE_PAD: f64 = 3.0;
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

/// One of the two per-property switches in the property list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PropertyButton {
    Mute,
    Solo,
}

/// Panel geometry shared by drawing and hit testing. Keeping one layout keeps
/// the selection targets exactly under the visible rows.
pub(super) struct Layout {
    pub panel: Rect,
    pub extension: Rect,
    pub flux_button: Rect,
    band_rows: [Rect; BAND_COUNT],
    property_cells: [Rect; Property::COUNT],
    property_mute: [Rect; Property::COUNT],
    property_solo: [Rect; Property::COUNT],
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
        let flux_button_width = 58.0 * scale;
        let flux_button = Rect::from_origin_size(
            Point::new(
                x0 + panel_width - 8.0 * scale - flux_button_width,
                y0 + 2.0 * scale,
            ),
            Size::new(flux_button_width, header_height - 4.0 * scale),
        );

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
        let toggle_size = PROPERTY_TOGGLE_SIZE * scale;
        let toggle_top = |cell: Rect| cell.center().y - toggle_size * 0.5;
        let property_solo = std::array::from_fn(|index| {
            let cell = property_cells[index];
            let right =
                cell.x1 - (PROPERTY_EDGE_PAD + PROPERTY_VALUE_WIDTH + PROPERTY_EDGE_PAD) * scale;
            Rect::from_origin_size(
                Point::new(right - toggle_size, toggle_top(cell)),
                Size::new(toggle_size, toggle_size),
            )
        });
        let property_mute = std::array::from_fn(|index| {
            let solo = property_solo[index];
            let right = solo.x0 - PROPERTY_TOGGLE_GAP * scale;
            let top = toggle_top(property_cells[index]);
            Rect::from_origin_size(
                Point::new(right - toggle_size, top),
                Size::new(toggle_size, toggle_size),
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
            flux_button,
            band_rows,
            property_cells,
            property_mute,
            property_solo,
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
            .enumerate()
            .find(|(_, cell)| cell.y1 <= self.footer_top && cell.contains(point))
            .map(|(index, _)| DebugSelection::Property(Property::VARIANTS[index]))
    }

    #[cfg(test)]
    pub(super) fn property_center(&self, property: Property) -> Point {
        self.property_cells[property.index()].center()
    }

    pub(super) fn property_button_at(&self, point: Point) -> Option<(Property, PropertyButton)> {
        for (index, &property) in Property::VARIANTS.iter().enumerate() {
            if self.property_cells[index].y1 > self.footer_top {
                continue;
            }
            if self.property_mute[index].contains(point) {
                return Some((property, PropertyButton::Mute));
            }
            if self.property_solo[index].contains(point) {
                return Some((property, PropertyButton::Solo));
            }
        }
        None
    }

    #[cfg(test)]
    pub(super) fn property_mute_center(&self, property: Property) -> Point {
        self.property_mute[property.index()].center()
    }

    #[cfg(test)]
    pub(super) fn property_solo_center(&self, property: Property) -> Point {
        self.property_solo[property.index()].center()
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
    flux_band: usize,
    temporal_view: &TemporalView,
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

    let extension = if matches!(selection, Some(DebugSelection::Flux)) {
        let (_, extension) = inspector_panes(view, text_scale);
        draw_flux_analyzer(
            fonts, scene, view, history, snapshot, visualizer, palette, colors, text_scale,
            flux_band, pointer,
        );
        extension
    } else if let Some(property) = property {
        let (_, extension) = inspector_panes(view, text_scale);
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
            temporal_view,
            pointer,
        );
        extension
    } else {
        layout.extension
    };

    if let Some(selection) = selection {
        let focus = inspector_focus(selection, tree_target, flux_band);
        draw_extension(
            fonts, scene, &layout, extension, snapshot, history, visualizer, palette, colors,
            selection, focus, graph_max,
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
            "ANALYZER {} BANDS / {}  V VALUE  ONSET FLOW",
            BAND_COUNT,
            refresh.name()
        ),
        (9.5 * scale) as f32,
        UiFace::SemiBold,
        panel.width() - 16.0 * scale - layout.flux_button.width() - 6.0 * scale,
    );
    let header_center = panel.y0 + HEADER_HEIGHT * scale * 0.5;
    draw_left_center(
        scene,
        &header,
        panel.x0 + 8.0 * scale,
        header_center,
        palette.vivid(9),
    );
    let flux_active = selection == Some(DebugSelection::Flux);
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        if flux_active {
            palette.vivid(9).with_alpha(0.24)
        } else {
            palette.neutral[0].with_alpha(0.08)
        },
        None,
        &layout.flux_button,
    );
    scene.stroke(
        &Stroke::new(0.8 * scale),
        Affine::IDENTITY,
        palette
            .vivid(9)
            .with_alpha(if flux_active { 0.9 } else { 0.5 }),
        None,
        &layout.flux_button,
    );
    let flux_label = fit(
        fonts,
        "FLUX ENV",
        (7.0 * scale) as f32,
        UiFace::SemiBold,
        layout.flux_button.width() - 4.0 * scale,
    );
    draw_left_center(
        scene,
        &flux_label,
        layout.flux_button.center().x - flux_label.width() * 0.5,
        layout.flux_button.center().y,
        if flux_active {
            palette.vivid(9)
        } else {
            palette.neutral[1]
        },
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
        let dependency_width = bar_x - dependency_x - 4.0 * scale;
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
        let active = visualizer.property_active(property);
        draw_row_highlight(scene, cell, color, highlighted, selected, scale);

        let center = cell.center().y;
        let value = snapshot.properties[property.index()];
        let name_width = (layout.property_mute[index].x0 - cell.x0 - 6.0 * scale).max(1.0);
        let name = fit(
            fonts,
            property.name(),
            (8.5 * scale) as f32,
            UiFace::Light,
            name_width,
        );
        draw_left_center(
            scene,
            &name,
            cell.x0 + PROPERTY_EDGE_PAD * scale,
            center,
            color.with_alpha(if !active {
                0.4
            } else if selected || highlighted {
                1.0
            } else {
                0.9
            }),
        );
        let value = fit(
            fonts,
            &format!("{value:>5.2}"),
            (8.5 * scale) as f32,
            UiFace::SemiBold,
            PROPERTY_VALUE_WIDTH * scale,
        );
        draw_right_center(
            scene,
            &value,
            cell.x1 - PROPERTY_EDGE_PAD * scale,
            center,
            palette.neutral[0].with_alpha(if active { 0.9 } else { 0.35 }),
        );
        draw_property_toggle(
            fonts,
            scene,
            layout.property_mute[index],
            "M",
            visualizer.property_muted(property),
            palette.vivid(0),
            active,
            palette,
            scale,
        );
        draw_property_toggle(
            fonts,
            scene,
            layout.property_solo[index],
            "S",
            visualizer.property_soloed(property),
            palette.vivid(2),
            active,
            palette,
            scale,
        );
    }

    let footer = layout.footer_top;
    let first = format!(
        "PWR S{:.2} R{:.2}  BASS S{:.2} R{:.2}  TREB S{:.2}",
        snapshot.power.smooth,
        snapshot.power.raw,
        snapshot.bass.smooth,
        snapshot.bass.raw,
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
            "K {:>3} S {:>3} D {:>3}  F {:.1}/{:.1}/{:.1}  {} L{} R{}",
            snapshot.kick_hits,
            snapshot.snare_hits,
            snapshot.drop_hits,
            snapshot.kick_onset.flash,
            snapshot.snare_onset.flash,
            snapshot.drop_onset.flash,
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
    focus: InspectorFocus,
    graph_max: &[Option<GraphMax>; Property::COUNT],
) {
    let scale = layout.scale;
    if rect.height() < EXTENSION_MIN_HEIGHT * scale {
        return;
    }
    let color = focus.color(palette);
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
    let (heading, description, dependencies) = match focus {
        InspectorFocus::Selection(selection) => match selection {
            DebugSelection::Band(index) => {
                let band = &snapshot.bands[index];
                (
                    format!(
                        "BAND {:02}  {:.0}-{:.0}Hz  {}",
                        index,
                        band.low_hz,
                        band.high_hz,
                        band.role.name()
                    ),
                    format!(
                        "Excess {:.3}, onset {:.3}, ABDR {:.2}, ABL {:.2}, DRP {:.2}.",
                        band.excess, band.onset, band.abdr, band.abl, band.drp
                    ),
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
            DebugSelection::Flux => {
                unreachable!("the flux analyzer uses InspectorFocus::Flux")
            }
        },
        InspectorFocus::Detector(detector) => {
            detector_extension_text(detector, snapshot, visualizer)
        }
        InspectorFocus::Flux(band) => flux_extension_text(band, snapshot, visualizer),
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

    let selected_max = match focus {
        InspectorFocus::Selection(DebugSelection::Property(property)) => {
            graph_max[property.index()]
        }
        _ => None,
    };
    let (cells, rows) = extension_graph_cells(rect, scale, focus, visualizer);
    for (source, cell) in cells {
        draw_graph(
            fonts,
            scene,
            cell,
            source,
            selection,
            focus,
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

fn detector_extension_text(
    detector: Detector,
    snapshot: &DebugSnapshot,
    visualizer: &Visualizer,
) -> (String, String, String) {
    if detector == Detector::Drop {
        return drop_extension_text(snapshot, visualizer);
    }
    let trigger = trigger_snapshot(snapshot, detector);
    let onset = onset_flux_snapshot(snapshot, detector);
    let hits = match detector {
        Detector::Kick => snapshot.kick_hits,
        Detector::Snare => snapshot.snare_hits,
        _ => 0,
    };
    (
        format!("{} TRIGGER  {}", detector.name(), trigger_state(&trigger)),
        format!("ONSET FLUX {:.3}/{:.3}", onset.smoothed, onset.threshold),
        format!("HITS {hits}  LAST {:.2}", trigger.power),
    )
}

fn flux_extension_text(
    band: usize,
    snapshot: &DebugSnapshot,
    visualizer: &Visualizer,
) -> (String, String, String) {
    let debug = &snapshot.bands[band];
    let settings = visualizer.detector_settings();
    let band_settings = settings.band_onset[band];
    (
        format!(
            "ONSET FLOW B{:02}  {:.0}-{:.0}Hz",
            band, debug.low_hz, debug.high_hz
        ),
        format!(
            "EXCESS {:.3}  ONSET {:.3}  ABDR {:.2}  ABL {:.2}  DRP {:.2}",
            debug.excess, debug.onset, debug.abdr, debug.abl, debug.drp
        ),
        format!(
            "ABL G{:.2} {} {:.2}  BG {:.0}/{:.0}ms  FLOOR {:.3}  CAP {:.2}",
            band_settings.abl_gain,
            band_settings.abl_curve.name(),
            band_settings.abl_amount,
            settings.excess.background_attack * 1000.0,
            settings.excess.background_release * 1000.0,
            settings.excess.floor,
            settings.excess.cap
        ),
    )
}

fn drop_extension_text(
    snapshot: &DebugSnapshot,
    visualizer: &Visualizer,
) -> (String, String, String) {
    let settings = visualizer.detector_settings();
    let trigger = snapshot.drop_onset;
    let status = snapshot.drop_status;
    (
        format!("DROP TRIGGER  {}", trigger_state(&trigger)),
        format!(
            "LF {:.2}  HF {:.2}  DIFF {:.2}  CONF {:.2}/{:.2} x{:.2}",
            status.low,
            status.high,
            status.diff,
            status.confidence,
            settings.drop_gap_threshold,
            settings.drop_gap_gain
        ),
        format!(
            "WINDOW {:.2}s  ONSET {:.3}/{:.3}  HITS {}  LAST {:.2}",
            settings.drop_gap_window,
            status.onset.smoothed,
            status.onset.threshold,
            snapshot.drop_hits,
            trigger.power
        ),
    )
}

pub(super) fn tree_rect(view: &View, text_scale: VisualizerTextScale) -> Rect {
    inspector_panes(view, text_scale).0
}

/// Geometry for a selected property or band. The tree keeps its minimum width
/// and the inspector gives up width when the two cannot both fit.
fn inspector_panes(view: &View, text_scale: VisualizerTextScale) -> (Rect, Rect) {
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

/// One drawable 16-band kernel bank.
#[derive(Clone, Copy)]
pub(super) struct KernelBank {
    pub kernel: DropKernel,
    pub rect: Rect,
}

/// One drawable 16-band onset flow bank.
#[derive(Clone, Copy)]
pub(super) struct FluxBank {
    pub rect: Rect,
    pub selected: usize,
}

/// The two internal onset-flow graphs below the analyzer controls.
#[derive(Clone, Copy)]
pub(super) struct OnsetGraphs {
    pub rect: Rect,
    pub lanes: [Rect; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TreeAction {
    ToggleMix(Contribution),
    AddExtra(MixPoint),
    RemoveExtra(ExtraRef),
    ToggleAdaptive(EnvelopeTarget),
    ClearPattern(Detector),
    TogglePatternOverlay,
    TogglePatternPreview,
    TogglePatternPause,
    TogglePatternSuppression,
}

#[derive(Clone, Copy)]
pub(super) struct ActionRow {
    pub action: TreeAction,
    rect: Rect,
}

/// The onset-input spectrogram and per-band temporal template editor for the
/// kick and snare detectors.
#[derive(Clone, Copy)]
pub(super) struct TemporalBank {
    pub detector: Detector,
    pub canvas: Rect,
    pub clear: ActionRow,
    pub overlay: ActionRow,
    pub preview: ActionRow,
    pub pause: ActionRow,
    pub suppress: ActionRow,
    pub offset: Rect,
}

pub(super) const PATTERN_POINT_RADIUS: f64 = 2.0;
pub(super) const PATTERN_POINT_HIT_RADIUS: f64 = 6.0;
pub(super) const PATTERN_LINE_HIT_RADIUS: f64 = 3.0;

#[derive(Clone, Copy, Debug)]
pub(super) enum PatternHit {
    Vertex {
        layer: PatternLayer,
        index: usize,
    },
    Edge {
        layer: PatternLayer,
        index: usize,
        point: [f32; 2],
    },
}

pub(super) fn pattern_hit(
    pattern: &TemporalPattern,
    kernel: PatternKernel,
    rect: Rect,
    pointer: Point,
    scale: f64,
) -> Option<PatternHit> {
    let (inner, outer) = pattern.polygons(kernel);
    let mut nearest: Option<(f64, PatternHit)> = None;
    for (layer, points) in [(PatternLayer::Inner, inner), (PatternLayer::Outer, outer)] {
        for (index, point) in points.iter().enumerate() {
            let x = rect.x0 + f64::from(point[0]) * rect.width();
            let y = rect.y0 + f64::from(point[1]) * rect.height();
            let distance = ((x - pointer.x).powi(2) + (y - pointer.y).powi(2)).sqrt();
            if distance <= PATTERN_POINT_HIT_RADIUS * scale
                && nearest.as_ref().is_none_or(|(best, _)| distance < *best)
            {
                nearest = Some((distance, PatternHit::Vertex { layer, index }));
            }
        }
    }
    if nearest.is_some() {
        return nearest.map(|(_, hit)| hit);
    }
    for (layer, points) in [(PatternLayer::Inner, inner), (PatternLayer::Outer, outer)] {
        if points.len() < 2 {
            continue;
        }
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            let ax = rect.x0 + f64::from(a[0]) * rect.width();
            let ay = rect.y0 + f64::from(a[1]) * rect.height();
            let bx = rect.x0 + f64::from(b[0]) * rect.width();
            let by = rect.y0 + f64::from(b[1]) * rect.height();
            let dx = bx - ax;
            let dy = by - ay;
            let length_squared = dx * dx + dy * dy;
            if length_squared <= f64::EPSILON {
                continue;
            }
            let t =
                (((pointer.x - ax) * dx + (pointer.y - ay) * dy) / length_squared).clamp(0.0, 1.0);
            let x = ax + t * dx;
            let y = ay + t * dy;
            let distance = ((x - pointer.x).powi(2) + (y - pointer.y).powi(2)).sqrt();
            if distance <= PATTERN_LINE_HIT_RADIUS * scale
                && nearest.as_ref().is_none_or(|(best, _)| distance < *best)
            {
                nearest = Some((
                    distance,
                    PatternHit::Edge {
                        layer,
                        index,
                        point: [
                            (x - rect.x0) as f32 / rect.width() as f32,
                            (y - rect.y0) as f32 / rect.height() as f32,
                        ],
                    },
                ));
            }
        }
    }
    nearest.map(|(_, hit)| hit)
}

#[derive(Default)]
pub(super) struct TemporalView {
    pub overlay: bool,
    pub preview: bool,
    pub paused: bool,
    pub kernel: PatternKernel,
    pub offset: f32,
    pub frozen: Option<VecDeque<[f32; BAND_COUNT]>>,
}

impl TemporalView {
    pub fn editing() -> Self {
        Self {
            overlay: true,
            preview: true,
            ..Self::default()
        }
    }
}

pub(super) struct TreeLayout {
    pub rect: Rect,
    pub rows: Vec<TreeRow>,
    pub sliders: Vec<SliderRow>,
    pub actions: Vec<ActionRow>,
    pub kernels: Vec<KernelBank>,
    pub temporal: Option<TemporalBank>,
    pub flux_bank: Option<FluxBank>,
    pub onset_graphs: Option<OnsetGraphs>,
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

    pub fn kernel_at(&self, point: Point) -> Option<(DropKernel, Rect, usize)> {
        self.kernels.iter().find_map(|bank| {
            bank.rect.contains(point).then(|| {
                let fraction = ((point.x - bank.rect.x0) / bank.rect.width()).clamp(0.0, 0.999_999);
                let band = (fraction * BAND_COUNT as f64) as usize;
                (bank.kernel, bank.rect, band)
            })
        })
    }

    pub fn flux_band_at(&self, point: Point) -> Option<usize> {
        let bank = self.flux_bank?;
        bank.rect.contains(point).then(|| {
            let fraction = ((point.x - bank.rect.x0) / bank.rect.width()).clamp(0.0, 0.999_999);
            (fraction * BAND_COUNT as f64) as usize
        })
    }

    pub fn action_at(&self, point: Point) -> Option<TreeAction> {
        self.actions
            .iter()
            .find(|row| row.rect.contains(point))
            .map(|row| row.action)
            .or_else(|| {
                self.temporal.and_then(|temporal| {
                    [
                        temporal.clear,
                        temporal.overlay,
                        temporal.preview,
                        temporal.pause,
                        temporal.suppress,
                    ]
                    .into_iter()
                    .find(|row| row.rect.contains(point))
                    .map(|row| row.action)
                })
            })
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
            .or_else(|| {
                self.temporal.and_then(|temporal| {
                    [
                        temporal.clear,
                        temporal.overlay,
                        temporal.preview,
                        temporal.pause,
                        temporal.suppress,
                    ]
                    .into_iter()
                    .find(|row| row.action == action)
                    .map(|row| row.rect.center())
                })
            })
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
    let compact = matches!(
        target,
        Some(TreeTarget::Detector(Detector::Kick | Detector::Snare))
    );
    let row_height = if compact {
        12.0 * scale
    } else {
        TREE_PARAM_ROW * scale
    };
    let action_rows = action_rows(parameter_panel, &actions, scale, row_height);
    let sliders = slider_rows(
        parameter_panel,
        &parameters,
        scale,
        action_rows.len(),
        row_height,
    );
    let kernel_targets: Vec<DropKernel> = match target {
        Some(TreeTarget::Detector(Detector::Drop)) => DropKernel::ALL.to_vec(),
        _ => Vec::new(),
    };
    let kernels = kernel_banks(
        parameter_panel,
        &sliders,
        &action_rows,
        scale,
        &kernel_targets,
    );
    let temporal = match target {
        Some(TreeTarget::Detector(detector @ (Detector::Kick | Detector::Snare))) => {
            temporal_bank(parameter_panel, &sliders, &action_rows, scale, detector)
        }
        _ => None,
    };
    TreeLayout {
        rect,
        rows,
        sliders,
        actions: action_rows,
        kernels,
        temporal,
        flux_bank: None,
        onset_graphs: None,
        parameter_title,
        parameter_panel,
    }
}

fn temporal_bank(
    panel: Rect,
    sliders: &[SliderRow],
    actions: &[ActionRow],
    scale: f64,
    detector: Detector,
) -> Option<TemporalBank> {
    let top = parameter_content_top(panel, sliders, actions, scale);
    let available = panel.y1 - top;
    if available < 70.0 * scale {
        return None;
    }
    let button_height = 11.0 * scale;
    let gap = 2.0 * scale;
    let offset_height = 8.0 * scale;
    let button_width = (panel.width() - 4.0 * 3.0 * scale) / 5.0;
    let button = |index: usize, action: TreeAction| ActionRow {
        action,
        rect: Rect::new(
            panel.x0 + index as f64 * (button_width + 3.0 * scale),
            top + scale,
            panel.x0 + index as f64 * (button_width + 3.0 * scale) + button_width,
            top + button_height,
        ),
    };
    let clear = button(0, TreeAction::ClearPattern(detector));
    let overlay = button(1, TreeAction::TogglePatternOverlay);
    let preview = button(2, TreeAction::TogglePatternPreview);
    let pause = button(3, TreeAction::TogglePatternPause);
    let suppress = button(4, TreeAction::TogglePatternSuppression);
    let offset = Rect::new(panel.x0, panel.y1 - offset_height, panel.x1, panel.y1);
    let canvas = Rect::new(
        panel.x0,
        top + button_height + gap,
        panel.x1,
        offset.y0 - gap,
    );
    if canvas.height() < 24.0 * scale {
        return None;
    }
    Some(TemporalBank {
        detector,
        canvas,
        clear,
        overlay,
        preview,
        pause,
        suppress,
        offset,
    })
}

fn parameter_content_top(
    panel: Rect,
    sliders: &[SliderRow],
    actions: &[ActionRow],
    scale: f64,
) -> f64 {
    let action_bottom = actions
        .last()
        .map_or(panel.y0 + 12.0 * scale, |row| row.rect.y1);
    let slider_bottom = sliders.last().map_or(action_bottom, |row| row.track.y1);
    slider_bottom.max(action_bottom) + 4.0 * scale
}

fn flux_bank(
    panel: Rect,
    sliders: &[SliderRow],
    actions: &[ActionRow],
    scale: f64,
    selected: usize,
    bottom: f64,
) -> Option<FluxBank> {
    let label_height = 9.0 * scale;
    let top = parameter_content_top(panel, sliders, actions, scale);
    let available = bottom - top;
    if available <= label_height + 8.0 * scale {
        return None;
    }
    Some(FluxBank {
        rect: Rect::new(panel.x0, top + label_height, panel.x1, bottom),
        selected,
    })
}

fn kernel_banks(
    panel: Rect,
    sliders: &[SliderRow],
    actions: &[ActionRow],
    scale: f64,
    targets: &[DropKernel],
) -> Vec<KernelBank> {
    if targets.is_empty() {
        return Vec::new();
    }
    let label_height = 9.0 * scale;
    let gap = 3.0 * scale;
    let top = parameter_content_top(panel, sliders, actions, scale);
    let available = panel.y1 - top;
    let count = targets.len() as f64;
    let per_bank = (available - gap * (count - 1.0)) / count;
    if per_bank <= label_height + 8.0 * scale {
        return Vec::new();
    }
    let label_width = (panel.width() * 0.34).clamp(38.0 * scale, 132.0 * scale);
    let value_width = 42.0 * scale;
    let left = panel.x0 + label_width;
    let right = panel.x1 - value_width;
    targets
        .iter()
        .enumerate()
        .map(|(index, &kernel)| {
            let bank_top = top + index as f64 * (per_bank + gap);
            KernelBank {
                kernel,
                rect: Rect::new(left, bank_top + label_height, right, bank_top + per_bank),
            }
        })
        .collect()
}

fn parameter_content(
    target: Option<TreeTarget>,
    mixer: &Mixer,
) -> (String, Vec<Parameter>, Vec<TreeAction>) {
    match target {
        Some(TreeTarget::Contribution(Contribution::Fixed(point, index))) => {
            let spec = point.contributions()[index];
            let contribution = Contribution::Fixed(point, index);
            let mut parameters = spec.input().map(Input::parameters).unwrap_or_default();
            if spec.constant().is_some() {
                parameters.push(Parameter::Constant(contribution));
            }
            if let Some(gain) = spec.gain {
                parameters.push(Parameter::Gain(gain));
            }
            if let Some(style) = spec.style_scale {
                parameters.push(Parameter::Style(style));
            }
            let mut actions = vec![TreeAction::ToggleMix(contribution)];
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
        Some(TreeTarget::Detector(Detector::Flux)) => {
            unreachable!("the flux analyzer has its own panel")
        }
        Some(TreeTarget::Detector(detector)) => (
            detector.name().to_owned(),
            detector.parameters(),
            Vec::new(),
        ),
        None => (
            "SELECT A MIX POINT OR COMPONENT ROW".to_owned(),
            Vec::new(),
            Vec::new(),
        ),
    }
}

fn action_rows(panel: Rect, actions: &[TreeAction], scale: f64, row_height: f64) -> Vec<ActionRow> {
    let title_height = 12.0 * scale;
    actions
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let top = panel.y0 + title_height + index as f64 * row_height;
            ActionRow {
                action: *action,
                rect: Rect::new(
                    panel.x0,
                    top + 1.0 * scale,
                    panel.x1,
                    top + row_height - 2.0 * scale,
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
    row_height: f64,
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
                + (rows_before + index) as f64 * row_height
                + row_height * 0.5;
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
                    center - row_height * 0.45,
                    track.x1,
                    center + row_height * 0.45,
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
    temporal_view: &TemporalView,
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
            &layout.kernels,
            shape,
            visualizer,
            palette,
            colors,
            scale,
            pointer,
        );
        if let Some(temporal) = layout.temporal {
            draw_temporal_bank(
                fonts,
                scene,
                temporal,
                visualizer,
                palette,
                scale,
                temporal_view,
                pointer,
            );
        }
    }
}

pub(super) fn flux_analyzer_layout(
    view: &View,
    text_scale: VisualizerTextScale,
    band: usize,
) -> TreeLayout {
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let rect = tree_rect(view, text_scale);
    let parameter_panel = Rect::new(
        rect.x0 + 8.0 * scale,
        rect.y0 + 25.0 * scale,
        rect.x1 - 8.0 * scale,
        rect.y1 - 5.0 * scale,
    );
    let target = EnvelopeTarget::BandIndex(band);
    let parameters = vec![
        Parameter::InputLow(target),
        Parameter::InputHigh(target),
        Parameter::BandAblGain(band),
        Parameter::BandAblCurve(band),
        Parameter::BandAblAmount(band),
        Parameter::ExcessAttack,
        Parameter::ExcessRelease,
        Parameter::ExcessFloor,
        Parameter::ExcessCap,
    ];
    let actions = vec![TreeAction::ToggleAdaptive(target)];
    let action_rows = action_rows(parameter_panel, &actions, scale, TREE_PARAM_ROW * scale);
    let sliders = slider_rows(
        parameter_panel,
        &parameters,
        scale,
        action_rows.len(),
        TREE_PARAM_ROW * scale,
    );
    let content_top = parameter_content_top(parameter_panel, &sliders, &action_rows, scale);
    let available = parameter_panel.y1 - content_top;
    let mut onset_graphs = None;
    let flux_bank = if available >= 130.0 * scale {
        let graphs_height = (available * 0.48).clamp(80.0 * scale, 130.0 * scale);
        let graphs_rect = Rect::new(
            parameter_panel.x0,
            parameter_panel.y1 - graphs_height,
            parameter_panel.x1,
            parameter_panel.y1,
        );
        let title_height = 9.0 * scale;
        let gap = 4.0 * scale;
        let lane_height = (graphs_height - title_height - gap) * 0.5;
        let first = Rect::new(
            graphs_rect.x0,
            graphs_rect.y0 + title_height,
            graphs_rect.x1,
            graphs_rect.y0 + title_height + lane_height,
        );
        let second = Rect::new(
            graphs_rect.x0,
            first.y1 + gap,
            graphs_rect.x1,
            graphs_rect.y1,
        );
        onset_graphs = Some(OnsetGraphs {
            rect: graphs_rect,
            lanes: [first, second],
        });
        flux_bank(
            parameter_panel,
            &sliders,
            &action_rows,
            scale,
            band,
            graphs_rect.y0 - 6.0 * scale,
        )
    } else {
        flux_bank(
            parameter_panel,
            &sliders,
            &action_rows,
            scale,
            band,
            parameter_panel.y1,
        )
    };
    TreeLayout {
        rect,
        rows: Vec::new(),
        sliders,
        actions: action_rows,
        kernels: Vec::new(),
        temporal: None,
        flux_bank,
        onset_graphs,
        parameter_title: format!("ONSET FLOW B{band:02}"),
        parameter_panel,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_flux_analyzer(
    fonts: &mut Fonts,
    scene: &mut Scene,
    view: &View,
    history: &DebugHistory,
    snapshot: &DebugSnapshot,
    visualizer: &Visualizer,
    palette: Palette,
    colors: MeterColors,
    text_scale: VisualizerTextScale,
    band: usize,
    pointer: Option<Point>,
) {
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let layout = flux_analyzer_layout(view, text_scale, band);
    let rect = layout.rect;
    if rect.width() < TREE_MIN_VISIBLE_WIDTH * scale || rect.height() < 200.0 * scale {
        return;
    }
    let color = palette.vivid(9);
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
        "ONSET FLOW ANALYZER",
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
        &format!("B{band:02}"),
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
        "bars select band / sliders tune / graphs show internals",
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
    draw_slider_content(
        fonts,
        scene,
        layout.parameter_panel,
        &layout.parameter_title,
        &layout.sliders,
        &layout.actions,
        &[],
        None,
        visualizer,
        palette,
        colors,
        scale,
        pointer,
    );
    if let Some(bank) = layout.flux_bank {
        draw_flux_bank(fonts, scene, bank, snapshot, palette, scale, pointer);
    }
    if let Some(graphs) = layout.onset_graphs {
        draw_onset_internals(
            fonts, scene, graphs, history, snapshot, band, palette, scale,
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
            let contribution = Contribution::Fixed(point, index);
            if let Some(value) = visualizer.mixer().constant_value(contribution) {
                return format!("{value:.2}");
            }
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
            Detector::Flux => palette.vivid(9),
            Detector::Kick => palette.vivid(0),
            Detector::Snare => palette.vivid(2),
            Detector::Drop => palette.vivid(8),
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
        TreeAction::ClearPattern(_) => "CLEAR PATTERN".to_owned(),
        TreeAction::TogglePatternOverlay => "TOGGLE KERNEL OVERLAY".to_owned(),
        TreeAction::TogglePatternPreview => "TOGGLE KERNEL PREVIEW".to_owned(),
        TreeAction::TogglePatternPause => "TOGGLE SPECTROGRAM PAUSE".to_owned(),
        TreeAction::TogglePatternSuppression => "TOGGLE SUPPRESSION KERNEL".to_owned(),
    }
}

fn envelope_shape(target: EnvelopeTarget, visualizer: &Visualizer) -> Option<InputShape> {
    let input = match target {
        EnvelopeTarget::Band(component) => Input::Band(component),
        EnvelopeTarget::BandIndex(band) => return Some(visualizer.band_shape(band)),
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
    kernels: &[KernelBank],
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
    if sliders.is_empty() && actions.is_empty() && kernels.is_empty() {
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
    draw_kernel_banks(
        fonts, scene, panel, kernels, visualizer, palette, scale, pointer,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_flux_bank(
    fonts: &mut Fonts,
    scene: &mut Scene,
    bank: FluxBank,
    snapshot: &DebugSnapshot,
    palette: Palette,
    scale: f64,
    pointer: Option<Point>,
) {
    let title = fit(
        fonts,
        "ONSET FLOW",
        (7.0 * scale) as f32,
        UiFace::SemiBold,
        bank.rect.width(),
    );
    draw_left_center(
        scene,
        &title,
        bank.rect.x0,
        bank.rect.y0 - 4.5 * scale,
        palette.vivid(9),
    );
    let selected = fit(
        fonts,
        &format!("B{:02}", bank.selected),
        (7.0 * scale) as f32,
        UiFace::SemiBold,
        36.0 * scale,
    );
    draw_right_center(
        scene,
        &selected,
        bank.rect.x1,
        bank.rect.y0 - 4.5 * scale,
        palette.neutral[0],
    );
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.05),
        None,
        &bank.rect,
    );
    scene.stroke(
        &Stroke::new(0.7 * scale),
        Affine::IDENTITY,
        palette.vivid(9).with_alpha(0.22),
        None,
        &bank.rect,
    );
    let peak = snapshot
        .bands
        .iter()
        .map(|band| band.onset.max(band.excess))
        .fold(0.0_f32, f32::max)
        .max(1e-4);
    let column_width = bank.rect.width() / BAND_COUNT as f64;
    let hovered = pointer
        .filter(|point| bank.rect.contains(*point))
        .map(|point| (((point.x - bank.rect.x0) / column_width) as usize).min(BAND_COUNT - 1));
    for (band, debug) in snapshot.bands.iter().enumerate() {
        let column_x0 = bank.rect.x0 + band as f64 * column_width;
        let column_x1 = column_x0 + column_width;
        let is_selected = band == bank.selected;
        if hovered == Some(band) {
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                palette.neutral[0].with_alpha(0.14),
                None,
                &Rect::new(column_x0, bank.rect.y0, column_x1, bank.rect.y1),
            );
        }
        let color = palette.vivid(band);
        let level = (debug.onset / peak).clamp(0.0, 1.0);
        let top = bank.rect.y1 - f64::from(level) * bank.rect.height();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            color.with_alpha(if is_selected { 0.8 } else { 0.5 }),
            None,
            &Rect::new(
                column_x0 + 0.5 * scale,
                top,
                column_x1 - 0.5 * scale,
                bank.rect.y1,
            ),
        );
        let raw = (debug.excess / peak).clamp(0.0, 1.0);
        let raw_y = bank.rect.y1 - f64::from(raw) * bank.rect.height();
        scene.stroke(
            &Stroke::new(0.7 * scale),
            Affine::IDENTITY,
            color.with_alpha(if is_selected { 0.75 } else { 0.35 }),
            None,
            &Line::new(
                Point::new(column_x0 + 0.5 * scale, raw_y),
                Point::new(column_x1 - 0.5 * scale, raw_y),
            ),
        );
        if is_selected {
            scene.stroke(
                &Stroke::new(1.0 * scale),
                Affine::IDENTITY,
                palette.neutral[0].with_alpha(0.8),
                None,
                &Rect::new(column_x0, bank.rect.y0, column_x1, bank.rect.y1),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_onset_internals(
    fonts: &mut Fonts,
    scene: &mut Scene,
    graphs: OnsetGraphs,
    history: &DebugHistory,
    snapshot: &DebugSnapshot,
    band: usize,
    palette: Palette,
    scale: f64,
) {
    let title = fit(
        fonts,
        "ONSET INTERNALS",
        (7.0 * scale) as f32,
        UiFace::SemiBold,
        graphs.rect.width(),
    );
    draw_left_center(
        scene,
        &title,
        graphs.rect.x0,
        graphs.rect.y0 + 4.5 * scale,
        palette.vivid(9),
    );
    let debug = &snapshot.bands[band];
    let wsdr = snapshot
        .bands
        .iter()
        .map(|candidate| candidate.abdr)
        .sum::<f32>()
        / BAND_COUNT as f32;
    let drp_peak = history
        .samples()
        .map(|sample| sample.band_drp[band])
        .fold(1.0_f32, f32::max)
        .max(debug.drp);
    let range_label = format!(
        "RANGE  ABDR {:.2}  WSDR {:.2}  DRP {:.2}",
        debug.abdr, wsdr, debug.drp
    );
    let range_plot =
        draw_internal_lane(fonts, scene, graphs.lanes[0], &range_label, palette, scale);
    draw_trace(
        scene,
        history,
        range_plot,
        (0.0, drp_peak),
        TraceStyle {
            color: palette.vivid(band),
            width: 1.0 * scale,
            start: 0.0,
        },
        |sample| sample.band_abdr[band],
    );
    draw_trace(
        scene,
        history,
        range_plot,
        (0.0, drp_peak),
        TraceStyle {
            color: palette.neutral[1].with_alpha(0.55),
            width: 0.8 * scale,
            start: 0.0,
        },
        |sample| sample.band_abdr.iter().sum::<f32>() / BAND_COUNT as f32,
    );
    draw_trace(
        scene,
        history,
        range_plot,
        (0.0, drp_peak),
        TraceStyle {
            color: palette.vivid(9),
            width: 0.9 * scale,
            start: 0.0,
        },
        |sample| sample.band_drp[band],
    );

    let flow_peak = history
        .samples()
        .flat_map(|sample| {
            [
                sample.band_abl[band],
                sample.band_excess[band],
                sample.band_onset[band],
            ]
        })
        .fold(1.0_f32, f32::max)
        .max(debug.abl)
        .max(debug.excess)
        .max(debug.onset);
    let flow_label = format!(
        "FLOW  ABL {:.2}  EXCESS {:.3}  ONSET {:.3}",
        debug.abl, debug.excess, debug.onset
    );
    let flow_plot = draw_internal_lane(fonts, scene, graphs.lanes[1], &flow_label, palette, scale);
    draw_trace(
        scene,
        history,
        flow_plot,
        (0.0, flow_peak),
        TraceStyle {
            color: palette.vivid(9),
            width: 0.9 * scale,
            start: 0.0,
        },
        |sample| sample.band_abl[band],
    );
    draw_trace(
        scene,
        history,
        flow_plot,
        (0.0, flow_peak),
        TraceStyle {
            color: palette.vivid(band).with_alpha(0.45),
            width: 0.8 * scale,
            start: 0.0,
        },
        |sample| sample.band_excess[band],
    );
    draw_trace(
        scene,
        history,
        flow_plot,
        (0.0, flow_peak),
        TraceStyle {
            color: palette.vivid(band),
            width: 1.0 * scale,
            start: 0.0,
        },
        |sample| sample.band_onset[band],
    );
}

fn draw_internal_lane(
    fonts: &mut Fonts,
    scene: &mut Scene,
    lane: Rect,
    label: &str,
    palette: Palette,
    scale: f64,
) -> Rect {
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.04),
        None,
        &lane,
    );
    scene.stroke(
        &Stroke::new(0.7 * scale),
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.25),
        None,
        &lane,
    );
    let label = fit(
        fonts,
        label,
        (7.0 * scale) as f32,
        UiFace::Light,
        lane.width() - 4.0 * scale,
    );
    draw_left_center(
        scene,
        &label,
        lane.x0 + 2.0 * scale,
        lane.y0 + 4.5 * scale,
        palette.neutral[1],
    );
    Rect::new(
        lane.x0 + 2.0 * scale,
        lane.y0 + 9.0 * scale,
        lane.x1 - 2.0 * scale,
        lane.y1 - 1.0 * scale,
    )
}

fn intensity_color(palette: Palette, value: f32) -> Color {
    let value = value.clamp(0.0, 1.0);
    let stops = [
        (0.0, palette.vivid[0].with_alpha(0.0)),
        (0.18, palette.vivid[0]),
        (0.38, palette.vivid[4]),
        (0.55, palette.vivid[3]),
        (0.72, palette.vivid[5]),
        (0.86, palette.vivid[1]),
        (1.0, palette.neutral[0]),
    ];
    for pair in stops.windows(2) {
        let (start, from) = pair[0];
        let (end, to) = pair[1];
        if value <= end {
            let progress = ((value - start) / (end - start)).clamp(0.0, 1.0);
            return mix(from, to, progress);
        }
    }
    palette.neutral[0]
}

#[allow(clippy::too_many_arguments)]
fn draw_temporal_bank(
    fonts: &mut Fonts,
    scene: &mut Scene,
    temporal: TemporalBank,
    visualizer: &Visualizer,
    palette: Palette,
    scale: f64,
    view: &TemporalView,
    pointer: Option<Point>,
) {
    let settings = visualizer.detector_settings();
    let pattern = match temporal.detector {
        Detector::Kick => &settings.kick_pattern,
        Detector::Snare => &settings.snare_pattern,
        Detector::Drop | Detector::Flux => return,
    };
    let kernel = view.kernel;
    let hop = visualizer.hop_seconds();
    let max_steps = (0.5 / hop).floor().max(1.0) as usize;
    let live_history = visualizer.onset_history();
    let history = if view.paused {
        view.frozen.as_ref().unwrap_or(live_history)
    } else {
        live_history
    };
    let steps = history.len().min(max_steps);
    let start = history.len() - steps;
    let shift = if view.paused {
        f64::from(view.offset / 0.5) * temporal.canvas.width()
    } else {
        0.0
    };
    draw_temporal_spectrogram(
        scene,
        temporal.canvas,
        history,
        start,
        steps,
        shift,
        palette,
        scale,
    );
    if view.preview {
        draw_kernel_heat(scene, temporal.canvas, pattern, kernel, palette, 0.4);
    }
    let hovered = if view.overlay {
        pointer
            .filter(|point| temporal.canvas.contains(*point))
            .and_then(|point| pattern_hit(pattern, kernel, temporal.canvas, point, scale))
    } else {
        None
    };
    if view.overlay {
        let (inner, outer) = pattern.polygons(kernel);
        draw_polygon(
            scene,
            temporal.canvas,
            inner,
            outer,
            palette,
            scale,
            hovered,
        );
        let effective_line = temporal.canvas.x0
            + temporal.canvas.width() * f64::from(pattern.effective.clamp(0.1, 1.0));
        scene.stroke(
            &Stroke::new(1.0 * scale),
            Affine::IDENTITY,
            palette.vivid[2].with_alpha(0.8),
            None,
            &Line::new(
                Point::new(effective_line, temporal.canvas.y0),
                Point::new(effective_line, temporal.canvas.y1),
            ),
        );
    }

    let input_label = fit(
        fonts,
        "SPECTROGRAM 0.50s",
        (7.0 * scale) as f32,
        UiFace::SemiBold,
        temporal.canvas.width(),
    );
    draw_left_center(
        scene,
        &input_label,
        temporal.canvas.x0 + 2.0 * scale,
        temporal.canvas.y0 + 4.5 * scale,
        palette.neutral[0],
    );
    let kernel_label = fit(
        fonts,
        &format!(
            "{} {:.0}%",
            if view.kernel == PatternKernel::Suppression {
                "SUPPRESS"
            } else {
                "KERNEL"
            },
            pattern.effective * 100.0
        ),
        (7.0 * scale) as f32,
        UiFace::SemiBold,
        80.0 * scale,
    );
    draw_right_center(
        scene,
        &kernel_label,
        temporal.canvas.x1 - 2.0 * scale,
        temporal.canvas.y0 + 4.5 * scale,
        palette.vivid[2],
    );

    for (row, label, active) in [
        (temporal.clear, "CLEAR", false),
        (temporal.overlay, "OVERLAY", view.overlay),
        (temporal.preview, "PREVIEW", view.preview),
        (temporal.pause, "PAUSE", view.paused),
        (
            temporal.suppress,
            "SUPPRESS",
            view.kernel == PatternKernel::Suppression,
        ),
    ] {
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            if active {
                palette.vivid[2].with_alpha(0.22)
            } else {
                palette.neutral[0].with_alpha(0.08)
            },
            None,
            &row.rect,
        );
        scene.stroke(
            &Stroke::new(0.7 * scale),
            Affine::IDENTITY,
            if active {
                palette.vivid[2]
            } else {
                palette.neutral[0].with_alpha(0.35)
            },
            None,
            &row.rect,
        );
        let text = fit(
            fonts,
            label,
            (7.0 * scale) as f32,
            UiFace::SemiBold,
            row.rect.width() - 4.0 * scale,
        );
        draw_left_center(
            scene,
            &text,
            row.rect.x0 + 2.0 * scale,
            row.rect.center().y,
            if active {
                palette.vivid[2]
            } else {
                palette.neutral[1]
            },
        );
    }

    draw_offset_control(fonts, scene, temporal.offset, view, palette, scale);
}

#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
fn draw_temporal_spectrogram(
    scene: &mut Scene,
    rect: Rect,
    history: &VecDeque<[f32; BAND_COUNT]>,
    start: usize,
    steps: usize,
    shift: f64,
    palette: Palette,
    scale: f64,
) {
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        palette.neutral[3].with_alpha(0.5),
        None,
        &rect,
    );
    scene.stroke(
        &Stroke::new(0.7 * scale),
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.25),
        None,
        &rect,
    );
    if steps == 0 || rect.height() <= 0.0 {
        return;
    }
    let row_height = rect.height() / BAND_COUNT as f64;
    let column_width = rect.width() / steps as f64;
    for band in 0..BAND_COUNT {
        let y0 = rect.y0 + band as f64 * row_height;
        for step in 0..steps {
            let value = history[start + step][band].clamp(0.0, 1.0);
            if value <= 0.0 {
                continue;
            }
            let x0 = rect.x0 + shift + step as f64 * column_width;
            let x1 = x0 + column_width.max(1.0);
            let x0 = x0.max(rect.x0);
            let x1 = x1.min(rect.x1);
            if x1 <= x0 {
                continue;
            }
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                intensity_color(palette, value),
                None,
                &Rect::new(x0, y0, x1, (y0 + row_height.max(1.0)).min(rect.y1)),
            );
        }
    }
}

fn draw_kernel_heat(
    scene: &mut Scene,
    rect: Rect,
    pattern: &TemporalPattern,
    kernel: PatternKernel,
    palette: Palette,
    alpha: f32,
) {
    let steps = 32;
    let row_height = rect.height() / BAND_COUNT as f64;
    let column_width = rect.width() / steps as f64;
    for band in 0..BAND_COUNT {
        let y0 = rect.y0 + band as f64 * row_height;
        for step in 0..steps {
            let x = (step as f32 + 0.5) / steps as f32;
            let y = (band as f32 + 0.5) / BAND_COUNT as f32;
            let value = pattern.effective_field_value_for(kernel, x, y);
            if value <= 0.0 {
                continue;
            }
            let color = intensity_color(palette, value).with_alpha(alpha * value);
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                color,
                None,
                &Rect::new(
                    rect.x0 + step as f64 * column_width,
                    y0,
                    rect.x0 + (step + 1) as f64 * column_width,
                    y0 + row_height.max(1.0),
                ),
            );
        }
    }
}

fn draw_polygon(
    scene: &mut Scene,
    rect: Rect,
    inner: &[[f32; 2]],
    outer: &[[f32; 2]],
    palette: Palette,
    scale: f64,
    hovered: Option<PatternHit>,
) {
    let map = |point: &[f32; 2]| {
        Point::new(
            rect.x0 + f64::from(point[0]) * rect.width(),
            rect.y0 + f64::from(point[1]) * rect.height(),
        )
    };
    for (layer, points, alpha) in [
        (PatternLayer::Outer, outer, 0.75),
        (PatternLayer::Inner, inner, 1.0),
    ] {
        if points.len() < 3 {
            continue;
        }
        let mut path = BezPath::new();
        path.move_to(map(&points[0]));
        for point in &points[1..] {
            path.line_to(map(point));
        }
        path.close_path();
        scene.stroke(
            &Stroke::new(1.3 * scale),
            Affine::IDENTITY,
            palette.vivid[2].with_alpha(alpha),
            None,
            &path,
        );
        for index in 0..points.len() {
            if let Some(PatternHit::Edge {
                layer: edge_layer,
                index: edge_index,
                ..
            }) = hovered
            {
                if edge_layer == layer && edge_index == index {
                    let a = map(&points[index]);
                    let b = map(&points[(index + 1) % points.len()]);
                    scene.stroke(
                        &Stroke::new(2.2 * scale),
                        Affine::IDENTITY,
                        palette.neutral[0],
                        None,
                        &Line::new(a, b),
                    );
                }
            }
        }
        for (index, point) in points.iter().enumerate() {
            let center = map(point);
            let radius = if matches!(
                hovered,
                Some(PatternHit::Vertex {
                    layer: hover_layer,
                    index: hover_index,
                }) if hover_layer == layer && hover_index == index
            ) {
                PATTERN_POINT_RADIUS * 1.7 * scale
            } else {
                PATTERN_POINT_RADIUS * scale
            };
            let hovered_point = matches!(
                hovered,
                Some(PatternHit::Vertex {
                    layer: hover_layer,
                    index: hover_index,
                }) if hover_layer == layer && hover_index == index
            );
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                palette.vivid[2],
                None,
                &Rect::new(
                    center.x - radius,
                    center.y - radius,
                    center.x + radius,
                    center.y + radius,
                ),
            );
            if hovered_point {
                scene.stroke(
                    &Stroke::new(1.0 * scale),
                    Affine::IDENTITY,
                    palette.neutral[0],
                    None,
                    &Rect::new(
                        center.x - radius - 1.5 * scale,
                        center.y - radius - 1.5 * scale,
                        center.x + radius + 1.5 * scale,
                        center.y + radius + 1.5 * scale,
                    ),
                );
            }
        }
    }
}

fn draw_offset_control(
    fonts: &mut Fonts,
    scene: &mut Scene,
    rect: Rect,
    view: &TemporalView,
    palette: Palette,
    scale: f64,
) {
    let center_y = rect.center().y;
    scene.stroke(
        &Stroke::new(0.8 * scale),
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.3),
        None,
        &Line::new(Point::new(rect.x0, center_y), Point::new(rect.x1, center_y)),
    );
    scene.stroke(
        &Stroke::new(1.0 * scale),
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.6),
        None,
        &Line::new(
            Point::new(rect.center().x, rect.y0),
            Point::new(rect.center().x, rect.y1),
        ),
    );
    let fraction = (view.offset / 0.5 + 0.5).clamp(0.0, 1.0) as f64;
    let knob_x = rect.x0 + rect.width() * fraction;
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        palette.vivid[2],
        None,
        &Rect::new(
            knob_x - 2.0 * scale,
            rect.y0 + scale,
            knob_x + 2.0 * scale,
            rect.y1 - scale,
        ),
    );
    let label = fit(
        fonts,
        &format!("OFFSET {:+.2}s", view.offset),
        (6.5 * scale) as f32,
        UiFace::Light,
        rect.width(),
    );
    draw_left_center(
        scene,
        &label,
        rect.x0 + 2.0 * scale,
        center_y,
        palette.neutral[1],
    );
}

fn draw_kernel_banks(
    fonts: &mut Fonts,
    scene: &mut Scene,
    panel: Rect,
    kernels: &[KernelBank],
    visualizer: &Visualizer,
    palette: Palette,
    scale: f64,
    pointer: Option<Point>,
) {
    let settings = visualizer.detector_settings();
    for bank in kernels {
        let (label_text, color) = match bank.kernel {
            DropKernel::Low => ("LOW", palette.vivid(6)),
            DropKernel::High => ("HIGH", palette.vivid(10)),
            DropKernel::Onset => ("ONSET", palette.vivid(9)),
        };
        let label = fit(
            fonts,
            label_text,
            (7.0 * scale) as f32,
            UiFace::SemiBold,
            (bank.rect.x0 - panel.x0 - 4.0 * scale).max(1.0),
        );
        draw_left_center(scene, &label, panel.x0, bank.rect.center().y, color);
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            palette.neutral[0].with_alpha(0.05),
            None,
            &bank.rect,
        );
        scene.stroke(
            &Stroke::new(0.7 * scale),
            Affine::IDENTITY,
            color.with_alpha(0.22),
            None,
            &bank.rect,
        );
        let weights = settings.drop_kernel_weights(bank.kernel);
        let column_width = bank.rect.width() / BAND_COUNT as f64;
        let hovered = pointer
            .filter(|point| bank.rect.contains(*point))
            .map(|point| (((point.x - bank.rect.x0) / column_width) as usize).min(BAND_COUNT - 1));
        for (band, weight) in weights.iter().enumerate() {
            let column_x0 = bank.rect.x0 + band as f64 * column_width;
            let column_x1 = column_x0 + column_width;
            if hovered == Some(band) {
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    palette.neutral[0].with_alpha(0.14),
                    None,
                    &Rect::new(column_x0, bank.rect.y0, column_x1, bank.rect.y1),
                );
            }
            let top = bank.rect.y1 - f64::from(*weight) * bank.rect.height();
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                color.with_alpha(0.55),
                None,
                &Rect::new(
                    column_x0 + 0.5 * scale,
                    top,
                    column_x1 - 0.5 * scale,
                    bank.rect.y1,
                ),
            );
        }
        scene.stroke(
            &Stroke::new(0.7 * scale),
            Affine::IDENTITY,
            palette.neutral[2].with_alpha(0.35),
            None,
            &Line::new(
                Point::new(bank.rect.x0, bank.rect.y1),
                Point::new(bank.rect.x1, bank.rect.y1),
            ),
        );
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
        Input::PowerLevel => Some(format!("{:.2}", snapshot.power.raw)),
        Input::Bass => Some(format!("{:.2}", snapshot.bass.smooth)),
        Input::BassLevel => Some(format!("{:.2}", snapshot.bass.raw)),
        Input::Kick => Some(format!("{:.2}", snapshot.kick)),
        Input::KickOnset => Some(trigger_state(&snapshot.kick_onset).to_owned()),
        Input::SnareOnset => Some(trigger_state(&snapshot.snare_onset).to_owned()),
        Input::DropOnset => Some(trigger_state(&snapshot.drop_onset).to_owned()),
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

fn trigger_state(trigger: &TriggerStatus) -> &'static str {
    if trigger.choked {
        "CHOKED"
    } else if trigger.flash > 0.0 {
        "TRIGGER"
    } else if trigger.armed {
        "ARMED"
    } else {
        "IDLE"
    }
}

fn input_color(input: Input, palette: Palette, routing: &BandRouting) -> Color {
    match input {
        Input::Band(component) => palette.vivid(routing[component]),
        Input::BassBands | Input::Bass | Input::BassLevel => palette.vivid(6),
        Input::Power | Input::PowerLevel | Input::DropOnset => palette.vivid(8),
        Input::Kick | Input::KickOnset => palette.vivid(0),
        Input::SnareOnset => palette.vivid(2),
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
    /// Raw relative excess and the processed continuous onset flow.
    BandOnset(usize),
    BandRole(BandComponent),
    Power,
    Bass,
    Onset(Detector),
    Kick,
    Property(Property),
    /// Weighted low projection of the band levels.
    DropLow,
    /// Weighted high projection of the band levels.
    DropHigh,
    /// High projection minus low projection.
    DropDiff,
    /// Mean high-minus-low difference over the rolling window.
    DropConfidence,
    /// Whether the window confidence currently permits a drop.
    DropEligible,
    /// Composite onset flux with its adaptive threshold.
    DetectorOnsetFlux(Detector),
    /// Inverted-kernel flux the detector subtracts, drawn below the onset graph.
    DetectorSuppression(Detector),
}

/// What the extension pane is diagnosing. A detector selected in the tree
/// replaces the property graphs even though the tree keeps the property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InspectorFocus {
    Selection(DebugSelection),
    Detector(Detector),
    /// The per-band onset analyzer, with the selected band.
    Flux(usize),
}

fn inspector_focus(
    selection: DebugSelection,
    tree_target: Option<TreeTarget>,
    flux_band: usize,
) -> InspectorFocus {
    if selection == DebugSelection::Flux {
        return InspectorFocus::Flux(flux_band);
    }
    let detector = match tree_target {
        Some(TreeTarget::Detector(detector)) => Some(detector),
        Some(TreeTarget::Input(Input::KickOnset)) => Some(Detector::Kick),
        Some(TreeTarget::Input(Input::SnareOnset)) => Some(Detector::Snare),
        Some(TreeTarget::Input(Input::DropOnset)) => Some(Detector::Drop),
        _ => None,
    };
    match detector {
        Some(detector @ (Detector::Kick | Detector::Snare | Detector::Drop)) => {
            InspectorFocus::Detector(detector)
        }
        _ => InspectorFocus::Selection(selection),
    }
}

pub(super) fn detector_focused(
    selection: DebugSelection,
    tree_target: Option<TreeTarget>,
    flux_band: usize,
) -> bool {
    matches!(
        inspector_focus(selection, tree_target, flux_band),
        InspectorFocus::Detector(_) | InspectorFocus::Flux(_)
    )
}

impl InspectorFocus {
    fn color(self, palette: Palette) -> Color {
        match self {
            Self::Selection(selection) => selection.color(palette),
            Self::Detector(Detector::Kick) => palette.vivid(0),
            Self::Detector(Detector::Snare) => palette.vivid(2),
            Self::Detector(Detector::Drop) | Self::Detector(Detector::Flux) => palette.vivid(8),
            Self::Flux(_) => palette.vivid(9),
        }
    }
}

fn trigger_sample(sample: &DebugSample, detector: Detector) -> TriggerStatus {
    match detector {
        Detector::Flux => TriggerStatus::ZERO,
        Detector::Kick => sample.kick_onset,
        Detector::Snare => sample.snare_onset,
        Detector::Drop => sample.drop_onset,
    }
}

fn trigger_snapshot(snapshot: &DebugSnapshot, detector: Detector) -> TriggerStatus {
    match detector {
        Detector::Flux => TriggerStatus::ZERO,
        Detector::Kick => snapshot.kick_onset,
        Detector::Snare => snapshot.snare_onset,
        Detector::Drop => snapshot.drop_onset,
    }
}

fn onset_flux_snapshot(snapshot: &DebugSnapshot, detector: Detector) -> OnsetFluxStatus {
    match detector {
        Detector::Kick => snapshot.kick_flux,
        Detector::Snare => snapshot.snare_flux,
        Detector::Drop => snapshot.drop_status.onset,
        Detector::Flux => OnsetFluxStatus::default(),
    }
}

fn onset_flux_sample(sample: &DebugSample, detector: Detector) -> OnsetFluxStatus {
    match detector {
        Detector::Kick => sample.kick_flux,
        Detector::Snare => sample.snare_flux,
        Detector::Drop => sample.drop_status.onset,
        Detector::Flux => OnsetFluxStatus::default(),
    }
}

impl GraphSource {
    fn has_raw(self) -> bool {
        matches!(
            self,
            Self::Band(_)
                | Self::BandOnset(_)
                | Self::BandRole(_)
                | Self::Power
                | Self::Bass
                | Self::DetectorOnsetFlux(_)
                | Self::DetectorSuppression(_)
        )
    }

    /// Suppression grows downward from the top of its rect, away from the
    /// onset graph above it.
    fn inverted(self) -> bool {
        matches!(self, Self::DetectorSuppression(_))
    }

    fn range(self, graph_max: Option<GraphMax>) -> (f32, f32) {
        match self {
            Self::Band(_) | Self::BandRole(_) | Self::Power | Self::Bass => (0.0, 1.0),
            Self::BandOnset(_) => (0.0, 1.0),
            Self::Onset(_) | Self::DropLow | Self::DropHigh | Self::DropEligible => (0.0, 1.0),
            Self::DropDiff | Self::DropConfidence => {
                (DROP_GAP_THRESHOLD_MIN, DROP_GAP_THRESHOLD_MAX)
            }
            Self::DetectorOnsetFlux(_) | Self::DetectorSuppression(_) => (0.0, 0.25),
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
            Self::Band(index) | Self::BandOnset(index) => palette.vivid(index),
            Self::BandRole(component) => palette.vivid(routing[component]),
            Self::Power => palette.vivid(8),
            Self::Bass => palette.vivid(6),
            Self::Onset(Detector::Flux) => palette.vivid(9),
            Self::Onset(Detector::Kick) => palette.vivid(0),
            Self::Onset(Detector::Snare) => palette.vivid(2),
            Self::Onset(Detector::Drop) | Self::DropEligible => palette.vivid(8),
            Self::Kick => palette.vivid(0),
            Self::Property(property) => palette.vivid(property.index()),
            Self::DropLow => palette.vivid(6),
            Self::DropHigh => palette.vivid(10),
            Self::DropDiff => palette.vivid(2),
            Self::DropConfidence => palette.vivid(4),
            Self::DetectorOnsetFlux(Detector::Kick) | Self::DetectorSuppression(Detector::Kick) => {
                palette.vivid(0)
            }
            Self::DetectorOnsetFlux(Detector::Snare)
            | Self::DetectorSuppression(Detector::Snare) => palette.vivid(2),
            Self::DetectorOnsetFlux(Detector::Drop)
            | Self::DetectorOnsetFlux(Detector::Flux)
            | Self::DetectorSuppression(Detector::Drop)
            | Self::DetectorSuppression(Detector::Flux) => palette.vivid(9),
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
            Self::BandOnset(index) => {
                format!("B{:02} ONSET {:.3}", index, snapshot.bands[index].onset)
            }
            Self::BandRole(component) => {
                let index = routing[component];
                format!(
                    "B{:02} {} {:.2}",
                    index,
                    component.label(),
                    snapshot.bands[index].value
                )
            }
            Self::Power => format!("PWR {:.2}", snapshot.power.smooth),
            Self::Bass => format!("BASS {:.2}", snapshot.bass.smooth),
            Self::Onset(detector) => {
                let trigger = trigger_snapshot(snapshot, detector);
                format!(
                    "{} {} {:.2}",
                    detector.name(),
                    trigger_state(&trigger),
                    trigger.power
                )
            }
            Self::Kick => format!("KICK {:.2}", snapshot.kick),
            Self::Property(property) => format!(
                "{} {:.2}",
                property.name(),
                snapshot.properties[property.index()]
            ),
            Self::DropLow => format!("LF {:.2}", snapshot.drop_status.low),
            Self::DropHigh => format!("HF {:.2}", snapshot.drop_status.high),
            Self::DropDiff => format!("DIFF {:.2}", snapshot.drop_status.diff),
            Self::DropConfidence => format!("CONF {:.2}", snapshot.drop_status.confidence),
            Self::DropEligible => format!("ELIG {}", u8::from(snapshot.drop_onset.armed)),
            Self::DetectorOnsetFlux(detector) => {
                let onset = onset_flux_snapshot(snapshot, detector);
                format!("ONSET {:.3}/{:.3}", onset.smoothed, onset.threshold)
            }
            Self::DetectorSuppression(detector) => {
                let onset = onset_flux_snapshot(snapshot, detector);
                format!("SUP {:.3}", onset.suppressed)
            }
        }
    }

    /// Short captions for graphs that only exist in the drop detector view.
    fn detector_usage(self) -> Option<&'static str> {
        match self {
            Self::DropLow => Some("low projection"),
            Self::DropHigh => Some("high projection"),
            Self::DropDiff => Some("high - low"),
            Self::DropConfidence => Some("window mean"),
            Self::DropEligible => Some("latched gap"),
            Self::DetectorOnsetFlux(_) => Some("kernel onset flux"),
            Self::DetectorSuppression(_) => Some("suppression"),
            _ => None,
        }
    }

    fn value(self, sample: &DebugSample, raw: bool, routing: &BandRouting) -> Option<f32> {
        match self {
            Self::Band(index) => Some(if raw {
                sample.raw_bands[index]
            } else {
                sample.bands[index]
            }),
            Self::BandOnset(index) => Some(if raw {
                sample.band_excess[index]
            } else {
                sample.band_onset[index]
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
            Self::Onset(detector) => (!raw).then(|| trigger_sample(sample, detector).flash),
            Self::Kick => (!raw).then_some(sample.kick),
            Self::Property(property) => (!raw).then(|| sample.property(property)),
            Self::DropLow => (!raw).then_some(sample.drop_status.low),
            Self::DropHigh => (!raw).then_some(sample.drop_status.high),
            Self::DropDiff => (!raw).then_some(sample.drop_status.diff),
            Self::DropConfidence => (!raw).then_some(sample.drop_status.confidence),
            Self::DropEligible => (!raw).then_some(if sample.drop_onset.armed { 1.0 } else { 0.0 }),
            Self::DetectorOnsetFlux(detector) => {
                let onset = onset_flux_sample(sample, detector);
                Some(if raw { onset.flux } else { onset.smoothed })
            }
            Self::DetectorSuppression(detector) => {
                let onset = onset_flux_sample(sample, detector);
                Some(if raw {
                    onset.inhibition
                } else {
                    onset.suppressed
                })
            }
        }
    }

    /// The shaped input whose adaptive references this graph belongs to.
    fn shape(self, visualizer: &Visualizer) -> Option<InputShape> {
        match self {
            Self::Band(index) => Some(visualizer.band_shape(index)),
            Self::BandOnset(_) => None,
            Self::BandRole(component) => visualizer.input_shape(Input::Band(component)),
            Self::Power => visualizer.input_shape(Input::Power),
            Self::Bass => visualizer.input_shape(Input::Bass),
            Self::Kick
            | Self::Onset(_)
            | Self::Property(_)
            | Self::DropLow
            | Self::DropHigh
            | Self::DropDiff
            | Self::DropConfidence
            | Self::DropEligible
            | Self::DetectorOnsetFlux(_)
            | Self::DetectorSuppression(_) => None,
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

/// The diagnostic traces available for one detector focus.
fn detector_graph_sources(detector: Detector, flux_band: usize) -> Vec<GraphSource> {
    match detector {
        Detector::Drop => vec![
            GraphSource::DropLow,
            GraphSource::DropHigh,
            GraphSource::DropDiff,
            GraphSource::DropConfidence,
            GraphSource::DropEligible,
            GraphSource::DetectorOnsetFlux(Detector::Drop),
            GraphSource::DetectorSuppression(Detector::Drop),
            GraphSource::Onset(Detector::Drop),
        ],
        Detector::Kick => vec![
            GraphSource::DetectorOnsetFlux(Detector::Kick),
            GraphSource::DetectorSuppression(Detector::Kick),
            GraphSource::Onset(Detector::Kick),
        ],
        Detector::Snare => vec![
            GraphSource::DetectorOnsetFlux(Detector::Snare),
            GraphSource::DetectorSuppression(Detector::Snare),
            GraphSource::Onset(Detector::Snare),
        ],
        Detector::Flux => vec![
            GraphSource::BandOnset(flux_band),
            GraphSource::Band(flux_band),
        ],
    }
}

fn graph_sources(selection: DebugSelection, visualizer: &Visualizer) -> Vec<GraphSource> {
    let mut graphs = Vec::new();
    let mut push = |source: GraphSource| {
        if !graphs.contains(&source) {
            graphs.push(source);
        }
    };
    let detectors = visualizer.detector_settings();
    match selection {
        DebugSelection::Band(index) => {
            push(GraphSource::Band(index));
            push(GraphSource::BandOnset(index));
            if index < 4 {
                push(GraphSource::Bass);
            }
            if detectors.kick_contains(index) {
                push(GraphSource::Onset(Detector::Kick));
            }
            if detectors.snare_contains(index) {
                push(GraphSource::Onset(Detector::Snare));
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
                    Dependency::Power => push(GraphSource::Power),
                    Dependency::Bass => push(GraphSource::Bass),
                    Dependency::KickOnset => {
                        push(GraphSource::Kick);
                        push(GraphSource::Onset(Detector::Kick));
                    }
                    Dependency::SnareOnset => push(GraphSource::Onset(Detector::Snare)),
                    Dependency::DropOnset => {
                        push(GraphSource::Power);
                        push(GraphSource::Onset(Detector::Drop));
                    }
                    Dependency::Kick => push(GraphSource::Kick),
                }
            }
        }
        DebugSelection::Flux => {}
    }
    graphs
}

/// Graph cells inside the inspector extension, shared by drawing and hit
/// testing so the bottom maximum buttons stay under their visible rectangles.
fn extension_graph_cells(
    rect: Rect,
    scale: f64,
    focus: InspectorFocus,
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
    let graphs = match focus {
        InspectorFocus::Selection(selection) => graph_sources(selection, visualizer),
        InspectorFocus::Detector(detector) => detector_graph_sources(detector, 0),
        InspectorFocus::Flux(band) => detector_graph_sources(Detector::Flux, band),
    };
    if graphs.is_empty() || graphs_top >= inner.y1 {
        return (Vec::new(), 0);
    }
    let gap = 4.0 * scale;
    if let InspectorFocus::Flux(_) = focus {
        let rows = graphs.len();
        let row_height = (inner.y1 - graphs_top - gap * (rows - 1) as f64) / rows as f64;
        let cells = graphs
            .into_iter()
            .enumerate()
            .map(|(row, source)| {
                (
                    source,
                    Rect::from_origin_size(
                        Point::new(inner.x0, graphs_top + row as f64 * (row_height + gap)),
                        Size::new(inner.width(), row_height),
                    ),
                )
            })
            .collect();
        return (cells, rows);
    }
    if let InspectorFocus::Detector(detector) = focus {
        let onset = GraphSource::DetectorOnsetFlux(detector);
        let suppression = GraphSource::DetectorSuppression(detector);
        let mut others = Vec::with_capacity(graphs.len());
        let mut has_onset = false;
        for source in graphs {
            if source == onset {
                has_onset = true;
            } else if source != suppression {
                others.push(source);
            }
        }
        assert!(
            has_onset,
            "a detector focus always exposes its composite onset graph"
        );
        let total = others.len() + 2;
        let rows = 2;
        let columns = total.div_ceil(rows);
        let row_height = (inner.y1 - graphs_top - gap) / rows as f64;
        let width = (inner.width() - gap * (columns - 1) as f64) / columns as f64;
        let cell = |row: usize, column: usize| {
            Rect::from_origin_size(
                Point::new(
                    inner.x0 + column as f64 * (width + gap),
                    graphs_top + row as f64 * (row_height + gap),
                ),
                Size::new(width, row_height),
            )
        };
        let mut cells = Vec::with_capacity(total);
        cells.push((onset, cell(0, 0)));
        cells.push((suppression, cell(1, 0)));
        let second_row = columns - 1;
        for (index, source) in others.into_iter().enumerate() {
            let (row, column) = if index < second_row {
                (0, index + 1)
            } else {
                (1, index - second_row + 1)
            };
            cells.push((source, cell(row, column)));
        }
        return (cells, rows);
    }
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
    let rect = inspector_panes(view, text_scale).1;
    let scale = view.ui_scale() * f64::from(text_scale.factor());
    let (cells, _) = extension_graph_cells(
        rect,
        scale,
        InspectorFocus::Selection(selection),
        visualizer,
    );
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
#[cfg(test)]
fn graph_usage(selection: DebugSelection, source: GraphSource) -> &'static str {
    graph_usage_for_focus(InspectorFocus::Selection(selection), selection, source)
}

fn graph_usage_for_focus(
    focus: InspectorFocus,
    selection: DebugSelection,
    source: GraphSource,
) -> &'static str {
    if let InspectorFocus::Flux(_) = focus {
        return match source {
            GraphSource::BandOnset(_) => "excess + onset flow",
            GraphSource::Band(_) | GraphSource::BandRole(_) => "raw + low/high refs",
            _ => "",
        };
    }
    if let Some(usage) = source.detector_usage() {
        return usage;
    }
    match selection {
        DebugSelection::Band(_) => match source {
            GraphSource::Band(_) | GraphSource::BandRole(_) => "level + raw FFT",
            GraphSource::BandOnset(_) => "excess + onset flow",
            GraphSource::Bass => "low-band envelope",
            GraphSource::Power => "wide RMS level",
            GraphSource::Onset(_) => "trigger flash",
            GraphSource::Kick
            | GraphSource::Property(_)
            | GraphSource::DropLow
            | GraphSource::DropHigh
            | GraphSource::DropDiff
            | GraphSource::DropConfidence
            | GraphSource::DropEligible
            | GraphSource::DetectorOnsetFlux(_)
            | GraphSource::DetectorSuppression(_) => "",
        },
        DebugSelection::Property(property) => match source {
            GraphSource::Property(_) => "resulting value",
            GraphSource::Band(_) | GraphSource::BandOnset(_) => match property {
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
                Property::Crossfade => "×gain add rate",
                Property::Morph => "×gain add 1.1",
                Property::Entrance => "rate ×gain add 1.4",
                _ => "feeds the property",
            },
            GraphSource::Bass => match property {
                Property::Pulse => "×gain add to energy",
                Property::Kick => "envelope",
                Property::Crossfade => "feeds the fade rate",
                _ => "feeds the property",
            },
            GraphSource::Onset(_) => "trigger flash",
            GraphSource::Kick => match property {
                Property::Pulse => "×gain add to energy",
                Property::Crossfade => "×gain add to rate",
                _ => "feeds the property",
            },
            GraphSource::DropLow
            | GraphSource::DropHigh
            | GraphSource::DropDiff
            | GraphSource::DropConfidence
            | GraphSource::DropEligible
            | GraphSource::DetectorOnsetFlux(_)
            | GraphSource::DetectorSuppression(_) => "",
        },
        DebugSelection::Flux => "",
    }
}

fn graph_usage_text(
    selection: DebugSelection,
    source: GraphSource,
    visualizer: &Visualizer,
    focus: InspectorFocus,
) -> String {
    let usage = graph_usage_for_focus(focus, selection, source);
    if matches!(focus, InspectorFocus::Flux(_)) {
        return usage.to_owned();
    }
    let DebugSelection::Property(property) = selection else {
        return usage.to_owned();
    };
    let input = match source {
        GraphSource::Band(_)
        | GraphSource::BandOnset(_)
        | GraphSource::Onset(_)
        | GraphSource::DropLow
        | GraphSource::DropHigh
        | GraphSource::DropDiff
        | GraphSource::DropConfidence
        | GraphSource::DropEligible
        | GraphSource::DetectorOnsetFlux(_)
        | GraphSource::DetectorSuppression(_) => return usage.to_owned(),
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
    focus: InspectorFocus,
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
    let graph_max = match (focus, source) {
        (
            InspectorFocus::Selection(DebugSelection::Property(selected)),
            GraphSource::Property(property),
        ) if selected == property => graph_max,
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
    let usage = graph_usage_text(selection, source, visualizer, focus);
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
    let baseline_y = if source.inverted() {
        plot.y0
    } else {
        plot.center().y
    };
    scene.stroke(
        &Stroke::new(0.7),
        Affine::IDENTITY,
        palette.neutral[0].with_alpha(0.08),
        None,
        &Line::new(
            Point::new(plot.x0, baseline_y),
            Point::new(plot.x1, baseline_y),
        ),
    );
    let settings = visualizer.detector_settings();
    if let GraphSource::DropConfidence = source {
        draw_level_line(
            scene,
            plot,
            range,
            settings.drop_gap_threshold,
            color.with_alpha(0.45),
            scale,
        );
    }
    if let GraphSource::DetectorOnsetFlux(detector) = source {
        draw_trace(
            scene,
            history,
            plot,
            range,
            TraceStyle {
                color: palette.neutral[1].with_alpha(0.8),
                width: 0.9 * scale,
                start: 0.0,
            },
            |sample| onset_flux_sample(sample, detector).threshold,
        );
    }
    if source.has_raw() {
        draw_source_trace(
            scene,
            history,
            plot,
            range,
            TraceStyle {
                color: color.with_alpha(0.35),
                width: 0.8 * scale,
                start: 0.0,
            },
            source.inverted(),
            |sample| source.value(sample, true, routing).unwrap_or(0.0),
        );
    }
    draw_source_trace(
        scene,
        history,
        plot,
        range,
        TraceStyle {
            color,
            width: 1.2 * scale,
            start: 0.0,
        },
        source.inverted(),
        |sample| source.value(sample, false, routing).unwrap_or(0.0),
    );
    if let Some(shape) = source.shape(visualizer) {
        let flux_band =
            matches!(focus, InspectorFocus::Flux(_)) && matches!(source, GraphSource::Band(_));
        if shape.is_adaptive() || flux_band {
            if flux_band {
                draw_reference_lines(scene, plot, range, shape, palette, scale);
            }
            draw_reference_markers(scene, plot, range, shape, palette, scale);
        }
    }
    if let GraphSource::Onset(detector) = source {
        draw_onset_ticks(scene, history, plot, detector, palette, scale);
    }
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

struct TraceStyle {
    color: Color,
    width: f64,
    /// `start` overrides the left edge when a trace only applies to a suffix of
    /// the history; zero means use the oldest retained sample.
    start: f32,
}

fn draw_source_trace(
    scene: &mut Scene,
    history: &DebugHistory,
    plot: Rect,
    range: (f32, f32),
    style: TraceStyle,
    inverted: bool,
    value: impl Fn(&DebugSample) -> f32,
) {
    if inverted {
        draw_inverted_trace(scene, history, plot, range, style, value);
    } else {
        draw_trace(scene, history, plot, range, style, value);
    }
}

fn draw_trace(
    scene: &mut Scene,
    history: &DebugHistory,
    plot: Rect,
    range: (f32, f32),
    style: TraceStyle,
    value: impl Fn(&DebugSample) -> f32,
) {
    draw_trace_axis(scene, history, plot, range, style, false, value);
}

fn draw_inverted_trace(
    scene: &mut Scene,
    history: &DebugHistory,
    plot: Rect,
    range: (f32, f32),
    style: TraceStyle,
    value: impl Fn(&DebugSample) -> f32,
) {
    draw_trace_axis(scene, history, plot, range, style, true, value);
}

fn draw_trace_axis(
    scene: &mut Scene,
    history: &DebugHistory,
    plot: Rect,
    range: (f32, f32),
    style: TraceStyle,
    inverted: bool,
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
    for sample in history.samples() {
        if sample.at < start {
            continue;
        }
        let x = plot.x0 + f64::from((sample.at - start) / span) * plot.width();
        let y = map_level_axis(plot, range, value(sample), inverted);
        if count == 0 {
            path.move_to(Point::new(x, y));
        } else {
            path.line_to(Point::new(x, y));
        }
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
}

/// Marks an adaptive shape's low and high references along the plot's right
/// edge so the live trace can be read against them.
fn draw_reference_markers(
    scene: &mut Scene,
    plot: Rect,
    range: (f32, f32),
    shape: InputShape,
    palette: Palette,
    scale: f64,
) {
    let half_height = (plot.height() * 0.12).clamp(0.8 * scale, 2.5 * scale);
    let width = 1.4 * half_height;
    let color = palette.neutral[1].with_alpha(0.8);
    for reference in [shape.low, shape.high] {
        let y =
            map_level(plot, range, reference).clamp(plot.y0 + half_height, plot.y1 - half_height);
        let marker = Triangle::new(
            Point::new(plot.x1 - width, y),
            Point::new(plot.x1, y - half_height),
            Point::new(plot.x1, y + half_height),
        );
        scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &marker);
    }
}

/// Draws the live low and high references across the whole plot so the trace
/// can be compared with them over time.
fn draw_reference_lines(
    scene: &mut Scene,
    plot: Rect,
    range: (f32, f32),
    shape: InputShape,
    palette: Palette,
    scale: f64,
) {
    let color = palette.neutral[1].with_alpha(0.24);
    for reference in [shape.low, shape.high] {
        let y = map_level(plot, range, reference);
        scene.stroke(
            &Stroke::new(0.6 * scale),
            Affine::IDENTITY,
            color,
            None,
            &Line::new(Point::new(plot.x0, y), Point::new(plot.x1, y)),
        );
    }
}

/// Marks confirmed triggers along the top of a detector graph.
fn draw_onset_ticks(
    scene: &mut Scene,
    history: &DebugHistory,
    plot: Rect,
    detector: Detector,
    palette: Palette,
    scale: f64,
) {
    draw_trigger_marks(
        scene,
        history,
        plot,
        palette.vivid(0).with_alpha(0.55),
        scale,
        |sample| trigger_sample(sample, detector).flash,
    );
}

fn draw_trigger_marks(
    scene: &mut Scene,
    history: &DebugHistory,
    plot: Rect,
    color: Color,
    scale: f64,
    flash: impl Fn(&DebugSample) -> f32,
) {
    let Some((first, last)) = history.span() else {
        return;
    };
    let span = last - first;
    if span <= 0.0 {
        return;
    }
    for sample in history.samples() {
        if flash(sample) < 0.8 {
            continue;
        }
        let x = plot.x0 + f64::from((sample.at - first) / span) * plot.width();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            color,
            None,
            &Rect::from_origin_size(
                Point::new(x - 0.5 * scale, plot.y0),
                Size::new(scale.max(0.8), 2.0 * scale),
            ),
        );
    }
}

fn map_level(plot: Rect, range: (f32, f32), value: f32) -> f64 {
    map_level_axis(plot, range, value, false)
}

fn map_level_axis(plot: Rect, range: (f32, f32), value: f32, inverted: bool) -> f64 {
    let span = (range.1 - range.0).max(1e-6);
    let normalized = ((value - range.0) / span).clamp(0.0, 1.0);
    if inverted {
        plot.y0 + f64::from(normalized) * plot.height()
    } else {
        plot.y1 - f64::from(normalized) * plot.height()
    }
}

fn draw_level_line(
    scene: &mut Scene,
    plot: Rect,
    range: (f32, f32),
    value: f32,
    color: Color,
    scale: f64,
) {
    let y = map_level(plot, range, value);
    scene.stroke(
        &Stroke::new(0.8 * scale),
        Affine::IDENTITY,
        color,
        None,
        &Line::new(Point::new(plot.x0, y), Point::new(plot.x1, y)),
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_property_toggle(
    fonts: &mut Fonts,
    scene: &mut Scene,
    rect: Rect,
    label: &str,
    toggled: bool,
    toggled_color: Color,
    property_active: bool,
    palette: Palette,
    scale: f64,
) {
    let fill = if toggled {
        toggled_color.with_alpha(0.3)
    } else {
        palette.neutral[0].with_alpha(if property_active { 0.07 } else { 0.03 })
    };
    let stroke = if toggled {
        toggled_color.with_alpha(0.9)
    } else {
        palette.neutral[2].with_alpha(if property_active { 0.4 } else { 0.18 })
    };
    let text = if toggled {
        toggled_color
    } else if property_active {
        palette.neutral[1].with_alpha(0.6)
    } else {
        palette.neutral[1].with_alpha(0.28)
    };
    scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &rect);
    scene.stroke(
        &Stroke::new(0.7 * scale),
        Affine::IDENTITY,
        stroke,
        None,
        &rect,
    );
    let label = fit(
        fonts,
        label,
        (5.5 * scale) as f32,
        UiFace::SemiBold,
        rect.width() - scale,
    );
    draw_left_center(
        scene,
        &label,
        rect.center().x - label.width() * 0.5,
        rect.center().y,
        text,
    );
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
    let detectors = visualizer.detector_settings();
    if dependencies.contains(&Dependency::KickOnset) && detectors.kick_contains(band) {
        route.push('K');
    }
    if dependencies.contains(&Dependency::SnareOnset) && detectors.snare_contains(band) {
        route.push('S');
    }
    if dependencies.contains(&Dependency::DropOnset) {
        route.push('D');
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
        Dependency::KickOnset => "K-ONSET".to_owned(),
        Dependency::SnareOnset => "S-ONSET".to_owned(),
        Dependency::DropOnset => "D-ONSET".to_owned(),
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
        let (_, inspector) = inspector_panes(&view, VisualizerTextScale::default());
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
            Some(TreeTarget::Detector(Detector::Drop)),
            &Mixer::default(),
        );
        assert_eq!(tree.sliders.len(), 14);
        assert!(tree
            .sliders
            .iter()
            .any(|slider| slider.parameter == Parameter::DropGapWindow));
        assert!(tree
            .sliders
            .iter()
            .any(|slider| slider.parameter == Parameter::DropLowLp));
        assert_eq!(tree.kernels.len(), 3);
        let bank = tree.kernels[0];
        let band = 8;
        let point = Point::new(
            bank.rect.x0 + (band as f64 + 0.5) * bank.rect.width() / BAND_COUNT as f64,
            bank.rect.center().y,
        );
        assert_eq!(
            tree.kernel_at(point)
                .map(|(kernel, _, band)| (kernel, band)),
            Some((DropKernel::Low, band))
        );
        assert_eq!(
            tree.sliders
                .iter()
                .map(|slider| slider.parameter)
                .collect::<Vec<_>>(),
            Detector::Drop.parameters()
        );
    }

    #[test]
    fn detector_trigger_focus_switches_to_detector_graphs() {
        let selection = DebugSelection::Property(Property::Crossfade);
        assert_eq!(
            inspector_focus(selection, None, 0),
            InspectorFocus::Selection(selection)
        );
        for detector in [Detector::Kick, Detector::Snare, Detector::Drop] {
            assert_eq!(
                inspector_focus(selection, Some(TreeTarget::Detector(detector)), 0),
                InspectorFocus::Detector(detector)
            );
        }
        assert_eq!(
            inspector_focus(selection, Some(TreeTarget::Input(Input::DropOnset)), 0),
            InspectorFocus::Detector(Detector::Drop)
        );
        assert_eq!(
            inspector_focus(DebugSelection::Flux, None, 7),
            InspectorFocus::Flux(7)
        );
        assert_eq!(
            detector_graph_sources(Detector::Flux, 7),
            vec![GraphSource::BandOnset(7), GraphSource::Band(7)]
        );
        assert!(!graph_usage_for_focus(
            InspectorFocus::Flux(7),
            selection,
            GraphSource::BandOnset(7)
        )
        .is_empty());
        assert!(
            !graph_usage_for_focus(InspectorFocus::Flux(7), selection, GraphSource::Band(7))
                .is_empty()
        );
        let graphs = detector_graph_sources(Detector::Drop, 0);
        for expected in [
            GraphSource::DropLow,
            GraphSource::DropHigh,
            GraphSource::DropDiff,
            GraphSource::DropConfidence,
            GraphSource::DropEligible,
            GraphSource::DetectorOnsetFlux(Detector::Drop),
            GraphSource::DetectorSuppression(Detector::Drop),
            GraphSource::Onset(Detector::Drop),
        ] {
            assert!(graphs.contains(&expected), "missing {expected:?}");
        }
        for source in graphs {
            assert!(
                !graph_usage(selection, source).is_empty(),
                "{source:?} needs a caption"
            );
        }
        for detector in [Detector::Kick, Detector::Snare] {
            let graphs = detector_graph_sources(detector, 0);
            assert!(graphs.contains(&GraphSource::DetectorOnsetFlux(detector)));
            assert!(graphs.contains(&GraphSource::DetectorSuppression(detector)));
            assert!(graphs.contains(&GraphSource::Onset(detector)));
            for source in graphs {
                assert!(!graph_usage(selection, source).is_empty());
            }
        }
        assert_eq!(GraphSource::DropDiff.range(None), (-1.0, 1.0));
        assert_eq!(GraphSource::DropHigh.range(None), (0.0, 1.0));
        assert_eq!(
            GraphSource::DetectorOnsetFlux(Detector::Drop).range(None),
            (0.0, 0.25)
        );
        assert_eq!(
            GraphSource::DetectorSuppression(Detector::Drop).range(None),
            (0.0, 0.25)
        );
    }

    #[test]
    fn drop_focus_fits_its_graph_cells() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let visualizer = Visualizer::new();
        let (_, extension) = inspector_panes(&view, text_scale);
        let scale = view.ui_scale() * f64::from(text_scale.factor());
        let (cells, rows) = extension_graph_cells(
            extension,
            scale,
            InspectorFocus::Detector(Detector::Drop),
            &visualizer,
        );
        assert_eq!(rows, 2);
        assert_eq!(cells.len(), detector_graph_sources(Detector::Drop, 0).len());
        for (_, cell) in cells {
            assert!(cell.width() >= 12.0 * scale);
            assert!(cell.height() >= 12.0 * scale);
        }
    }

    #[test]
    fn kick_and_snare_focus_fits_their_flux_graphs() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let visualizer = Visualizer::new();
        let (_, extension) = inspector_panes(&view, text_scale);
        let scale = view.ui_scale() * f64::from(text_scale.factor());
        for detector in [Detector::Kick, Detector::Snare] {
            let (cells, rows) = extension_graph_cells(
                extension,
                scale,
                InspectorFocus::Detector(detector),
                &visualizer,
            );
            assert_eq!(rows, 2);
            assert_eq!(cells.len(), detector_graph_sources(detector, 0).len());
            for source in [
                GraphSource::DetectorOnsetFlux(detector),
                GraphSource::DetectorSuppression(detector),
            ] {
                assert!(cells.iter().any(|(candidate, _)| *candidate == source));
            }
        }
    }

    #[test]
    fn flux_focus_shows_the_flux_then_the_raw_band_envelope() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let visualizer = Visualizer::new();
        let (_, extension) = inspector_panes(&view, text_scale);
        let scale = view.ui_scale() * f64::from(text_scale.factor());
        let (cells, rows) =
            extension_graph_cells(extension, scale, InspectorFocus::Flux(5), &visualizer);
        assert_eq!(rows, 2);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].0, GraphSource::BandOnset(5));
        assert_eq!(cells[1].0, GraphSource::Band(5));
        let flux = cells[0].1;
        let band = cells[1].1;
        assert!(band.y0 > flux.y1);
        assert_eq!(band.x0, flux.x0);
        assert_eq!(band.x1, flux.x1);
        assert_eq!(band.height(), flux.height());
        assert!(GraphSource::Band(5)
            .shape(&visualizer)
            .is_some_and(|shape| shape.is_adaptive()));
        let snapshot = visualizer.debug_snapshot();
        let (heading, description, dependencies) = flux_extension_text(5, &snapshot, &visualizer);
        assert!(heading.contains("B05"));
        assert!(description.contains("EXCESS"));
        assert!(description.contains("ONSET"));
        assert!(dependencies.contains("ABL"));
    }

    #[test]
    fn detector_focus_stacks_suppression_below_the_onset_graph() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        let visualizer = Visualizer::new();
        let (_, extension) = inspector_panes(&view, text_scale);
        let scale = view.ui_scale() * f64::from(text_scale.factor());
        for detector in [Detector::Kick, Detector::Snare, Detector::Drop] {
            let (cells, _) = extension_graph_cells(
                extension,
                scale,
                InspectorFocus::Detector(detector),
                &visualizer,
            );
            let find = |source| {
                cells
                    .iter()
                    .find_map(|(candidate, rect)| (*candidate == source).then_some(*rect))
                    .expect("detector graph cell")
            };
            let onset = find(GraphSource::DetectorOnsetFlux(detector));
            let suppression = find(GraphSource::DetectorSuppression(detector));
            assert_eq!(suppression.x0, onset.x0);
            assert_eq!(suppression.x1, onset.x1);
            assert!(suppression.y0 > onset.y1);
            assert!((suppression.height() - onset.height()).abs() < 1e-6);
            assert!(GraphSource::DetectorSuppression(detector).inverted());
            assert!(!GraphSource::DetectorOnsetFlux(detector).inverted());
        }
    }

    #[test]
    fn inverted_graph_axis_grows_downward() {
        let plot = Rect::new(0.0, 0.0, 10.0, 10.0);
        let range = (0.0, 1.0);
        assert_eq!(map_level_axis(plot, range, 0.0, true), plot.y0);
        assert_eq!(map_level_axis(plot, range, 1.0, true), plot.y1);
        assert_eq!(map_level_axis(plot, range, 0.0, false), plot.y1);
        assert_eq!(map_level_axis(plot, range, 1.0, false), plot.y0);
    }

    #[test]
    fn kick_and_snare_focus_headers_report_their_onset_flux() {
        let visualizer = Visualizer::new();
        let snapshot = visualizer.debug_snapshot();
        for detector in [Detector::Kick, Detector::Snare] {
            let (heading, description, _) =
                detector_extension_text(detector, &snapshot, &visualizer);
            assert!(heading.ends_with("IDLE"));
            assert!(heading.starts_with(detector.name()));
            assert!(description.contains("ONSET FLUX"));
        }
    }

    #[test]
    fn drop_focus_header_reports_the_gap_state() {
        let visualizer = Visualizer::new();
        let snapshot = visualizer.debug_snapshot();
        let (heading, description, dependencies) = drop_extension_text(&snapshot, &visualizer);
        assert_eq!(heading, "DROP TRIGGER  IDLE");
        assert!(description.contains("LF 0.00"));
        assert!(description.contains("CONF 0.00/0.15"));
        assert!(dependencies.contains("WINDOW 1.00s"));
        assert!(dependencies.contains("ONSET"));
    }

    #[test]
    fn kick_and_snare_expose_temporal_pattern_editors() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let text_scale = VisualizerTextScale::default();
        for detector in [Detector::Kick, Detector::Snare] {
            let tree = tree_layout(
                &view,
                text_scale,
                Property::Crossfade,
                Some(TreeTarget::Detector(detector)),
                &Mixer::default(),
            );
            assert!(
                tree.kernels.is_empty(),
                "kick and snare use temporal templates"
            );
            let temporal = tree.temporal.expect("the temporal pattern editor");
            assert_eq!(temporal.detector, detector);
            assert!(temporal.canvas.height() > 0.0);
            assert!(temporal.offset.height() > 0.0);
            assert_eq!(
                tree.action_center(TreeAction::ClearPattern(detector)),
                Some(temporal.clear.rect.center())
            );
            assert_eq!(
                tree.action_center(TreeAction::TogglePatternOverlay),
                Some(temporal.overlay.rect.center())
            );
        }
    }

    #[test]
    fn flux_analyzer_exposes_band_controls() {
        let view = View::fit(Size::new(1280.0, 440.0));
        let layout = flux_analyzer_layout(&view, VisualizerTextScale::default(), 5);
        assert_eq!(layout.parameter_title, "ONSET FLOW B05");
        let target = EnvelopeTarget::BandIndex(5);
        let expected = vec![
            Parameter::InputLow(target),
            Parameter::InputHigh(target),
            Parameter::BandAblGain(5),
            Parameter::BandAblCurve(5),
            Parameter::BandAblAmount(5),
            Parameter::ExcessAttack,
            Parameter::ExcessRelease,
            Parameter::ExcessFloor,
            Parameter::ExcessCap,
        ];
        assert_eq!(
            layout
                .sliders
                .iter()
                .map(|slider| slider.parameter)
                .collect::<Vec<_>>(),
            expected
        );
        assert!(!layout.sliders.iter().any(|slider| matches!(
            slider.parameter,
            Parameter::InputCurve(_) | Parameter::InputAmount(_)
        )));
        assert_eq!(layout.actions.len(), 1);
        assert_eq!(layout.actions[0].action, TreeAction::ToggleAdaptive(target));
        assert!(layout.rows.is_empty());
        assert!(layout.kernels.is_empty());
        let bank = layout.flux_bank.expect("the flux analyzer bank");
        assert_eq!(bank.selected, 5);
        let center = bank.rect.center();
        assert_eq!(layout.flux_band_at(center), Some(BAND_COUNT / 2));

        let graphs = layout.onset_graphs.expect("the onset internals graphs");
        assert!(graphs.rect.y0 >= bank.rect.y1);
        let last_slider = layout.sliders.last().expect("the last slider");
        assert!(graphs.rect.y0 >= last_slider.track.y1);
        for lane in graphs.lanes {
            assert!(lane.y0 >= graphs.rect.y0);
            assert!(lane.y1 <= graphs.rect.y1);
            assert!(lane.height() > 0.0);
        }
    }

    #[test]
    fn onset_switches_get_processor_graphs() {
        let visualizer = Visualizer::new();
        let crossfade = graph_sources(DebugSelection::Property(Property::Crossfade), &visualizer);
        assert!(crossfade.contains(&GraphSource::Onset(Detector::Drop)));
        assert!(!crossfade.contains(&GraphSource::Onset(Detector::Kick)));

        let kick = graph_sources(DebugSelection::Property(Property::Kick), &visualizer);
        assert!(kick.contains(&GraphSource::Onset(Detector::Kick)));
        assert!(!kick.contains(&GraphSource::Onset(Detector::Drop)));

        let spin = graph_sources(DebugSelection::Property(Property::Spin), &visualizer);
        assert!(!spin
            .iter()
            .any(|source| matches!(source, GraphSource::Onset(_))));

        let low_band = graph_sources(DebugSelection::Band(0), &visualizer);
        assert!(low_band.contains(&GraphSource::Onset(Detector::Kick)));
        let snare_band = graph_sources(DebugSelection::Band(5), &visualizer);
        assert!(snare_band.contains(&GraphSource::Onset(Detector::Snare)));
        let high_band = graph_sources(DebugSelection::Band(8), &visualizer);
        assert!(!high_band.contains(&GraphSource::Onset(Detector::Kick)));
    }

    #[test]
    fn band_graphs_include_spectral_flux() {
        let visualizer = Visualizer::new();
        let graphs = graph_sources(DebugSelection::Band(5), &visualizer);
        assert!(graphs.contains(&GraphSource::Band(5)));
        assert!(graphs.contains(&GraphSource::BandOnset(5)));
        assert!(graphs.contains(&GraphSource::Power));
        assert!(!graph_usage(DebugSelection::Band(5), GraphSource::BandOnset(5)).is_empty());
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
        let (tree, inspector) = inspector_panes(&view, text_scale);
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
        let detectors = visualizer.detector_settings();
        assert_eq!(band_route(Property::Pulse, 0, &visualizer), "V");
        assert_eq!(band_route(Property::Kick, 0, &visualizer), "K");
        assert_eq!(band_route(Property::Crossfade, 0, &visualizer), "D");
        assert_eq!(band_route(Property::Crossfade, 4, &visualizer), "D");
        assert_eq!(band_route(Property::Flip, 5, &visualizer), "S");
        assert_eq!(band_route(Property::Spin, 4, &visualizer), "V");
        assert_eq!(band_route(Property::Spin, 5, &visualizer), "");
        assert!(Property::Kick.depends_on_band(3, routing, &detectors));
        assert!(!Property::Kick.depends_on_band(5, routing, &detectors));
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
            "×gain add rate"
        );
    }
}
