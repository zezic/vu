# VU

A stereo VU meter that analyzes desktop audio.

## Requirements

- Rust: https://rustup.rs/
- A GPU with compute support for Vello's wgpu renderer.

## Run

Run with `cargo run --release`. On macOS, allow system audio recording when prompted.

The meters switch between side-by-side and stacked layouts as you resize the window,
choosing the larger fitted scales and centering the pair. In the stacked layout,
the left channel is above the right channel.

`A` / `S` cycle backward or forward through Cordex's 41 selected Gogh themes.
Aura is the default. Colors crossfade over 200 ms, including needles and overload
lamps. The theme readout sits at the bottom-left in both layouts, with `[A] [S] Theme Name`
on one left-aligned line and the range popup above the selector.

`Z` / `X` decrease or increase preamp by 6 dB. `Q` / `W` cycle backward or forward
through the frame limits: 30, 48, 55, 60, 72, 90, 120, 180, 240, and VSync.
VSync is the default and leaves frame pacing to display synchronization. Numeric
choices apply an additional maximum frame rate using Cordex's deadline-based limiter.

Drag horizontally to adjust the dB range, or vertically to adjust the scale bend.
The initial drag direction selects the setting for that gesture. Its value fades
in at the corresponding bottom corner and fades out over about 1.4 seconds after
release. Double-click to reset both values to the default range (53.4 dB) and bend
(2.0).

Needle motion uses elapsed time and averages each frame's exposure, with motion
blur for fast movement at low frame rates. Two triangular wings fade around each
needle using sweep gradients. The stroke and wings share a radial opacity mask,
keeping the base bright while the faster-moving tip spreads into the blur.
At rest the needle stays opaque. Needles stop at the gauge endpoints.

Theme, preamp, and frame limit are restored from `vu-config.ron` on launch. Changes are
saved every two seconds and on exit, using a temporary sibling file and rename.
The config uses Cordex's RON format and platform directory convention:

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
It uses synthetic levels and does not initialize audio capture. Set
`VU_RENDER_PREVIEW_PATH` to a PNG path to save the horizontal frame and a
`.vertical.png` companion for the stacked layout.

Gauge faces and text are rasterized by Vello and retained on the GPU until the
view, a displayed setting, or a fading readout changes. A single GPU render pass
combines that cached image with the moving needles, motion blur, and overload
lamps. Needle parameters use inline GPU constants when supported and a retained
uniform buffer otherwise. Motion updates and presentation keep the selected cadence.

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
