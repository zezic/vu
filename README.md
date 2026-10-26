# VU

A stereo audio meter with VU, quasi-peak PPM, RMS, and sample-peak modes.

![VU stereo audio meter](./assets/screenshot.png)

## Requirements

- Rust: https://rustup.rs/
- A GPU with compute support for Vello's wgpu renderer.

## Run

Run with `cargo run --release`

On macOS, allow system audio recording when prompted.

## Hotkeys

- `B` - cycles the visualizer's analysis refresh rate: FFT hop, 20, 10, 5, 2,
and 1 Hz.
- `D` - toggles the visualizer's analysis debug panel.
- `Esc` - closes the analysis inspector without leaving the debug panel.
- `F` - toggles between the physical gauges and the wireframe visualizer.
- `M` - cycles VU, BBC-style PPM, custom 300 ms RMS, and Sample Peak.
- `N` - In PPM and Sample Peak, switches the physical needle between **Full Spec**
and **Vintage FR**, the two Sifam response curves.
- `A` / `S` - cycle backward or forward through Cordex's 41 selected Gogh themes.
- `Z` / `X` - decrease or increase preamp by 6 dB, or 1 dB while holding `Shift`.
- `Q` / `W` - cycle backward or forward
through the frame rate limits: 30, 48, 55, 60, 72, 90, 120, 180, 240, and VSync.
- `-` / `=` - decrease or increase visualizer UI text size in 10% steps, from 60%
to 200%. The setting persists in the config file.
- `C` / `V` - decrease or increase shutter exposure by half a frame, from 1 to 16
frame intervals. 
- `G` / `H` - decrease or increase the kick onset minimum gap in 5 ms steps,
from 5 to 200 ms.
- `J` / `K` - decrease or increase the drop gap confidence threshold in 0.05
steps, from -1.0 to +1.0. Both detector settings persist in the config file.
- `O` / `P` - decrease or increase the depth opacity modulation in 5% steps, from
0% (all vertices opaque) to 100% (the farthest vertices fully transparent). The
setting persists in the config file.
- `U` / `I` - decrease or increase the minimum near-edge opacity in 5% steps, from
60% to 100%. The setting persists in the config file.

## Modes

- **VU:** sine-calibrated rectified-average drive into a moving-coil model. The
  movement reaches 99% of a step in 300 ms and overshoots by 1.25%. Its deflection
  is proportional to amplitude, with a fixed scale from -20 to +6 VU.
- **PPM:** BBC-style quasi-peak detection with parallel slow and fast charging
  envelopes. The detector reads a 10 ms, 5 kHz tone burst approximately 2 dB below
  a sustained tone and returns by 26 dB in 3 seconds. Its physical needle follows
  the selected Sifam movement curve. Full Spec reaches about full deflection at
  100 ms, with approximately 5.4% overswing near 136 ms and settling around
  250 ms. Vintage FR is slower with less overswing. Pointer
  overswing can make its maximum indication exceed the detector's reading.
- **RMS:** a 300 ms rectangular mean-square window followed by a critically damped
  needle. This is the custom meter, with two temporal responses in series.
- **Sample Peak:** detects absolute sample peaks, holds them for 100 ms, and then
  returns at 20 dB per second, driving the same Sifam-style physical movement.
  It measures stored samples; it does not reconstruct intersample true peaks.

Preamp applies to every mode. In the dBFS modes, amplitude 1 after preamp is
0 dBFS; a full-scale sine therefore reads about -3.01 dBFS in RMS mode. For VU,
0 VU corresponds to a sine with RMS amplitude 1 after preamp.

In the dBFS modes, drag horizontally to adjust the dB range, or vertically to adjust
the scale bend. VU keeps its fixed amplitude-proportional scale.
Double-click to reset both values to the default range (53.4 dB) and bend
(2.0).

## Visualizer

![Abstract wireframe visualizer](./assets/visualizer.png)

Press `F` to replace the gauges with a pseudo-3D wireframe scene. The scene starts
as a simple cube and evolves through an octahedron, icosahedron, tesseract, and
torus as energy hits arrive. Each figure is projected in perspective and drawn as
vector lines with uniform weight. Edge opacity follows vertex depth: nearest
vertices use the adjustable near-opacity floor (60% by default, brightened by
line energy), farther vertices fade by the adjustable depth-fade strength (50% by
default), and every edge interpolates its opacity between those endpoints. Motion
blur carries the same per-vertex opacities through the exposure.

A 16-band spectrum analyzer covers 32 Hz to 16 kHz in log-spaced areas. The bands
drive rotation, camera tilt, twist, bulge, ripple, shear, jitter, line brightness,
and the pulse. The accent band shifts which clean vivid theme color each newly
spawned figure takes.
Every band runs a spectral-flux onset detector. Each FFT frame compresses every
bin to `log1p(magnitude)`, takes the positive difference from the previous frame,
and sums it over the band's bins. The summed flux is smoothed by an attack/release
exponential filter (5 ms attack, 50 ms release by default). A slow exponential
average of the smoothed flux (200 ms by default) forms an adaptive threshold;
sensitivity and a minimum threshold set how far a peak must rise, local-peak
detection confirms the turn, and a 30 ms minimum gap prevents chatter. Each onset
carries the peak's ratio to its adaptive threshold as its power.

The kick and snare detectors each project the 16-band spectral flux through a
drawable onset kernel and run the same attack/release envelope, slow adaptive
average, and local-peak detector, with their own dedicated flux settings. The
kick kernel defaults to bands 00-04 and the snare kernel to bands 05, 06, 10,
and 12.
The drop detector projects the 16 band levels through two drawable kernels: a
low kernel and a high kernel. Each projection is normalized by its kernel weight
sum, and the high-minus-low difference is averaged over a rolling window
(1.0 s by default). The mean is multiplied by a confidence gain (1.0 by
default), which scales it around zero before the threshold comparison. When the
scaled mean reaches the confidence threshold (0.15 by default), eligibility
latches high and stays high even if the confidence later falls; only a composite
onset from a third, independent onset kernel can clear it and confirm the drop.
A short deep gap and a long medium gap therefore contribute to the same
confidence budget, and the threshold may be negative. Its onset kernel projects
the per-band spectral flux and runs through the same attack/release envelope,
slow adaptive average, and local-peak detector as the per-band onsets, but with
its own dedicated flux settings. A kick emits a sequenced quake impulse and
drives the kick envelope; a snare flips the figure's pitch baseline every Nth
detection and chokes the kick; a drop always changes the scene, emits a vertical
quake, and chokes the kick. Flip and quake are damped springs, so both actions
overshoot slightly and settle.

All envelopes are smoothed with fast attack and slower release. Shape changes
crossfade and parameter changes ease toward their new targets. Every moving edge
uses the first and last recorded poses across the shutter exposure and
reconstructs the middle pose as the median of the corresponding endpoints.

For an ordinary sweep, the reconstructed middle pose carries a normal-width
thick body at peak opacity `P`; the start-to-middle span fades `0 -> P`, and the
middle-to-end span fades `P -> 0`. `P` falls as the frame-one to frame-three
vertex displacement grows, so a fast edge spreads into a fainter blur while a
slow edge stays crisp.

The ordinary sweep is one continuous triangle strip: incoming gradient, body,
and outgoing gradient. The body is the middle band of that strip, so its long
edges and all boundary vertices/colors are shared directly with the neighboring
gradient sections. No separate body mesh or line primitive is used.

The strip normal is oriented from the first pose to the last pose, so projected
back-facing edges use the same attachment order as front-facing edges. Crossing
bodies orient their normal so that `A1` stays on the `X1` side.

When the first and last edge lines intersect, the crossing is rendered as a
flat-capped thick body plus four wings. The body spans the median line `AM-BM`
with normal line thickness (half-thickness `max(0.5 * edge_width, 0.5 px)`). Its
two long edges have their midpoints at `X1` (top) and `X2` (bottom).

The wings are:

- top-left: `A1 - L_top - X1`
- top-right: `B2 - R_top - X1`
- bottom-left: `A2 - L_bottom - X2`
- bottom-right: `B1 - R_bottom - X2`

The body long edges carry the same `U -> 100 -> V` gradients as the wing bases,
so the colors match exactly where they meet. The body's flat caps carry `U` at
`AM` and `V` at `BM`. Both `X1` and `X2` sit on the body edges, never on the
inner center line, so the crossing keeps the full body thickness.

The outer wing edges `A1-X1`, `B2-X1`, `A2-X2`, and `B1-X2` stay fully
transparent. The wing shader masks the outer-to-X edge to zero while preserving
the body-edge `U/V -> 100` ramp on the attached base.
There is no convexity or small-displacement gate on the two ramps: any edge with
nonzero screen displacement emits both. An edge with zero screen displacement
keeps only its central line, because there is no spread to draw.

The animation phase runs on a continuous clock and is rebased only at a period
that is an integer number of cycles for every phase-driven deformation and
tesseract rotation. Crossing 2*PI therefore never snaps the geometry.

The selected Gogh theme drives the scene through its four neutral and twelve vivid
colors. A figure receives one clean vivid theme color when it spawns and keeps it
until it leaves the scene; music does not recolor it afterwards. Every edge of a
figure uses that color, except the torus, which alternates two vivid colors. The
tesseract therefore draws all of its edges in one color instead of striping the
nested cubes. No edge color is blended with the background or neutral tones. Theme
changes crossfade both the gauge colors and the wireframe palette.

Press `D` to show the live analysis panel. It refreshes once per completed FFT
hop by default; `B` cycles through FFT-hop, 20, 10, 5, 2, and 1 Hz. The panel
renders:

- all sixteen smoothed bands with raw values, their scene roles, and a short
  onset-history strip left of each level bar. Each row names what the band
  drives: `V` marks a dependency on the band value, `K` a kick sequence, `S` a
  snare slot, and `D` the drop detector.
- the live animation properties in a two-column grid: pulse, spin, pitch, roll,
  twist, bulge, ripple, shear, zoom, accent, line energy, jitter, phase, kick,
  flip, quake, crossfade, style morph, and entrance
- broadband, bass, and treble envelopes with raw levels in the footer, plus
  kick, snare, and drop hit totals, their trigger flashes, scene name, and line
  and ribbon counts

Hover a band or property to highlight it. Hovering a property also shows its
description in the footer. Click an item to pin the inspector below the panel;
click it again (or press `Esc`) to close it. Clicking empty panel space clears
the selection.

The inspector follows the selection:

- selecting a band draws its smoothed and raw level history, its raw and
  smoothed spectral flux with the adaptive threshold, its onset ticks, any
  confirmed detector it feeds, and every animation property whose state depends
  on that band.
- selecting a property draws its own history plus a graph for every band and
  envelope that feeds its computation
- every property row carries small `M` and `S` buttons. `M` mutes that property
  so it contributes its neutral value, and `S` solos it so every property that
  is not soloed is muted. Rows that are currently silenced dim, and mute and
  solo persist with the rest of the inspector tuning.
- every dependency graph carries a short caption naming how its result reaches
  the selected property, for example `×0.18 add to energy`, `rate ×1.8 add`, or
  `raw + smoothed flux / threshold`
- four or more dependency graphs wrap into two rows so their traces and captions
  stay legible
- the property output graph carries small maximum buttons along its bottom
  edge: `1`, `2`, `4`, `8`, and `16`. Selecting one sets that graph's upper
  bound while keeping the property's lower bound; clicking the active value
  again restores the property's default range. Choices that would leave no
  span above the lower bound are dimmed. The choice is remembered per
  property while the debug panel is open and resets with the panel toggle.
- selecting a property also opens its computation tree on the left side of the
  screen, clear of the hotkey cheatsheet. If the window is too narrow for both
  panes, the graph inspector gives up width first instead of hiding the tree.
  The tree shows the full algebra from inputs to the property value:
  multipliers (`×`), adders (`+`), clamps, smoothing/integration, and onset
  switches with their kernel onset flux, gap confidence, or confirmation
  window. Analysis leaves show their live values, and
  onset leaves report `IDLE`, `ARMED`, `TRIGGER`, or `CHOKED`. Tree rows use the
  same line height as the analyzer band list.
- every mix point is editable at runtime. Component rows carry a mix badge at
  their right edge: `+` means the term is added to the mix point and `×` means
  it multiplies the result. Clicking the badge switches the mode in place;
  selecting the row also exposes a `MIX MODE` button. A point's primary
  operation keeps its familiar shape: toggling a term in a sum point turns it
  into a factor of the sum, and toggling a term in a product point turns it
  into an additive offset. The tree's value and the live scene respond
  immediately. Constant `BASE` rows are selectable as well: their `VALUE`
  slider replaces the built-in constant for that mix point.
- selecting a mix point branch instead of a component exposes
  `+ ADD INPUT TO <POINT>`. The new row starts as a `PWR ENVELOPE` term with a
  small gain and additive mixing; selecting it exposes an `INPUT` selector for
  every band, envelope, raw level, onset, kick, phase clock, and style value, plus
  its own `EXTRA GAIN` slider, `MIX MODE` button, and `REMOVE EXTRA INPUT`
  button. Any number of extra inputs can be added to a point.
- clicking a component row opens its adjustable parameters at the bottom of the
  tree pane: band source, envelope attack/release, the component's multiplier
  gain in the selected property, the kick decay, or the corresponding style
  value. For example the SPIN band row exposes a BAND selector, attack, release,
  and the band gain. Changing BAND reroutes that semantic component to another
  of the sixteen FFT bands, and the tree labels, band dependency text, and
  inspector graphs follow the new route. Drag a slider to change the live value.
- each enveloped component also exposes its pre-envelope input shaping:
  `LOW REF` / `HIGH REF` rescale the raw value into 0..1, `CURVE` selects
  linear, inflated, deflated, or S-curve response, and `CURVE AMT` sets how far
  the response departs from linear. Zero is linear, one is the named bend, and
  higher amounts keep bending: inflated and deflated raise the value to
  stronger powers, while the S-curve applies the smoothstep again once per
  unit. A small `INPUT SHAPE` plot previews the selected curve across the
  low/high reference span, with a linear reference diagonal.
- `AUTO REFS` turns on adaptive reference tuning for one shaped input. The high
  reference falls slowly toward quiet material and is bumped up quickly by
  peaks; the low reference lifts slowly and is pushed down quickly by low
  values. Both references stay ordered and inside 0..1, the `INPUT SHAPE`
  preview follows them, and their live positions are marked with small
  triangles on the corresponding inspector graph.
- attack uses a logarithmic slider over 0.1 to 50.0; release uses 0.1 to 20.0 so the short
  envelope times get most of the slider travel. Slider tracks and line-shaped
  thumbs are tall enough for comfortable dragging.
- clicking a detector row or its `DETECTOR` leaf shows that detector's parameters
  in the same bottom parameter section of the tree pane used by ordinary
  components. `FLUX ONSET` exposes attack, release, average, sensitivity,
  minimum threshold, and minimum gap. `KICK` exposes its six dedicated
  onset-flux settings, a kick kernel bank, snare and drop choke times, quake
  multiplier, quake sequence length, and quake spring. `SNARE` exposes its six
  dedicated onset-flux settings, a snare kernel bank, flip count, choke, and
  flip spring. `DROP` exposes the gap window, confidence gain, confidence
  threshold, its six dedicated onset-flux settings, and quake multiplier, plus
  three kernel banks (`LOW`, `HIGH`, `ONSET`). Press and drag across a bank to
  draw its shape; the brush interpolates across columns and release ends the
  stroke. No separate pane is created. Selecting `KICK`, `SNARE`, or `DROP`
  (or its `DETECTOR` leaf) switches the inspector extension to that detector's
  traces. Kick and snare show the composite kernel onset flux against its
  adaptive threshold, plus the trigger flash. Drop shows the low and high
  projections, their difference, window confidence, latched eligibility,
  composite onset flux and threshold, and trigger flash. The header reports the
  detector state, flux or gap values against the threshold, hit total, and last
  power.
- every inspector edit persists in the config: detector changes (also reachable
  through the `G` / `H` and `J` / `K` hotkeys), envelope rates, kick decay, style
  values, component gains, band routing, input shaping, mix modes, extra inputs,
  base values, property mutes, solo state, and adaptive reference tracking. A
  tuned style value is applied to the current figure and to every figure spawned
  by a later drop; untuned style values keep their random variation. Adaptive
  reference positions are tracking state, so only the `AUTO REFS` choice is
  stored and the references adapt again after a restart.
- onset graphs show the decaying trigger flash and mark confirmed triggers along
  the top. The band flux graph shows raw and smoothed flux plus the adaptive
  threshold, and marks each onset. Every graph uses the same four-second
  display-rate history, and each band row's onset strip covers the last 0.6
  seconds, marking only full-opacity trigger blocks at confirmed flux peaks.

![Wireframe visualizer with analysis debug panel](./assets/visualizer-debug.png)

Occasionally a random figure vertex gets a sci-fi diagnosis: corner brackets and a
crosshair mark the live vertex, a stepped polyline connects it to a fixed-width
readout, and the readout's box moves smoothly on a deliberately sluggish anchor.
The readout cycles between screen position, object-space coordinates, velocity,
and acceleration.

## Typography

The complete VU meter face - scale markings, units, readouts, and hotkey hints -
uses the bundled D-DIN face. The visualizer HUD, analyzer debug panel, and vertex
diagnosis use Iosevka Tight Soft, with Light, Light Italic, SemiBold, and SemiBold
Italic faces assigned by role. The bundled Iosevka files are stripped glyph
subsets, so `Fonts::new` supplies their family, width, weight, and style metadata
during registration. Visualizer chrome uses fixed logical pixel sizes: it scales
with the display's DPI factor and the `-` / `=` text-size setting, but never with
window size. Overlay rows are centered by the font's cap height rather than its
tall ascender, so uppercase labels and the vertex readout sit optically centered
in their boxes.

## Config

- macOS: `~/Library/Application Support/com.zezic.vu/vu-config.ron`
- Linux: `$XDG_CONFIG_HOME/vu/vu-config.ron` (normally `~/.config/vu/vu-config.ron`)
- Windows: `%APPDATA%\zezic\vu\config\vu-config.ron`

## Audio Capture Details

On macOS 14.6+, a native Core Audio tap captures the stereo mix of all applications,
including playback through duplex interfaces and virtual devices such as BlackHole.
On Windows, CPAL WASAPI loopback captures the default output; restart VU after
changing that output or its sample rate.

On Linux, VU records the default input source. While VU is running, use your audio
system's routing UI to select the desktop output's monitor as VU's recording source.
For example, in `pavucontrol`, open the Recording tab and select "Monitor of ..."
for VU's stream. VU uses PulseAudio (including PipeWire's PulseAudio compatibility
service) when available, and otherwise uses the default ALSA input.

Mono output feeds both needles. For multichannel output, the meter analyzes the first
two channels as left and right.

## Development

Run the signal conversion and meter tests with `cargo test`.

Run the headless meter rendering test with
`cargo test renders_stereo_scales_text_needles_and_overload -- --ignored`.
Run the wireframe overlay preview with
`cargo test renders_wireframe_lines_and_swept_ribbons -- --ignored`.
Run the full visualizer HUD and diagnosis preview with
`cargo test renders_visualizer_face_and_wireframe_with_hud -- --ignored` (set
`VU_RENDER_PREVIEW_PATH` to save the PNG).
Verify every mode in both layouts with
`cargo test renders_all_measurement_modes_and_units_in_both_layouts -- --ignored`.
Set `VU_MODE_PREVIEW_PREFIX` to a path prefix to save PNG previews for both
needle movements and layouts.
These tests use synthetic levels and do not initialize audio capture. Set
`VU_RENDER_PREVIEW_PATH` to a PNG path to save the horizontal frame and a
`.vertical.png` companion for the stacked layout.

Gauge faces and text are rasterized by Vello and retained on the GPU until the
view, a displayed setting, or a fading readout changes. A single GPU render pass
combines that cached image with the moving needles, motion blur, and overload
lamps. Needle parameters use inline GPU constants when supported and a retained
uniform buffer otherwise.

## Profiling

Build with `cargo build --release`, then run `target/release/vu --profile`.
This opens the normal window at a 240 FPS limit with synthetic stereo audio and
zero preamp. It warms up for one second, measures for ten seconds, and exits.
The summary reports the GPU and pixel extent, achieved FPS, base rasterizations,
scene construction time, and presentation time split into drawable acquisition,
rasterization, and submission. These are wall-clock timings; acquisition can wait for display
synchronization. A second line reports actual process and main-thread CPU usage,
excluding warmup. Here 100% means one fully busy core, and process usage includes
all of VU's threads. Resize the window during warmup to compare sizes;
keep it visible throughout measurement. The run skips audio capture and leaves
saved settings untouched.

Base rasterizations count Vello work for the retained scales and labels. Zero
during steady needle motion is expected: the overlay still renders every frame.
The `rasterize` stage measures that base work; `submit` includes the combined
GPU pass and presentation submission.

On macOS, measure process CPU time with
`/usr/bin/time -lp target/release/vu --profile`. Divide `user + sys` by `real`
and multiply by 100 for average CPU usage, where 100% means one fully busy core.
This external average includes startup and warmup. For a stack sample:

```sh
target/release/vu --profile &
vu_pid=$!
sample "$vu_pid" 8 1 -file /tmp/vu.sample.txt
wait "$vu_pid"
```

To isolate rendering from the window and presentation loop, run
`cargo test --release profiles_moving_needles_at_240_fps -- --ignored --nocapture`.
It drives the meter's audio processor, motion, retained background, and GPU
overlay for 1,200 measured frames after warmup. Set `VU_PROFILE_MAX_RENDER_US`
to a positive per-frame microsecond budget to make it fail when rendering
exceeds that budget. Use release builds and compare runs on the same GPU.

The ignored `overlay_matches_vector_stroke_sweep_mask_and_overload` test compares
the overlay against vector rendering at rest, during rapid motion, in both layouts,
and after zooming, including subpixel strokes and overload gradients.

## Embedding

The crate exports its existing renderer, measurement modes, scales, themes, and
text layouts as a library. Feed borrowed output samples to `Meter::consume_buf`.
`Meter::update_gauges` uses the same detectors, lamp history, motion, stop contact,
permanent bending, shutter exposure and subpixel appearance comparison as the
standalone application's `Meter::draw`. `Meter::draw_gauge_faces` draws the same
scales, stop dots and lamp recesses at host-supplied transforms.

`meter::SWEEP_SHADER` supplies the shared geometric sweep-wing coverage and radial
opacity functions. `meter::ShutterWindow` supplies the frame-based shutter window
used by recorded motion sources.
