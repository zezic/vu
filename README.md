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

`Z` / `X` decrease or increase preamp by 6 dB. `Q` / `W` cycle backward or forward
through the frame limits: 30, 48, 55, 60, 72, 90, 120, 180, 240, and VSync.
VSync is the default and leaves frame pacing to display synchronization. Numeric
choices apply an additional maximum frame rate using Cordex's deadline-based limiter.

Preamp and frame limit are restored from `vu-config.ron` on launch. Changes are
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

Run the headless Vello rendering test with
`cargo test renders_stereo_scales_text_needles_and_overload -- --ignored`.
It uses synthetic levels and does not initialize audio capture. Set
`VU_RENDER_PREVIEW_PATH` to a PNG path to save the horizontal frame and a
`.vertical.png` companion for the stacked layout.
