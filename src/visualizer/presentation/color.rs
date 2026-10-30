//! A deliberately small presentation palette: ink, one accent, and one muted
//! tone. Backdrops and rules are alpha variants of the same three colors.

use vello::peniko::Color;

use crate::{
    theme::{mix, ThemeColors},
    visualizer::{Palette, Rng},
};

#[derive(Clone, Copy, Debug)]
pub struct ScenePalette {
    pub ink: Color,
    pub accent: Color,
    pub muted: Color,
    pub(crate) accent_index: usize,
}

impl ScenePalette {
    pub fn placeholder() -> Self {
        Self {
            ink: Color::WHITE,
            accent: Color::WHITE,
            muted: Color::WHITE,
            accent_index: 0,
        }
    }

    pub fn choose(palette: &Palette, colors: ThemeColors, rng: &mut Rng) -> Self {
        let ink = palette.neutral[0];
        let background = colors.background;
        let (mut accent_index, mut accent) = best_accent(palette, background);
        // Perturb the best-contrast choice with the scene seed so consecutive
        // scenes differ, but never accept a washed-out accent.
        if rng.unit() >= 0.5 {
            let index = (rng.range(Palette::VIVID_COUNT) + 3) % Palette::VIVID_COUNT;
            let candidate = palette.vivid(index);
            if contrast_ratio(candidate, background) >= 2.5 {
                accent_index = index;
                accent = candidate;
            }
        }
        Self {
            ink,
            accent,
            muted: mix(ink, background, 0.45),
            accent_index,
        }
    }

    /// Re-samples the same three slots from the live palette so a theme
    /// crossfade reaches the presentation. If the theme changes to a light or
    /// dark background that washes out the chosen accent, the best-contrast
    /// vivid slot replaces it.
    pub fn refresh(&mut self, palette: &Palette, colors: ThemeColors) {
        self.ink = palette.neutral[0];
        let background = colors.background;
        let current = palette.vivid(self.accent_index);
        if contrast_ratio(current, background) < 2.5 {
            (self.accent_index, self.accent) = best_accent(palette, background);
        } else {
            self.accent = current;
        }
        self.muted = mix(self.ink, background, 0.45);
    }
}

fn best_accent(palette: &Palette, background: Color) -> (usize, Color) {
    (0..Palette::VIVID_COUNT)
        .map(|index| (index, palette.vivid(index)))
        .max_by(|(_, a), (_, b)| {
            contrast_ratio(*a, background)
                .total_cmp(&contrast_ratio(*b, background))
                .then_with(|| a.components[0].total_cmp(&b.components[0]))
        })
        .expect("the palette has vivid colors")
}

fn linear(component: f32) -> f32 {
    if component <= 0.04045 {
        component / 12.92
    } else {
        ((component + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance(color: Color) -> f32 {
    let [r, g, b, _] = color.components;
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast_ratio(a: Color, b: Color) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
