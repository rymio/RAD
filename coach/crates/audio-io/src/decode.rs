//! Decode audio files (WAV, MP3, M4A/AAC, FLAC) to 16 kHz mono `f32`.

use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::resample::{downmix, resample};
use crate::{AudioError, Result, ENGINE_RATE};

/// Decode a file to 16 kHz mono, peak-normalised to −1 dBFS if it is quieter.
pub fn decode_file(path: &Path) -> Result<Vec<f32>> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| AudioError::Decode(e.to_string()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| AudioError::Decode("no audio track".into()))?;
    let track_id = track.id;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AudioError::Decode(e.to_string()))?;

    let mut mono: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                buf.copy_interleaved_ref(decoded);
                mono.extend(downmix(buf.samples(), spec.channels.count()));
            }
            Err(SymError::DecodeError(e)) => tracing::debug!("skipping bad packet: {e}"),
            Err(e) => return Err(AudioError::Decode(e.to_string())),
        }
    }
    if rate == 0 || mono.is_empty() {
        return Err(AudioError::Decode("file contains no audio".into()));
    }
    let mut out = resample(&mono, rate, ENGINE_RATE);
    normalise(&mut out);
    Ok(out)
}

/// Raise quiet audio so peaks reach −1 dBFS. Never attenuates or clips.
pub fn normalise(samples: &mut [f32]) {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let target = 0.891; // −1 dBFS
    if peak > 1e-4 && peak < target {
        let gain = target / peak;
        samples.iter_mut().for_each(|s| *s *= gain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_44k_stereo_wav() {
        let dir = std::env::temp_dir().join(format!("aac-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..44_100 {
            let s = ((2.0 * std::f32::consts::PI * 220.0 * i as f32 / 44_100.0).sin() * 0.25 * i16::MAX as f32) as i16;
            w.write_sample(s).unwrap();
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();

        let audio = decode_file(&path).unwrap();
        assert_eq!(audio.len(), 16_000);
        let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.891).abs() < 0.02, "peak {peak}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn round_trips_engine_wav() {
        let dir = std::env::temp_dir().join(format!("aac-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.wav");
        let clip: Vec<f32> = (0..8000).map(|i| (i as f32 * 0.05).sin() * 0.9).collect();
        crate::write_wav(&path, &clip).unwrap();
        let back = decode_file(&path).unwrap();
        assert_eq!(back.len(), clip.len());
        assert!(back.iter().zip(&clip).all(|(a, b)| (a - b).abs() < 0.01));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(decode_file(Path::new("/definitely/not/here.wav")).is_err());
    }
}
