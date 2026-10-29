# VU

A stereo audio visualizer that drives a pseudo-3D wireframe model with the music
playing on the desktop.

## Requirements

- Rust: https://rustup.rs/
- A GPU with compute support for Vello's wgpu renderer.
- Run from the project root so the visualizer can read `assets/models/`.

## Run

Run with `cargo run --release`

On macOS, allow system audio recording when prompted.

## Hotkeys

- `B` - cycles the analyzer's refresh rate: FFT hop, 20, 10, 5, 2, and 1 Hz.
- `D` - toggles the analyzer debug panel.
- `Esc` - closes the analysis inspector without leaving the debug panel.
- `A` / `S` - cycle backward or forward through Cordex's 41 selected Gogh themes.
- `Z` / `X` - decrease or increase preamp by 6 dB, or 1 dB while holding `Shift`.
- `Q` / `W` - cycle backward or forward through the frame rate limits: 30, 48,
  55, 60, 72, 90, 120, 180, 240, and VSync.
- `-` / `=` - decrease or increase HUD text size in 10% steps, from 60% to
  200%. The setting persists in the config file.
- `C` / `V` - decrease or increase shutter exposure by half a frame, from 1 to
  16 frame intervals.
- `G` - toggle the light onset/drop timeline above the bottom hotkey line.
- `J` / `K` - decrease or increase the drop gap gate in 1 dB steps, from -60
  to 0 dBFS. The factory gate is -18 dBFS and the setting persists in the
  config file.
- `O` / `P` - decrease or increase the depth opacity modulation in 5% steps,
  from 0% (all vertices opaque) to 100% (the farthest vertices fully
  transparent). The setting persists in the config file.
- `U` / `I` - decrease or increase the minimum near-edge opacity. From 100%
  down to 10% the step is 5%; from 10% down to 1% it is 1%. The setting
  persists in the config file.
- `,` / `.` - decrease or increase the model crease threshold in 5 degree steps,
  from 10 to 80 degrees. Lower values keep more mesh edges; the setting persists
  in the config file.
- `\` - advance to the next model. Energy drops also advance it and toggle
  between normal and POI showcase modes.
- `T` - toggle analytical motion blur. The setting persists in the config file.
- `;` / `'` - decrease or increase the base line width in 10% steps, from 10%
  to 300%. The setting persists in the config file.
- `L` / `R` - decrease or increase the motion-blur opacity floor in 5% steps,
  from 0% to 100%. The factory floor is 35%. The setting persists in the config
  file.
- `Y` / `Shift+Y` - increase or decrease the motion-blur fade power from 0.1 to
  4.0 in 0.1 steps. Higher powers lower the opacity at middle speeds; values
  below 1.0 keep middle speeds brighter. The factory power is 1.0. The setting
  persists in the config file.

## Visualizer

VU loads every `.glb` model in `assets/models/` in name order, welds vertices
that glTF split along UV and normal seams, and keeps boundary edges plus crease
edges sharper than the adjustable crease threshold (45 degrees by default). It
centers the mesh, scales it to unit radius, and projects the wireframe in
perspective. Each model is drawn as vector lines with uniform weight; model
edges use flat square caps.

### Showcase modes

The model runs in either normal mode or POI focus mode. Normal mode keeps the
usual spin and flips. Entering POI focus rounds an inverted flip target up to
the next whole turn and holds the focus blend at the normal view until the
spring settles upright, so the POI camera never frames an upside-down model;
the flip spring keeps its full action through the motion fade. In
POI focus mode, the camera frames a semantic point of interest, the normal
model motion fades out over its own spring, and each kick
adds a signed random yaw and pitch step while changing FOV by a random step.
Kicks use a signed magnitude between `ORBIT MIN` (default 20 deg) and
`ORBIT MAX` (default 45 deg). The FOV step uses `FOV MIN` (default 10 deg)
and `FOV MAX` (default 20 deg). Entering POI mode or switching to another POI
draws a separate signed yaw and pitch step between `SWITCH MIN` (default
0 deg) and `SWITCH MAX` (default 45 deg), applied relative to the current
camera angle, so every switch starts from the view currently on screen rather
than from the view the POI started with. The camera base direction is never
re-aimed to a POI's outward side; the POI is centered by the focus target, but
entry and switches only move the orbit step and may view the POI from the side
the camera already had. The focus target and distance still travel to the new
POI. The FOV target stays inside 30-120 deg; the spring may briefly overshoot
it. The FOV and camera distance move together, so the POI keeps its
size while the surrounding model undergoes a dolly zoom. The entry blend
spring is free to overshoot; the camera distance,
near plane, and depth radius are interpolated in log space so an overshoot
moves past the target instead of crossing the camera through it. `KICK ATTACK`
(default 4/s) adds an initial velocity to the yaw,
pitch, and FOV springs when a kick sets a new target, so the move starts
immediately instead of waiting for the spring to accelerate from rest. Kicks
are ignored until the entry spring has settled, so the
initial flight is never interrupted by a kick-driven orbit or FOV change. After settling,
POI mode also runs a slow continuous yaw orbit (`ORBIT SPEED`, default 6 deg/s)
and a slow in/out dolly (`DOLLY DEPTH`, default 15%, over `DOLLY TIME`, default
12 s). Kicks do not shake the camera while the showcase is in POI mode.
Empty POIs use their scaled cube as the framing volume; mesh POIs are framed
and drawn in a contrasting color. When the focused POI is a mesh, every other
mesh part fades to `POI ATTEN` (default 12%) using the selector
fade-in/fade-out times, while the POI mesh keeps full opacity. If the POI mesh
has more than two child mesh parts, one child also keeps full opacity; `HAT`
triggers advance that child through the child list in order. Empty POIs leave
all meshes at full opacity. The `ejection_port` empty also
pulls the `slide` mesh and its children backward by the port length.

Snares drive the mode cycle. The `NORMAL FLIPS` setting counts the flips before
POI mode starts, and `POI SWITCHES` counts the POI changes before normal mode
returns. `POI GAP` (default 1 s, in the `SNARE` panel) starts when POI mode
is entered and after every switch. A snare that arrives before the gap expires
is handled as a kick instead: it applies the normal orbit/FOV step but does not
advance the switch or exit cycle. A switch measures each eligible POI from the
current one, min-max normalizes those distances, squares the closeness weight,
and keeps a 0.1 relative weight for the farthest candidate, so nearby targets
dominate while a long jump remains possible. A confirmed drop also toggles the
mode: normal switches to POI focus, and POI focus returns to normal. The drop
still advances the model, so the
entry is resolved against the newly selected model; if that model has no
resolvable POIs, normal mode remains. A gap-gate entry always returns to
normal mode and resets the cycle. All cycle and spring settings live in the
analyzer panel: `SNARE` holds the two counters, and the `SCENE` panel holds
separate motion, focus, orbit, and dolly springs, the slow orbit/dolly controls,
the per-kick step and POI switch ranges, the kick attack, and the selector
timing and attenuation controls.

The selector effect is available only in normal mode. `SEL ENTRY N` kicks
(default 4) enable it; the entry count restarts whenever the selector turns on
or off. While active it keeps one mesh at full opacity, fades the other meshes
to the configured attenuation (`SEL ATTEN`), and has a 25% chance to color the
selected mesh differently. Entering the selector returns the model's flip
spring to an upright pose and hides the selector effect and the POI mesh
attenuation until the spring has settled on that upright whole turn, including
any whole turn still owed to it. A pending entry stays armed and starts only
after the flip has finished. Kicks and snares count toward an exit threshold
(`SEL EXIT N`, default 20); the trigger that reaches the threshold turns the
selector off. A drop or a model change turns it off immediately. A gap-gate
entry no longer turns it off: the selector turns off only after the gap gate
stays armed continuously for `SEL GAP s` (default 10 s), and clearing the gap
resets that timer. The `HAT` detector only changes the
selected mesh and never exits the selector; in POI mode it advances the
full-opacity child of a mesh POI instead. Snares do not flip the model while
the selector is entering or active; the snare that reaches the exit threshold
also does not add a flip. The `HAT` detector has its own pattern editor and is
independent of the snare and drop detectors.

Edge opacity follows vertex depth: nearest vertices use the adjustable
near-opacity floor (60% by default, brightened by line energy), farther vertices
fade by the adjustable depth-fade strength (50% by default), and every edge
interpolates its opacity between those endpoints. While a POI is focused the
fade volume widens around it to three POI diameters, capped at the model
diameter. The model composites
additively on dark themes and multiplicatively on light themes; zero or full
color components are nudged by one 8-bit step so every channel contributes.

Energy drops and `\` advance to the next model, wrapping around, and the HUD
reports the current model position. The model is driven by spin, pitch, roll,
pulse, accent, and depth-opacity controls. The accent band picks the vivid theme
color used by the current model; the color is frozen when the model appears. The
inspector still exposes twist, bulge, ripple, shear, and jitter; the wireframe
stays rigid for now, so those vertex-distortion controls do not move it yet.

`T` optionally enables analytical motion blur. The motion-blur intensity is
`floor + (1 - floor) * (1 - speed)^power`: `L` / `R` move the opacity floor and
`Y` / `Shift+Y` shape the speed curve. The swept ribbons keep the theme
composition. `;` / `'` adjusts the base line width. The bottom-right HUD reports
the current model position, blur state, line width, opacity floor, fade power,
and live line count beside the shutter readout.

A 16-band spectrum analyzer covers 32 Hz to 16 kHz in log-spaced areas. The bands
drive rotation, camera tilt, twist, bulge, ripple, shear, jitter, line brightness,
and the pulse. Every band extracts a continuous onset flow from its FFT bins.
Each bin keeps a background envelope with configurable attack and release
(320 ms and 200 ms by default), and the positive relative excess over that
background is divided by `background + floor` (1% of the normalized magnitude
scale by default), capped (1.0 by default), and averaged over the band's bins.
The tracked low/high references give the band's absolute dynamic range,
`ABDR = high - low`; the mean ABDR across the bands is `WSDR`, and
`DRP = ABDR / WSDR`. The tracked high reference is scaled by the per-band
`ABL GAIN` (0.5 to 8.0) and shaped by the per-band curve (S-curve, amount 2.5 by
default), giving `ABL`. The continuous per-band onset flow is
`excess * DRP * ABL`. The visual band envelope still uses the input curve and
amount, but the onset detector reads the FFT bins directly and does not use the
band curve.

The kick, snare, and hi-hat detectors use 0.5 s temporal onset templates. A
template is defined by two polygons in the normalized time-by-band plane: the
inner polygon is the maximum, the outer polygon is the zero boundary, and the
field between them is interpolated. The resulting field is matched against the
recent per-band onset stream with a normalized dot product. A second,
independently editable suppression kernel supplies the inhibition projection,
scaled by the detector's `ONS SUPPRESS` control (0.5 by default). The residual
flow runs through the detector's own attack/release envelope and local-peak
detector; only the composite detectors emit boolean onsets. The kick, snare,
and hi-hat threshold follows the median amplitude of the onset peaks seen in
the last `ONS AVERAGE` seconds, so tracks with different dynamic range trigger
on their own onset scale instead of an absolute level. `ONS SENS` sets the
fraction of that median a peak must reach (0.5 by default), and `ONS THRESH`
is the absolute floor: a peak below it neither confirms nor enters the median.
`ONS GAP` is the minimum spacing between confirmed onsets, with factory gaps
of 95 ms for kick, 100 ms for snare, and 60 ms for hi-hat. A confirmed onset
reports its strength relative to the median, so a typical hit is 1.0 and a
half-strength hit is 0.5. The snare output also has an absolute RMS gate
(`RMS GATE`, -45 dBFS by default): a confirmed snare below the gate updates the
tracker but is not emitted, so it cannot flip the model, choke the kick, or
advance the mode cycle. The drop detector keeps the flux-level average
instead: its threshold follows a slow mean of the same residual, with
`ONS AVERAGE` as that mean's time constant. Each detector also has an
`ONS PRE` trim on the matched projection before tracking, so a loud projection
can be brought into the tracker's range. `KERNEL AREA` limits the template
from its start up to the chosen fraction, so the leftmost part of the kernel is
always kept; the end of that effective area is faded out over 10% of its
length. The factory templates are hand-tuned polygons over the half-second
canvas: the kick template spans the whole band range, the snare template covers
bands 04-15, the hi-hat template covers the top bands, and the suppression
kernels concentrate on the low bands.

The drop detector gates its onset with the broadband RMS of the analysis
window. The RMS passes through a fast-attack, slow-release envelope (20 ms and
350 ms) so a short dip between beats cannot open the gate. When the envelope
falls below the configurable gate (`GAP dB`, -18 dBFS by default), eligibility
latches high and stays high even if the level later rises; only a composite
onset can clear it and confirm the drop. Its onset kernel multiplies the
continuous per-band onset flow, subtracts its own inverted-kernel projection
(0.5 by default), and runs through the same attack/release envelope and
local-peak detector as the other composite detectors, with its own flux-level
average and dedicated onset settings. A kick emits a sequenced quake impulse and drives the
kick envelope; a snare flips the model's pitch baseline every Nth detection and
chokes the kick; a drop always changes the model, emits a vertical quake, and
chokes the kick. Flip and quake are damped springs, so both actions overshoot
slightly and settle.

All envelopes are smoothed with fast attack and slower release. Parameter
changes ease toward their new targets. Every moving edge uses the first and last
recorded poses across the shutter exposure and reconstructs the middle pose as
the median of the corresponding endpoints.

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
the body-edge `U/V -> 100` ramp on the attached base. The two ramps appear only
after both exposure poses have cleared the body cross-section. Until then the
sweep is the central body alone: a ramp drawn inside the body would double the
ink and take away the body's long-edge coverage. Only displacement across the
stroke clears the body, so an edge moving along its own direction also keeps
just the body.

The animation phase runs on a continuous clock and is rebased only at a period
that is an integer number of cycles for every phase-driven deformation. Crossing
2*PI therefore never snaps the geometry.

The selected Gogh theme drives the scene through its four neutral and twelve
vivid colors. Theme changes crossfade the wireframe palette along with the HUD.

Press `G` to show the light debug timeline above the bottom-right hotkey
readout. It spans the same four-second display-rate history as the inspector
graphs and carries five tracks: kick, snare, and hat onset marks; the drop
eligibility latch and RMS-envelope level in one drop lane, with the configured
dBFS gate drawn across it; and confirmed drop triggers. The timeline
redraws every display frame and is independent of the analysis panel, so `D`
can stay closed while it runs.

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
  kick, snare, hat, and drop hit totals, their trigger flashes, model name, and
  line and ribbon counts

Hover a band or property to highlight it. Hovering a property also shows its
description in the footer. Click an item to pin the inspector below the panel;
click it again (or press `Esc`) to close it. Clicking empty panel space clears
the selection.

The inspector follows the selection:

- selecting a band draws its smoothed and raw level history, its relative excess
  and continuous onset flow, its ABDR, ABL, and DRP values, any confirmed
  detector it feeds, and every animation property whose state depends on that
  band.
- selecting a property draws its own history plus a graph for every band and
  envelope that feeds its computation
- every property row carries small `M` and `S` buttons. `M` mutes that property
  so it contributes its neutral value, and `S` solos it so every property that
  is not soloed is muted. Rows that are currently silenced dim, and mute and
  solo persist with the rest of the inspector tuning.
- every dependency graph carries a short caption naming how its result reaches
  the selected property, for example `×0.18 add to energy`, `rate ×1.8 add`, or
  `excess + onset flow`
- four or more dependency graphs wrap into two rows so their traces and captions
  stay legible
- the property output graph carries small maximum buttons along its bottom
  edge: `1`, `2`, `4`, `8`, and `16`. Selecting one sets that graph's upper
  bound while keeping the property's lower bound; clicking the active value
  again restores the property's default range. Choices that would leave no
  span above the lower bound are dimmed. The choice is remembered per property
  while the debug panel is open and resets with the panel toggle.
- selecting a property also opens its computation tree on the left side of the
  screen, clear of the hotkey cheatsheet. If the window is too narrow for both
  panes, the graph inspector gives up width first instead of hiding the tree.
  The tree shows the full algebra from inputs to the property value:
  multipliers (`×`), adders (`+`), clamps, smoothing/integration, and onset
  switches with their kernel onset flux, gap gate, or confirmation state.
  Analysis leaves show their live values, and onset leaves report `IDLE`,
  `ARMED`, `TRIGGER`, or `CHOKED`. Tree rows use the same line height as
  the analyzer band list.
- every mix point is editable at runtime. Component rows carry a mix badge at
  their right edge: `+` means the term is added to the mix point and `×` means
  it multiplies the result. Clicking the badge switches the mode in place;
  selecting the row also exposes a `MIX MODE` button. A point's primary
  operation keeps its familiar shape: toggling a term in a sum point turns it
  into a factor of the sum, and toggling a term in a product point turns it into
  an additive offset. The tree's value and the live scene respond immediately.
  Constant `BASE` rows are selectable as well: their `VALUE` slider replaces the
  built-in constant for that mix point.
- selecting a mix point branch instead of a component exposes
  `+ ADD INPUT TO <POINT>`. The new row starts as a `PWR ENVELOPE` term with a
  small gain and additive mixing; selecting it exposes an `INPUT` selector for
  every band, envelope, raw level, onset, kick, phase clock, and style value,
  plus its own `EXTRA GAIN` slider, `MIX MODE` button, and `REMOVE EXTRA INPUT`
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
- attack uses a logarithmic slider over 0.1 to 50.0; release uses 0.1 to 20.0 so
  the short envelope times get most of the slider travel. Slider tracks and
  line-shaped thumbs are tall enough for comfortable dragging.
- clicking a detector row or its `DETECTOR` leaf shows that detector's
  parameters in the same bottom parameter section of the tree pane used by
  ordinary components. The `FLUX` button in the analyzer header opens the
  per-band onset analyzer: 16 bars show every band's continuous onset flow and
  a raw excess tick, and clicking a bar selects that band. The analyzer pane
  exposes the selected band's scaling controls (`LOW REF`, `HIGH REF`,
  `AUTO REFS`, `ABL GAIN`, `ABL CURVE`, `ABL AMT`) together with the shared
  relative-excess background settings (`BG ATTACK`, `BG RELEASE`,
  `EXCESS FLOOR`, `EXCESS CAP`), and the inspector extension graphs that band's
  raw excess and processed onset flow. A second graph below shows the raw band
  level with its live low/high reference markers and faint reference lines
  across the full history. Inside the analyzer pane, below the 16-band flow
  bank, two internal graphs plot `ABDR`/`WSDR`/`DRP` and `ABL`/`EXCESS`/`ONSET`
  for the selected band. The local-peak threshold and minimum interval are the
  `ONS THRESH` and `ONS GAP` controls of each `KICK`, `SNARE`, `HAT`, and
  `DROP` detector. `KICK` exposes its seven dedicated onset settings, main and
  suppression kernels, snare and drop choke times, quake multiplier, quake
  sequence length, and quake spring. `SNARE` exposes its seven dedicated onset
  settings, main and suppression kernels, the RMS gate, flip count, choke, and
  flip spring.
  `HAT` exposes its seven dedicated onset settings and main and suppression
  kernels.
  The editor is one overlay canvas over the 16-band onset-input spectrogram;
  there is no separate kernel window. The spectrogram and computed kernel use
  the same intensity gradient: transparent red (weakest), red, magenta, blue,
  cyan, green, and foreground (strongest). `CLEAR` resets the kernel to its
  initial 0.5 s polygons. The `KERNEL AREA` slider limits the matched fraction
  from the kernel start up to 10-100% and fades the end of that fraction.
  `OVERLAY` shows the yellow polygon editor over the spectrogram; `PREVIEW`
  shows the computed kernel field; `PAUSE` freezes the spectrogram; `SUPPRESS`
  switches the editor between the main and suppression kernels. The offset bar
  under the canvas shifts it left or right by up to 0.25 s while keeping the
  kernel fixed for comparison. Each kernel starts as two nested yellow polygons
  with four control points each. Drag a vertex to reshape it; hovering
  highlights the nearest control point or edge, with a point hitbox three times
  its drawn size. Double-click a point to delete it and double-click an edge to
  add one. The inner polygon is the high contour and the outer polygon is the
  zero contour, with the field interpolated between them. `DROP` exposes the
  dBFS gate, its seven dedicated onset settings, inverted-kernel suppression,
  and quake multiplier, plus the `ONSET` kernel bank. Press and drag across the
  bank to draw its shape; the brush interpolates across columns and release
  ends the stroke. Selecting `KICK`, `SNARE`, `HAT`, or `DROP` (or its `DETECTOR` leaf)
  switches the inspector extension to that detector's traces. Kick, snare, and
  hat show the composite template onset flow, its median reference, and the
  threshold derived from it, plus the trigger flash. A suppression graph sits
  directly below the onset graph and draws the suppression projection and the
  weighted part actually subtracted, both growing downward from a zero line at
  the top. Drop shows the raw RMS and smoothed envelope against the gate, the
  latched eligibility, composite onset flow and threshold, its suppression
  analysis, and trigger flash. The header reports the detector
  state, onset, reference, and threshold values, hit total, and last power.
  Temporal templates persist per detector
  as the two polygons; configs written before the polygon editor load their
  previous step templates or 16-band kick/snare kernels as initial polygons.
  Per-band ABL settings persist as one recipe per band, while the
  relative-excess settings are global. Configs with the previous per-band flux
  recipes still load and use the new defaults.
- every inspector edit persists in the config: detector changes (the drop gap
  gate is also reachable through the `J` / `K` hotkeys), envelope rates,
  kick decay, style values, component gains, band routing, input shaping, mix
  modes, extra inputs, base values, property mutes, solo state, and adaptive
  reference tracking. A tuned style value is applied to the current model and to
  the next model shown; untuned style values keep the defaults. Adaptive
  reference positions are tracking state, so only the `AUTO REFS` choice is
  stored and the references adapt again after a restart.
- composite onset graphs show the decaying trigger flash and mark confirmed
  triggers along the top. The band onset graph shows raw relative excess and
  the processed continuous onset flow, without per-band trigger marks. Every
  graph uses the same four-second display-rate history.

Occasionally a random model vertex gets a sci-fi diagnosis: corner brackets and
a crosshair mark the live vertex, a stepped polyline connects it to a
fixed-width readout, and the readout's box moves smoothly on a deliberately
sluggish anchor. The readout cycles between screen position, object-space
coordinates, velocity, and acceleration.

## Typography

The HUD, analyzer debug panel, and vertex diagnosis use Iosevka Tight Soft, with
Light, Light Italic, SemiBold, and SemiBold Italic faces assigned by role. The
bundled Iosevka files are stripped glyph subsets, so `Fonts::new` supplies their
family, width, weight, and style metadata during registration. HUD chrome uses
fixed logical pixel sizes: it scales with the display's DPI factor and the `-` /
`=` text-size setting, but never with window size. Overlay rows are centered by
the font's cap height rather than its tall ascender, so uppercase labels and the
vertex readout sit optically centered in their boxes.

## Config

- macOS: `~/Library/Application Support/com.zezic.vu/vu-config.ron`
- Linux: `$XDG_CONFIG_HOME/vu/vu-config.ron` (normally `~/.config/vu/vu-config.ron`)
- Windows: `%APPDATA%\zezic\vu\config\vu-config.ron`

## Audio Capture Details

On macOS 14.6+, a native Core Audio tap captures the stereo mix of all
applications, including playback through duplex interfaces and virtual devices
such as BlackHole. On Windows, CPAL WASAPI loopback captures the default output;
restart VU after changing that output or its sample rate.

On Linux, VU records the default input source. While VU is running, use your
audio system's routing UI to select the desktop output's monitor as VU's
recording source. For example, in `pavucontrol`, open the Recording tab and
select "Monitor of ..." for VU's stream. VU uses PulseAudio (including
PipeWire's PulseAudio compatibility service) when available, and otherwise uses
the default ALSA input.

Mono output feeds both analysis channels. For multichannel output, VU analyzes
the first two channels as left and right.

## Development

Run the tests with `cargo test`.

Run the headless wireframe preview with
`cargo test renders_wireframe_lines_and_swept_ribbons -- --ignored` (set
`VU_RENDER_PREVIEW_PATH` to save the PNG). The test uses synthetic levels and
does not initialize audio capture.

The retained HUD face is rasterized by Vello and reused on the GPU until the
viewport, a displayed setting, or an animated HUD value changes. A single GPU
render pass combines that cached image with the model lines and swept motion
ribbons.

## Profiling

Build with `cargo build --release`, then run `target/release/vu --profile`.
This opens the normal window at a 240 FPS limit with synthetic stereo audio and
zero preamp. It warms up for one second, measures for ten seconds, and exits.
The summary reports the GPU and pixel extent, achieved FPS, base rasterizations,
scene construction time, and presentation time split into drawable acquisition,
rasterization, and submission. These are wall-clock timings; acquisition can
wait for display synchronization. A second line reports actual process and main
thread CPU usage, excluding warmup. Here 100% means one fully busy core, and
process usage includes all of VU's threads. Resize the window during warmup to
compare sizes; keep it visible throughout measurement. The run skips audio
capture and leaves saved settings untouched.

Base rasterizations count frames that could not reuse both cached layers. The
animated overlay still renders every frame. The `rasterize` stage measures the
cached background work; `submit` includes the combined GPU pass and presentation
submission.

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

## Embedding

The crate exports the audio cursor, model visualizer, HUD, themes, and GPU line
renderer as a library. Feed borrowed output samples to `Ui::consume_buf` to drive
the interactive application, or drive `Visualizer` directly with `consume_buf`
and `update` for a custom host. `visualizer::Renderer` composites the retained
HUD background and the model wireframe.
