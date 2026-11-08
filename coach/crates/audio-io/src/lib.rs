//! Audio in and out of the engine. Everything this crate hands over is 16 kHz mono `f32`.
//!
//! - [`capture`]: microphone stream on its own thread (cpal streams are not `Send` on macOS).
//! - [`playback`]: play a clip through the default output device.
//! - [`decode`]: WAV, MP3, M4A/AAC and FLAC files via symphonia.
//! - [`resample`]: band-limited streaming resampler and downmixing.

pub mod capture;
pub mod decode;
pub mod playback;
pub mod resample;

/// The engine's sample rate.
pub const ENGINE_RATE: u32 = 16_000;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no input device found — is a microphone connected and allowed in System Settings › Privacy › Microphone?")]
    NoInputDevice,
    #[error("no output device found")]
    NoOutputDevice,
    #[error("audio device error: {0}")]
    Device(String),
    #[error("could not read audio file: {0}")]
    Decode(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<cpal::Error> for AudioError {
    fn from(e: cpal::Error) -> Self {
        AudioError::Device(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AudioError>;

/// Write a 16 kHz mono clip as a 16-bit WAV file.
pub fn write_wav(path: &std::path::Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: ENGINE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| AudioError::Decode(e.to_string()))?;
    for &s in samples {
        writer
            .write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .map_err(|e| AudioError::Decode(e.to_string()))?;
    }
    writer.finalize().map_err(|e| AudioError::Decode(e.to_string()))
}
