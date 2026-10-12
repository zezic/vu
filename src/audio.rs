use anyhow::{bail, Context};
#[cfg(not(target_os = "macos"))]
use cpal::traits::HostTrait;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use std::sync::mpsc::Sender;

use crate::AudioEvent;

#[cfg(target_os = "macos")]
mod macos;

pub struct CaptureStream {
    // Fields drop in order: stop capture before destroying the native audio device.
    _stream: cpal::Stream,
    #[cfg(target_os = "macos")]
    _tap: macos::DesktopTap,
}

/// Captures desktop output on macOS and Windows, or the default recording source on Linux.
/// Keep the stream alive while the meter runs.
pub fn capture_stream(tx: Sender<AudioEvent>) -> anyhow::Result<CaptureStream> {
    if !cfg!(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux"
    )) {
        bail!("Audio capture is supported on macOS 14.6+, Windows, and Linux");
    }

    #[cfg(target_os = "macos")]
    let tap = macos::DesktopTap::new()
        .context("Failed to create a system audio tap; allow audio recording in macOS System Settings > Privacy & Security")?;
    #[cfg(target_os = "macos")]
    let device = tap.device();
    #[cfg(target_os = "windows")]
    let device = cpal::default_host()
        .default_output_device()
        .context("No default desktop output device is available")?;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let device = cpal::default_host()
        .default_input_device()
        .context("No default recording source is available")?;

    #[cfg(not(target_os = "windows"))]
    let config = device.default_input_config();
    #[cfg(target_os = "windows")]
    let config = device.default_output_config();
    let config = config.context("Failed to read the audio capture configuration")?;
    if config.channels() == 0 || config.sample_rate() == 0 {
        bail!("Invalid audio capture configuration: {config:?}");
    }
    log::info!("Audio capture: {device}, {config:?}");

    tx.send(AudioEvent::Config {
        samplerate: config.sample_rate() as usize,
    })?;

    let stream = match config.sample_format() {
        SampleFormat::I8 => build_capture::<i8>(&device, config, tx),
        SampleFormat::I16 => build_capture::<i16>(&device, config, tx),
        SampleFormat::I24 => build_capture::<cpal::I24>(&device, config, tx),
        SampleFormat::I32 => build_capture::<i32>(&device, config, tx),
        SampleFormat::I64 => build_capture::<i64>(&device, config, tx),
        SampleFormat::U8 => build_capture::<u8>(&device, config, tx),
        SampleFormat::U16 => build_capture::<u16>(&device, config, tx),
        SampleFormat::U24 => build_capture::<cpal::U24>(&device, config, tx),
        SampleFormat::U32 => build_capture::<u32>(&device, config, tx),
        SampleFormat::U64 => build_capture::<u64>(&device, config, tx),
        SampleFormat::F32 => build_capture::<f32>(&device, config, tx),
        SampleFormat::F64 => build_capture::<f64>(&device, config, tx),
        format => bail!("Unsupported capture sample format '{format}'"),
    }
    .context("Failed to build the audio capture stream")?;

    stream.play().context("Failed to start audio capture")?;
    Ok(CaptureStream {
        _stream: stream,
        #[cfg(target_os = "macos")]
        _tap: tap,
    })
}

fn build_capture<T>(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    tx: Sender<AudioEvent>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels());
    device.build_input_stream(
        config.into(),
        move |input: &[T], _| {
            let buf = stereo_frames(input, channels).collect();
            // The receiver is dropped when the window closes, before the stream is dropped.
            let _ = tx.send(AudioEvent::Buffer { buf });
        },
        |err| log::error!("Audio capture: {err}"),
        None,
    )
}

/// Mono feeds both needles; multichannel output uses the first left/right pair.
fn stereo_frames<T>(input: &[T], channels: usize) -> impl Iterator<Item = [f32; 2]> + '_
where
    T: Sample,
    f32: FromSample<T>,
{
    assert!(channels > 0, "audio must have at least one channel");
    assert_eq!(input.len() % channels, 0, "incomplete audio frame");
    input.chunks_exact(channels).map(move |frame| {
        let left = frame[0].to_sample::<f32>();
        let right = if channels == 1 {
            left
        } else {
            frame[1].to_sample::<f32>()
        };
        [left, right]
    })
}

#[cfg(test)]
mod tests {
    use super::stereo_frames;

    #[test]
    fn converts_mono_stereo_and_multichannel_to_meter_frames() {
        assert_eq!(
            stereo_frames(&[0.25_f32, -0.5], 1).collect::<Vec<_>>(),
            [[0.25, 0.25], [-0.5, -0.5]]
        );
        assert_eq!(
            stereo_frames(&[0.25_f32, -0.5, 0.75, 1.0], 2).collect::<Vec<_>>(),
            [[0.25, -0.5], [0.75, 1.0]]
        );
        assert_eq!(
            stereo_frames(&[0_i16, i16::MIN, 123, 456, 16384, 8192, 789, 1000], 4)
                .collect::<Vec<_>>(),
            [[0.0, -1.0], [0.5, 0.25]]
        );
    }
}
