//! Loads the authored weapon catalog with its real-world specs, points of
//! interest and comedy beats.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use anyhow::Context;
use serde::Deserialize;

const CATALOG_PATH: &str = "assets/models-spec/weapon_visualizer_meme_spec.json";

/// The weapon catalog. Unknown fields are rejected so the structs cannot
/// silently fall behind the authored data.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: String,
    pub description: String,
    pub provenance: Provenance,
    pub sources: Vec<Source>,
    pub weapons: BTreeMap<String, Weapon>,
    pub comedy_spec: ComedySpec,
}

impl Catalog {
    pub fn load() -> anyhow::Result<Self> {
        let text = fs::read_to_string(CATALOG_PATH)
            .with_context(|| format!("could not read the weapon catalog at {CATALOG_PATH}"))?;
        serde_json::from_str(&text)
            .with_context(|| format!("invalid weapon catalog at {CATALOG_PATH}"))
    }
}

/// Where the physical and comedic material came from.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub physical: String,
    pub jokes: String,
    pub model: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub url: String,
    pub applies_to: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComedySpec {
    pub styles: BTreeMap<Style, StyleGuide>,
    pub writing_rules: Vec<String>,
    pub selection: Selection,
    pub coordinate_policy: String,
    pub cue_policy: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleGuide {
    pub keywords: Vec<String>,
    pub logic: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub unit: String,
    pub styles_are_set: bool,
    pub allow_single_and_mixed: bool,
    pub weights_override_allowed: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weapon {
    pub id: String,
    pub display_name: String,
    pub category: Category,
    pub tagline: Option<String>,
    pub asset_variant: Option<String>,
    pub specs: Specs,
    pub poi: Vec<Poi>,
    pub interesting_facts: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Equipment,
    Grenade,
    MachineGun,
    Objective,
    Pistol,
    Rifle,
    Shotgun,
    Smg,
    Sniper,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Specs {
    pub real_world: RealWorld,
}

/// Physical values for the representative real-world family. A `None`
/// measurement means the value is unknown, not zero.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealWorld {
    pub cartridge: Option<String>,
    pub action: Option<String>,
    pub fire_mode: Option<String>,
    pub capacity_rounds: Option<u32>,
    pub mass_kg: Option<f64>,
    pub overall_length_mm: Option<f64>,
    pub barrel_length_mm: Option<f64>,
    pub price_usd: Option<f64>,
    /// Why the price applies, or why it is absent.
    pub price_basis: String,
    /// How the representative family differs from the supplied asset.
    pub configuration_note: String,
    /// Why a null measurement is unavailable rather than zero.
    pub measurement_caveat: String,
}

/// A semantic point of interest on the asset.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Poi {
    pub id: String,
    pub label: String,
    pub anchor: Anchor,
    pub fact: String,
    pub jokes: Vec<Joke>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// The namespace the name resolves in, such as a semantic mesh part.
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    /// Local model-space position, once the asset provides one.
    pub model_coordinates: Option<[f32; 3]>,
}

/// One authored caption beat. A joke with several styles is a mixed-style
/// beat, not several sequential jokes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Joke {
    pub id: String,
    pub styles: BTreeSet<Style>,
    pub text: String,
    pub intent: Intent,
    pub presentation: Option<Presentation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    Mlg,
    Surreal,
    TrollFact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    MontageOverreaction,
    AbsurdMemeLogic,
    CounterfeitFact,
}

/// Optional suggestions for camera, overlay, audio and timing.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presentation {
    pub camera: String,
    pub overlay: String,
    pub audio: String,
    pub duration_ms: u32,
    pub optional: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_deserializes() -> anyhow::Result<()> {
        let catalog = Catalog::load()?;
        assert!(!catalog.schema_version.is_empty());
        assert_eq!(catalog.weapons.len(), 31);
        assert!(!catalog.comedy_spec.styles.is_empty());

        let weapon = &catalog.weapons["smg_p90"];
        assert_eq!(weapon.category, Category::Smg);
        assert_eq!(weapon.poi.len(), 10);
        assert!(weapon.poi.iter().all(|poi| !poi.jokes.is_empty()));
        Ok(())
    }
}
