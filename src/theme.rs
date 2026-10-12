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
                .position(|entry| entry.name == "Srcery")
                .expect("Srcery is bundled"),
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

/// The single mapping from Gogh's terminal palette to VU's visual roles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterColors {
    pub background: Color,
    pub scale: Color,
    pub warning: Color,
    pub warning_dim: Color,
    pub needle: Color,
    pub needle_stop: Color,
    pub readout: Color,
    pub hint: Color,
    pub lamp_off: Color,
    pub lamp_hot: Color,
}

impl MeterColors {
    fn from_gogh(theme: &GoghTheme) -> Self {
        let background = hex(&theme.background);
        let scale = mix(hex(&theme.foreground), background, 0.3);
        Self {
            background,
            scale,
            warning: hex(&theme.basic[1]),
            warning_dim: mix(hex(&theme.basic[1]), background, 0.35),
            needle: hex(&theme.foreground),
            needle_stop: mix(background, scale, 0.5),
            readout: mix(background, scale, 0.4),
            hint: mix(background, scale, 0.6),
            lamp_off: mix(background, scale, 0.1),
            lamp_hot: hex(&theme.basic[11]),
        }
    }

    fn crossfade(self, target: Self, progress: f32) -> Self {
        Self {
            background: mix(self.background, target.background, progress),
            scale: mix(self.scale, target.scale, progress),
            warning: mix(self.warning, target.warning, progress),
            warning_dim: mix(self.warning_dim, target.warning_dim, progress),
            needle: mix(self.needle, target.needle, progress),
            needle_stop: mix(self.needle_stop, target.needle_stop, progress),
            readout: mix(self.readout, target.readout, progress),
            hint: mix(self.hint, target.hint, progress),
            lamp_off: mix(self.lamp_off, target.lamp_off, progress),
            lamp_hot: mix(self.lamp_hot, target.lamp_hot, progress),
        }
    }
}

pub fn mix(from: Color, to: Color, progress: f32) -> Color {
    Color::new(std::array::from_fn(|i| {
        from.components[i] + (to.components[i] - from.components[i]) * progress
    }))
}

fn hex(value: &str) -> Color {
    assert_eq!(value.len(), 7, "bundled colors are #RRGGBB");
    let rgb = u32::from_str_radix(value.strip_prefix('#').expect("hex prefix"), 16)
        .expect("valid bundled color");
    Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

#[derive(Deserialize)]
struct GoghTheme {
    name: String,
    background: String,
    foreground: String,
    basic: [String; 16],
}

struct Entry {
    name: String,
    colors: MeterColors,
}

fn registry() -> &'static [Entry] {
    static REGISTRY: OnceLock<Vec<Entry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let themes: Vec<GoghTheme> =
            ron::from_str(include_str!("../assets/themes.ron")).expect("valid bundled palettes");
        let mut entries: Vec<_> = themes
            .into_iter()
            .map(|theme| Entry {
                colors: MeterColors::from_gogh(&theme),
                name: theme.name,
            })
            .collect();
        let luma = |color: Color| {
            let [r, g, b, _] = color.components;
            0.299 * r + 0.587 * g + 0.114 * b
        };
        entries.sort_by(|a, b| {
            let light = |entry: &Entry| luma(entry.colors.background) >= luma(entry.colors.scale);
            light(a).cmp(&light(b)).then_with(|| a.name.cmp(&b.name))
        });
        entries
    })
}

struct Transition {
    from: MeterColors,
    elapsed: f32,
}

pub struct Theme {
    selected: ThemeId,
    pub colors: MeterColors,
    transition: Option<Transition>,
}

impl Theme {
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
    fn registry_cycles_and_serializes_every_shipped_theme() {
        assert_eq!(registry().len(), 41);
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
