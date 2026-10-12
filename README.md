# VU

A stereo audio meter with VU, quasi-peak PPM, RMS, and sample-peak modes.

## Requirements

- Rust: https://rustup.rs/
- A GPU with compute support for Vello's wgpu renderer.

## Run

Run with `cargo run --release`. On macOS, allow system audio recording when prompted.

The meters switch between side-by-side and stacked layouts as you resize the window,
choosing the larger fitted scales and centering the pair. In the stacked layout,
the left channel is above the right channel.

`M` cycles VU, BBC-style PPM, custom 300 ms RMS, and Sample Peak. The current
mode appears above the theme selector as `[M] Current mode`. RMS is the default.
Each gauge shows its units above the overload lamp: `VU` or `dBFS`.

In PPM and Sample Peak, `N` switches the physical needle between **Full Spec**
and **Vintage FR**, the two Sifam response curves. The `[N]` selector sits above
`[M]` at bottom-left. Full Spec is the default and reaches full deflection near
100 ms with about 5.4% overshoot. Vintage FR reaches full deflection near 200 ms
with about 2.5% overshoot. Selection preserves the current pose, momentum, and
shutter history, and persists across mode changes and restarts.

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
0 VU corresponds to a sine with RMS amplitude 1 after preamp. The default +18 dB
preamp places that reference at an input RMS amplitude of approximately 0.126.
Switching modes clears detector and needle history. Floating-point samples above
full scale are measured without clipping.

`A` / `S` cycle backward or forward through Cordex's 41 selected Gogh themes.
Aura is the default. Colors crossfade over 200 ms, including needles and overload
lamps. The theme readout sits at the bottom-left in both layouts, with `[A] [S] Theme Name`
on one left-aligned line. The range popup appears above the selector stack.

`Z` / `X` decrease or increase preamp by 6 dB. `Q` / `W` cycle backward or forward
through the frame limits: 30, 48, 55, 60, 72, 90, 120, 180, 240, and VSync.
VSync is the default and leaves frame pacing to display synchronization. Numeric
choices apply an additional maximum frame rate using Cordex's deadline-based limiter.

In the dBFS modes, drag horizontally to adjust the dB range, or vertically to adjust
the scale bend. VU keeps its fixed amplitude-proportional scale.
The initial drag direction selects the setting for that gesture. Its value fades
in at the corresponding bottom corner and fades out over about 1.4 seconds after
release. Double-click to reset both values to the default range (53.4 dB) and bend
(2.0).

`C` / `V` decrease or increase shutter exposure by half a frame, from 1 to 16
frame intervals. The bottom-right readout shows `Shutter 2.5f [C] [V]` for a 2.5-frame exposure. `1.0` is the
original full-frame exposure; `2.0` spans the latest two frames. Fractional values
include the trailing fraction of the oldest frame. Frame durations weight the
average, so changing FPS keeps the exposure tied to frame intervals. The needle's
physical response stays the same; longer exposures broaden blur and move the
visible stroke to the average over that longer path. At startup, the exposure
uses the available history. You can also set `shutter_frames: 2.5` in the config.

Needle motion models inertia, restoring torque, and damping. VU and peak modes
allow small overshoot; RMS uses critical damping. Full Spec uses a 5 Hz natural
frequency and 0.68 damping ratio; Vintage FR uses 3.07 Hz and 0.763. Both are
fits to the [Sifam movement responses](https://www.don-audio.com/mediafiles/datasheets/Sifam-Vintage_FR-Meters.pdf). Audio-time drive spans preserve
short events between display frames. The selected exposure averages that path, with motion
blur for fast movement at low frame rates. Two triangular wings fade around each
needle using sweep gradients. The stroke and wings share a radial opacity mask,
keeping the base bright while the faster-moving tip spreads into the blur.
At rest the needle stays opaque. Needles stop at the gauge endpoints.

Mode, peak movement, theme, preamp, frame limit, and shutter exposure are restored from `vu-config.ron` on launch. Changes are
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

The config mode variants are `Vu`, `Ppm`, `Rms`, and `SamplePeak`, for example
`mode: Ppm`. Configurations without a mode use `Rms`. Set
`peak_movement: FullSpec` or `peak_movement: VintageFr` to choose a physical peak
needle. Configurations without a peak movement use `FullSpec`.

Run the headless meter rendering test with
`cargo test renders_stereo_scales_text_needles_and_overload -- --ignored`.
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
