//! Audio file decoding via `symphonia` (pure Rust, no system libraries).
//! Decodes any supported file to mono `f32` PCM samples plus its sample
//! rate.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decodes the audio file at `path` to mono `f32` samples (channels
/// averaged) and its sample rate.
pub fn decode_audio_file(path: &Path) -> Result<(Vec<f32>, u32), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions {
            // Trim encoder delay/padding (e.g. from LAME's Xing/Info header)
            // so sample positions line up with the original source audio
            // rather than the lossy file's padded timeline. Without this,
            // detected beat offsets on MP3s are shifted late by the encoder
            // delay (typically 25-50ms).
            enable_gapless: true,
            ..Default::default()
        },
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    let track_id = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no supported audio track found")?
        .id;

    let track = format.tracks().iter().find(|t| t.id == track_id).unwrap();
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut sample_rate = 0u32;
    let mut mono_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(Box::new(e)),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue, // skip bad packets, matching typical decoder tolerance
            Err(e) => return Err(Box::new(e)),
        };

        let spec = *decoded.spec();
        sample_rate = spec.rate;
        let channels = spec.channels.count().max(1);

        append_mono(&decoded, channels, &mut mono_samples);
    }

    Ok((mono_samples, sample_rate))
}

/// Appends the channel-averaged mono samples of `decoded` to `out`.
fn append_mono(decoded: &AudioBufferRef, channels: usize, out: &mut Vec<f32>) {
    let mut planar = vec![Vec::<f32>::new(); channels];
    match decoded {
        AudioBufferRef::F32(buf) => {
            for (ch, p) in planar.iter_mut().enumerate() {
                *p = buf.chan(ch).to_vec();
            }
        }
        AudioBufferRef::S16(buf) => {
            for (ch, p) in planar.iter_mut().enumerate() {
                *p = buf
                    .chan(ch)
                    .iter()
                    .map(|&s| s as f32 / i16::MAX as f32)
                    .collect();
            }
        }
        AudioBufferRef::S32(buf) => {
            for (ch, p) in planar.iter_mut().enumerate() {
                *p = buf
                    .chan(ch)
                    .iter()
                    .map(|&s| s as f32 / i32::MAX as f32)
                    .collect();
            }
        }
        AudioBufferRef::U8(buf) => {
            for (ch, p) in planar.iter_mut().enumerate() {
                *p = buf
                    .chan(ch)
                    .iter()
                    .map(|&s| (s as f32 - 128.0) / 128.0)
                    .collect();
            }
        }
        // Other sample formats (S24, U16, U24, U32, F64, etc.) are rare in
        // practice for the file types this CLI targets; converting via a
        // planar copy through symphonia's generic `SampleBuffer` covers
        // them uniformly instead of matching every variant by hand.
        other => {
            use symphonia::core::audio::SampleBuffer;
            let spec = *other.spec();
            let duration = other.capacity() as u64;
            let mut buf = SampleBuffer::<f32>::new(duration, spec);
            buf.copy_interleaved_ref(other.clone());
            let interleaved = buf.samples();
            let frames = interleaved.len() / channels;
            for (ch, p) in planar.iter_mut().enumerate() {
                *p = (0..frames)
                    .map(|f| interleaved[f * channels + ch])
                    .collect();
            }
        }
    }

    let frames = planar[0].len();
    out.reserve(frames);
    for f in 0..frames {
        let sum: f32 = planar.iter().map(|p| p[f]).sum();
        out.push(sum / channels as f32);
    }
}
