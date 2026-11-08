//! Play a 16 kHz mono clip through the default output device.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use crossbeam_channel::{bounded, Sender};

use crate::resample::resample;
use crate::{AudioError, Result, ENGINE_RATE};

/// A playing clip. Dropping it stops playback.
pub struct Playback {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    position: Arc<AtomicUsize>,
    finished: Arc<AtomicBool>,
    total: usize,
    device_rate: u32,
}

impl Playback {
    /// Start playing `samples` (16 kHz mono).
    pub fn start(samples: &[f32]) -> Result<Self> {
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let (ready_tx, ready_rx) = bounded::<Result<(u32, usize)>>(1);
        let position = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let clip = samples.to_vec();
        let (pos, fin) = (position.clone(), finished.clone());

        let thread = std::thread::Builder::new().name("playback".into()).spawn(move || {
            let started = (|| -> Result<(cpal::Stream, u32, usize)> {
                let host = cpal::default_host();
                let device = host.default_output_device().ok_or(AudioError::NoOutputDevice)?;
                let supported = device.default_output_config()?;
                let rate = supported.sample_rate();
                let format = supported.sample_format();
                let config: cpal::StreamConfig = supported.into();
                let data = Arc::new(resample(&clip, ENGINE_RATE, rate));
                let total = data.len();
                let stream = match format {
                    SampleFormat::F32 => build::<f32>(&device, config, data, pos, fin)?,
                    SampleFormat::I16 => build::<i16>(&device, config, data, pos, fin)?,
                    SampleFormat::I32 => build::<i32>(&device, config, data, pos, fin)?,
                    SampleFormat::U16 => build::<u16>(&device, config, data, pos, fin)?,
                    other => return Err(AudioError::Device(format!("unsupported sample format {other:?}"))),
                };
                stream.play()?;
                Ok((stream, rate, total))
            })();
            match started {
                Ok((stream, rate, total)) => {
                    let _ = ready_tx.send(Ok((rate, total)));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })?;

        let (device_rate, total) =
            ready_rx.recv().map_err(|_| AudioError::Device("playback thread exited early".into()))??;
        Ok(Self { stop: Some(stop_tx), thread: Some(thread), position, finished, total, device_rate })
    }

    /// A cheap, clonable view of progress for a watcher thread.
    pub fn monitor(&self) -> PlaybackMonitor {
        PlaybackMonitor {
            position: self.position.clone(),
            finished: self.finished.clone(),
            total: self.total,
            device_rate: self.device_rate,
        }
    }

    pub fn duration_s(&self) -> f32 {
        self.total as f32 / self.device_rate as f32
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    /// Seconds played so far.
    pub fn position_s(&self) -> f32 {
        self.position.load(Ordering::Relaxed).min(self.total) as f32 / self.device_rate as f32
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

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    data: Arc<Vec<f32>>,
    position: Arc<AtomicUsize>,
    finished: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let stream = device.build_output_stream::<T, _, _>(
        config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut pos = position.load(Ordering::Relaxed);
            for frame in out.chunks_mut(channels) {
                let s = data.get(pos).copied().unwrap_or(0.0);
                frame.iter_mut().for_each(|o| *o = s.to_sample::<T>());
                pos += 1;
            }
            position.store(pos, Ordering::Relaxed);
            if pos >= data.len() {
                finished.store(true, Ordering::Relaxed);
            }
        },
        |err| tracing::warn!("output stream error: {err}"),
        None,
    )?;
    Ok(stream)
}

#[derive(Debug, Clone)]
pub struct PlaybackMonitor {
    position: Arc<AtomicUsize>,
    finished: Arc<AtomicBool>,
    total: usize,
    device_rate: u32,
}

impl PlaybackMonitor {
    pub fn position_s(&self) -> f32 {
        self.position.load(Ordering::Relaxed).min(self.total) as f32 / self.device_rate as f32
    }
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }
}
