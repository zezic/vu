# VU

A stereo VU meter that analyzes desktop audio.

Run with `cargo run --release`. On macOS, allow system audio recording when prompted.

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

Run the signal conversion and meter tests with `cargo test`.
