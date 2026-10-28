# Weapon Visualizer — Content & Runtime Spec (v3.2)

## Purpose

Drive a 3D inspection app with real-world-inspired numeric weapon attributes, semantic points of interest (POIs), and deliberately ridiculous micro-scenes drawn from three treatments: **MLG montage parodies**, **Surreal Memes**, and **Troll Facts** (a short deadpan alternative to Troll Physics). Each joke may use one style or combine styles. The joke can be in the edit; a caption need not itself contain a punchline.

The companion data file is `weapon_visualizer_meme_spec.json`. It contains all **32 original IDs**, **10 POIs per asset**, and **320 authored caption beats**, plus presentation cues. The third style uses deliberately stupid statements, not faux-scientific instructions. These are jokes, not real engineering claims.

## Creative rulebook

| Style ID | Vocabulary and artifacts | Comedic mechanism |
|---|---|---|
| `mlg` | `360 NO SCOPE`, `MOM GET THE CAMERA`, hitmarker, airhorn, bass drop, unnecessary punch zoom | Treat an unremarkable part inspection as a world-record esports moment. |
| `surreal` | `STONKS`, `HELTH`, `ROTAT`, `E`, Meme Man, bad 3D, spaced-out captions | Objects acquire impossible properties without the scene acknowledging anything strange. |
| `troll_fact` | `FACT`, `OBVIOUSLY`, misleading captions, counterfeit technical labels | A short, stupid, almost-obvious or impossible statement, delivered as if it were an established fact. No steps. |

Do not put the vocabularies through a common slogan template. Vary cadence, typography, silence, intentional repetition, sound, and camera movement. Avoid clever-advertising copy, generic swagger, and jokes at the spectator's expense. Any ostensibly technical nonsense must remain explicitly inside a joke record, never in a factual spec field.

## Data model

- `schema_version`: string (`"3.2.0"`).
- `comedy_spec`: style lexicon, creative rules, selection behavior, coordinate and cue policy.
- `sources`: source URLs and coverage descriptions for representative physical information.
- `weapons`: object keyed by the exact asset ID, e.g. `smg_p90`.
  - `display_name`, `category`, `asset_variant`.
  - `specs.real_world`: numeric fields with units, e.g. `mass_kg`, `overall_length_mm`, `barrel_length_mm`, `capacity_rounds`, `price_usd`; plus `cartridge`, `action`, and caveats. `null` is an unknown, not zero.
  - `interesting_facts`: contextual factual descriptions.
  - `poi[]`: exactly ten semantic anchors for the current data set.
    - `id`, `label`, `fact`, `anchor.type`, `anchor.name`, `anchor.model_coordinates`.
    - `jokes[]`: one authored entry per POI in v3; designed to allow more later.
      - `id`: stable caption ID.
      - `styles`: **array** of style identifiers (one or more).
      - `text`: caption, may include `\n` for staged line reveals.
      - `intent`: `montage_overreaction`, `absurd_meme_logic`, or `counterfeit_fact`.
      - `presentation`: optional `camera`, `overlay`, `audio`, `duration_ms`, `optional`.

### Example

```json
{
  "id": "smg_p90_j09",
  "styles": ["troll_fact"],
  "text": "The magazine has a ninety-degree personality.",
  "presentation": {
    "camera": "deadpan_hold",
    "overlay": "counterfeit_fact_label",
    "audio": "dry_click_or_silence",
    "duration_ms": 1800,
    "optional": true
  },
  "intent": "counterfeit_fact"
}
```

This is a deliberately fictional caption and must never be surfaced as an engineering fact.

## Runtime choreography

1. Select an asset and load its model, specs, and `poi` list.
2. Pick an available POI; resolve its semantic `anchor.name` to a mesh, bone, or author-defined local coordinate.
3. Pick one of the POI's joke beats. A style array with two members is a mixed-style beat; it is **not** two sequential jokes.
4. Zoom/focus the camera, then display the caption and optional effects. `\n` can be revealed one line at a time.
5. Independently select a *real* numeric field for graphical effects; label its unit and preserve null handling. Never animate a pseudo-science claim as though it were physical data.
6. End the beat; make repeated selection avoid recent POIs and joke IDs where possible.

## Model integration

Coordinates are intentionally absent (`null`) because no 3D models were supplied. Semantic names such as `magazine`, `scope_turret`, and `ejection_port` need an asset-specific binding map. Some named parts might not exist visibly on a particular mesh: skip or remap them, never invent world-space coordinates. Keep the bind map separate from the content so models can be replaced without rewriting captions.

Suggested Rust types (illustrative):

```rust
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
struct Catalog {
    schema_version: String,
    weapons: HashMap<String, Weapon>,
}

#[derive(Deserialize)]
struct Weapon {
    display_name: String,
    specs: Specs,
    poi: Vec<Poi>,
}

#[derive(Deserialize)]
struct Specs { real_world: RealWorld }

#[derive(Deserialize)]
struct RealWorld {
    price_usd: Option<f64>,
    mass_kg: Option<f64>,
    capacity_rounds: Option<u32>,
}

#[derive(Deserialize)]
struct Poi { id: String, jokes: Vec<Joke> }

#[derive(Deserialize)]
struct Joke {
    id: String,
    styles: Vec<String>,
    text: String,
}
```

Use richer structs for the other fields and a style enum if the schema is frozen.

## Data limitations

- These are representative real-world weapon *families*, not certified measurements of the actual 3D models. Historical configurations and accessories may differ.
- `price_usd` is mostly `null`: an unsourced universal market price would be misleading. One sourced M3 Tactical variant price is included.
- Grenade and objective-prop IDs do not uniquely identify real products; leave unsupported physical fields empty.
- The comedic layer is original fiction and may intentionally contradict physics. Source links substantiate physical details, not jokes.
- Real-world details and 3D visual parts require manual model-by-model QA before release.

## QA acceptance checks

1. Exactly 32 expected IDs, 10 POIs for each, 320 or more joke beats total.
2. Every joke has nonempty `styles`, `text`, and a stable unique `id`.
3. Every `styles[]` entry belongs to the three allowed IDs.
4. No joke field is treated as numeric specification data.
5. `null` measurements are rendered as unavailable, never 0.
6. Every POI has a valid semantic target in the selected model, or the UI skips it gracefully.
7. Preview actual pacing with sound off *and* sound on: silence is sometimes part of the gag.
8. Troll Fact editorial test: identify the actual POI and check that the line reads as a deadpan, stupid observation—not a mini tutorial, marketing slogan, or random metaphor.
