use std::sync::OnceLock;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use vello::peniko::Color;

use crate::frame_schedule::Adjustment;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeId(usize);

impl Default for ThemeId {
    fn default() -> Self {
        Self(
            registry()
                .iter()
                .position(|entry| entry.name == "Twilight")
                .expect("Twilight is bundled"),
        )
    }
}

impl ThemeId {
    pub fn name(self) -> &'static str {
        &registry()[self.0].name
    }

    #[must_use]
    pub fn adjusted(self, adjustment: Adjustment) -> Self {
        let count = registry().len();
        Self(match adjustment {
            Adjustment::Decrease => (self.0 + count - 1) % count,
            Adjustment::Increase => (self.0 + 1) % count,
        })
    }
}

impl Serialize for ThemeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for ThemeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        registry()
            .iter()
            .position(|entry| entry.name == name)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown theme {name:?}")))
    }
}

/// A named slot in the selected theme's original terminal palette.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum::VariantArray, strum::Display,
)]
#[strum(serialize_all = "title_case")]
pub enum Accent {
    Foreground,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}
impl ThemeId {
    /// The registry groups a light theme by its background being lighter than
    /// its foreground.
    pub fn is_light(self) -> bool {
        let entry = &registry()[self.0];
        luma(entry.colors.background) >= luma(entry.foreground)
    }

    pub fn color(self, accent: Accent) -> Color {
        let entry = &registry()[self.0];
        let index = match accent {
            Accent::Foreground => return entry.foreground,
            Accent::Black => 0,
            Accent::Red => 1,
            Accent::Green => 2,
            Accent::Yellow => 3,
            Accent::Blue => 4,
            Accent::Magenta => 5,
            Accent::Cyan => 6,
            Accent::White => 7,
            Accent::BrightBlack => 8,
            Accent::BrightRed => 9,
            Accent::BrightGreen => 10,
            Accent::BrightYellow => 11,
            Accent::BrightBlue => 12,
            Accent::BrightMagenta => 13,
            Accent::BrightCyan => 14,
            Accent::BrightWhite => 15,
        };
        entry.accents[index]
    }
}

/// The single mapping from Gogh's terminal palette to VU's UI roles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeColors {
    pub background: Color,
    pub readout: Color,
    pub hint: Color,
}

impl ThemeColors {
    fn from_gogh(theme: &GoghTheme) -> Self {
        let background = hex(&theme.background);
        let foreground = hex(&theme.foreground);
        let scale = mix(foreground, background, 0.3);
        Self {
            background,
            readout: mix(background, scale, 0.4),
            hint: mix(background, scale, 0.6),
        }
    }

    fn crossfade(self, target: Self, progress: f32) -> Self {
        Self {
            background: mix(self.background, target.background, progress),
            readout: mix(self.readout, target.readout, progress),
            hint: mix(self.hint, target.hint, progress),
        }
    }
}

pub fn mix(from: Color, to: Color, progress: f32) -> Color {
    Color::new(std::array::from_fn(|i| {
        from.components[i] + (to.components[i] - from.components[i]) * progress
    }))
}

fn luma(color: Color) -> f32 {
    let [r, g, b, _] = color.components;
    0.299 * r + 0.587 * g + 0.114 * b
}

fn hex(value: &str) -> Color {
    assert_eq!(value.len(), 7, "bundled colors are #RRGGBB");
    let rgb = u32::from_str_radix(value.strip_prefix('#').expect("hex prefix"), 16)
        .expect("valid bundled color");
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// One entry of the bundled Gogh `themes.json` catalog.
#[derive(Deserialize)]
struct GoghTheme {
    name: String,
    background: String,
    foreground: String,
    color_01: String,
    color_02: String,
    color_03: String,
    color_04: String,
    color_05: String,
    color_06: String,
    color_07: String,
    color_08: String,
    color_09: String,
    color_10: String,
    color_11: String,
    color_12: String,
    color_13: String,
    color_14: String,
    color_15: String,
    color_16: String,
}

impl GoghTheme {
    /// The sixteen standard terminal slots in ANSI order.
    fn palette(&self) -> [&str; 16] {
        [
            &self.color_01,
            &self.color_02,
            &self.color_03,
            &self.color_04,
            &self.color_05,
            &self.color_06,
            &self.color_07,
            &self.color_08,
            &self.color_09,
            &self.color_10,
            &self.color_11,
            &self.color_12,
            &self.color_13,
            &self.color_14,
            &self.color_15,
            &self.color_16,
        ]
    }
}

struct Entry {
    name: String,
    foreground: Color,
    accents: [Color; 16],
    colors: ThemeColors,
}

fn registry() -> &'static [Entry] {
    static REGISTRY: OnceLock<Vec<Entry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let themes: Vec<GoghTheme> = serde_json::from_str(include_str!("../assets/themes.json"))
            .expect("valid bundled palettes");
        let mut entries: Vec<_> = themes
            .into_iter()
            .map(|theme| Entry {
                colors: ThemeColors::from_gogh(&theme),
                foreground: hex(&theme.foreground),
                accents: theme.palette().map(hex),
                name: theme.name,
            })
            .collect();
        entries.sort_by(|a, b| {
            let light = |entry: &Entry| luma(entry.colors.background) >= luma(entry.foreground);
            light(a).cmp(&light(b)).then_with(|| a.name.cmp(&b.name))
        });
        entries
    })
}

struct Transition {
    from: ThemeColors,
    elapsed: f32,
}

pub struct Theme {
    selected: ThemeId,
    pub colors: ThemeColors,
    transition: Option<Transition>,
}

impl Theme {
    pub fn selected(&self) -> ThemeId {
        self.selected
    }

    pub fn new(selected: ThemeId) -> Self {
        Self {
            selected,
            colors: registry()[selected.0].colors,
            transition: None,
        }
    }

    pub fn select(&mut self, selected: ThemeId) {
        self.selected = selected;
        self.transition = Some(Transition {
            from: self.colors,
            elapsed: 0.0,
        });
    }

    pub fn advance(&mut self, dt: f32) {
        let Some(transition) = &mut self.transition else {
            return;
        };
        transition.elapsed += dt;
        let progress = (transition.elapsed / 0.2).min(1.0);
        self.colors = transition.from.crossfade(
            registry()[self.selected.0].colors,
            if progress < 0.5 {
                2.0 * progress * progress
            } else {
                1.0 - (-2.0 * progress + 2.0).powi(2) / 2.0
            },
        );
        if progress == 1.0 {
            self.colors = registry()[self.selected.0].colors;
            self.transition = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_theme_classifies_as_light_or_dark() {
        let light = (0..registry().len())
            .filter(|&index| ThemeId(index).is_light())
            .count();
        assert!(light > 0);
        assert!(light < registry().len());
    }

    #[test]
    fn registry_cycles_and_serializes_every_shipped_theme() {
        assert_eq!(registry().len(), 361);
        let first = ThemeId(0);
        assert_eq!(
            first
                .adjusted(Adjustment::Decrease)
                .adjusted(Adjustment::Increase),
            first
        );
        for index in 0..registry().len() {
            let id = ThemeId(index);
            let text = ron::to_string(&id).unwrap();
            assert_eq!(ron::from_str::<ThemeId>(&text).unwrap(), id);
        }
        assert!(ron::from_str::<ThemeId>("\"Unknown theme\"").is_err());
    }

    #[test]
    fn bundled_theme_names_are_unique() {
        let mut names: Vec<_> = registry().iter().map(|entry| entry.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            registry().len(),
            "names address the saved theme"
        );
    }

    #[test]
    fn interrupted_crossfade_starts_from_visible_colors_and_settles() {
        let mut theme = Theme::new(ThemeId::default());
        let initial = theme.colors;
        let next = theme.selected.adjusted(Adjustment::Increase);
        theme.select(next);
        theme.advance(0.1);
        let halfway = initial.crossfade(registry()[next.0].colors, 0.5);
        assert_eq!(theme.colors, halfway);
        theme.select(next.adjusted(Adjustment::Increase));
        theme.advance(0.0);
        assert_eq!(theme.colors, halfway);
        theme.advance(0.2);
        assert_eq!(theme.colors, registry()[theme.selected.0].colors);
        assert!(theme.transition.is_none());
        let settled = theme.colors;
        theme.advance(1.0);
        assert_eq!(theme.colors, settled);
    }
}
