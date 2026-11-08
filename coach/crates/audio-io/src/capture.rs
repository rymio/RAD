//! Microphone capture. The cpal stream lives on a dedicated thread (it is not `Send`
//! on macOS); 16 kHz mono chunks arrive on a channel.

use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

use crate::resample::{downmix, Resampler};
use crate::{AudioError, Result, ENGINE_RATE};

/// A running microphone capture. Dropping it stops the stream.
pub struct Capture {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    pub device_name: String,
    pub device_rate: u32,
    pub device_channels: u16,
}

impl Capture {
    /// Start capturing from the default input device.
    /// Returns the capture handle and a receiver of 16 kHz mono chunks.
    pub fn start() -> Result<(Self, Receiver<Vec<f32>>)> {
        let (data_tx, data_rx) = unbounded::<Vec<f32>>();
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let (ready_tx, ready_rx) = bounded::<Result<(String, u32, u16)>>(1);

        let thread = std::thread::Builder::new()
            .name("mic-capture".into())
            .spawn(move || {
                let started = (|| -> Result<(cpal::Stream, String, u32, u16)> {
                    let host = cpal::default_host();
                    let device = host.default_input_device().ok_or(AudioError::NoInputDevice)?;
                    let name = device.description().map(|d| d.to_string()).unwrap_or_else(|_| device.to_string());
                    let supported = device.default_input_config()?;
                    let rate = supported.sample_rate();
                    let channels = supported.channels();
                    let format = supported.sample_format();
                    let config: cpal::StreamConfig = supported.into();
                    let stream = match format {
                        SampleFormat::F32 => build::<f32>(&device, config, data_tx)?,
                        SampleFormat::I16 => build::<i16>(&device, config, data_tx)?,
                        SampleFormat::I32 => build::<i32>(&device, config, data_tx)?,
                        SampleFormat::U16 => build::<u16>(&device, config, data_tx)?,
                        SampleFormat::I8 => build::<i8>(&device, config, data_tx)?,
                        other => return Err(AudioError::Device(format!("unsupported sample format {other:?}"))),
                    };
                    stream.play()?;
                    Ok((stream, name, rate, channels))
                })();
                match started {
                    Ok((stream, name, rate, channels)) => {
                        let _ = ready_tx.send(Ok((name, rate, channels)));
                        let _ = stop_rx.recv(); // park until stopped or the handle is dropped
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })?;

        let (device_name, device_rate, device_channels) = ready_rx
            .recv()
            .map_err(|_| AudioError::Device("capture thread exited before starting".into()))??;
        tracing::info!(%device_name, device_rate, device_channels, "microphone capture started");
        Ok((
            Self { stop: Some(stop_tx), thread: Some(thread), device_name, device_rate, device_channels },
            data_rx,
        ))
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, tx: Sender<Vec<f32>>) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let mut resampler = Resampler::new(config.sample_rate, ENGINE_RATE);
    let stream = device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let as_f32: Vec<f32> = data.iter().map(|s| s.to_sample::<f32>()).collect();
            let mono = downmix(&as_f32, channels);
            let out = resampler.process(&mono);
            if !out.is_empty() {
                let _ = tx.send(out);
            }
        },
        |err| tracing::warn!("input stream error: {err}"),
        None,
    )?;
    Ok(stream)
}
