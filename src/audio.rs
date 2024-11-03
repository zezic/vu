use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};
use log::info;
use std::sync::mpsc::{Receiver, Sender};

use crate::AudioEvent;


pub fn audio_thread(tx: Sender<AudioEvent>, shutdown_rx: Receiver<()>) -> Result<(), anyhow::Error> {
    let host =  cpal::default_host();

    // Set up the input device and stream with the default input config.
    let device =  host.default_input_device()
    .expect("failed to find input device");

    println!("Input device: {}", device.name()?);

    let config = device
        .default_input_config()
        .expect("Failed to get default input config");
    println!("Default input config: {:?}", config);

    let err_fn = move |err| {
        eprintln!("an error occurred on stream: {}", err);
    };

    let stream = match config.sample_format() {
        cpal::SampleFormat::I8 => device.build_input_stream(
            &config.into(),
            move |data, _: &_| write_input_data::<i8, i8>(data, &tx),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.into(),
            move |data, _: &_| write_input_data::<i16, i16>(data, &tx),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I32 => device.build_input_stream(
            &config.into(),
            move |data, _: &_| write_input_data::<i32, i32>(data, &tx),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data, _: &_| write_input_data::<f32, f32>(data, &tx),
            err_fn,
            None,
        )?,
        sample_format => {
            return Err(anyhow::Error::msg(format!(
                "Unsupported sample format '{sample_format}'"
            )))
        }
    };

    stream.play()?;

    // Let recording go until shutdown
    shutdown_rx.recv().unwrap();

    Ok(())
}

fn write_input_data<T, U>(input: &[T], tx: &Sender<AudioEvent>)
where
    T: Sample,
    U: Sample,
    f32: FromSample<T>
{
    let mut data = vec![];
    for x in input {
        let s = x.to_sample::<f32>();
        data.push(s);
    }

    tx.send(AudioEvent::Buffer { buf: data }).unwrap();
}