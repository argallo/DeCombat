//! WAV and MP3 decoding to f32 PCM.
// SPEC: AUDIO_2009.md §10 (WAV: PCM16 mono/stereo at 44100/22050/16000/11025/5000 Hz), §8.3 (MP3: MPEG-1 L3
// 44.1 kHz and MPEG-2 L3 16 kHz, optional ID3v2 tag)

use std::io::Cursor;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug)]
pub enum DecodeError {
    Wav(String),
    Mp3(String),
    Unsupported(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Wav(s) => write!(f, "wav: {s}"),
            DecodeError::Mp3(s) => write!(f, "mp3: {s}"),
            DecodeError::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}
impl std::error::Error for DecodeError {}

/// Decoded audio: interleaved f32 samples in `[-1, 1]` at the file's own rate.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }
    pub fn duration_s(&self) -> f64 {
        if self.rate == 0 {
            0.0
        } else {
            self.frames() as f64 / f64::from(self.rate)
        }
    }
    /// Frame `i` as `(left, right)`; mono is duplicated, more than two channels use the first two.
    #[inline]
    pub fn frame_lr(&self, i: usize) -> (f32, f32) {
        let c = self.channels as usize;
        let base = i * c;
        match c {
            0 => (0.0, 0.0),
            1 => (self.samples[base], self.samples[base]),
            _ => (self.samples[base], self.samples[base + 1]),
        }
    }
    /// Frame `i` averaged to mono (used by 3D voices, which are spatialised from a mono source).
    #[inline]
    pub fn frame_mono(&self, i: usize) -> f32 {
        let (l, r) = self.frame_lr(i);
        if self.channels == 1 {
            l
        } else {
            0.5 * (l + r)
        }
    }
}

/// Header facts of a WAV image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavInfo {
    pub tag_pcm: bool,
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub frames: u32,
}

/// Reads only the RIFF/WAVE header (a truncated image of a few KiB is enough).
pub fn wav_info(bytes: &[u8]) -> Result<WavInfo, DecodeError> {
    let r =
        hound::WavReader::new(Cursor::new(bytes)).map_err(|e| DecodeError::Wav(e.to_string()))?;
    let s = r.spec();
    Ok(WavInfo {
        tag_pcm: s.sample_format == hound::SampleFormat::Int,
        channels: s.channels,
        rate: s.sample_rate,
        bits: s.bits_per_sample,
        frames: r.duration(),
    })
}

/// WAVE format tags. // PUBLIC: RIFF/WAVE `fmt ` chunk (Microsoft multimedia registration)
pub mod tag {
    pub const PCM: u16 = 0x0001;
    pub const MS_ADPCM: u16 = 0x0002;
    pub const FLOAT: u16 = 0x0003;
    pub const IMA_ADPCM: u16 = 0x0011;
    pub const MPEG_L3: u16 = 0x0055;
    pub const EXTENSIBLE: u16 = 0xfffe;
}

/// The format tag of a RIFF/WAVE image's `fmt ` chunk (walks the chunk list; a truncated image of a few KiB is
/// enough). `None` when the image is not RIFF/WAVE or has no `fmt ` chunk.
pub fn wav_format_tag(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut i = 12usize;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len =
            u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        if id == b"fmt " {
            return bytes
                .get(i + 8..i + 10)
                .map(|t| u16::from_le_bytes([t[0], t[1]]));
        }
        // chunks are padded to an even length
        i = i.checked_add(8 + len + (len & 1))?;
    }
    None
}

/// Name of a format tag for messages and census lines.
pub fn tag_name(t: u16) -> &'static str {
    match t {
        tag::PCM => "PCM",
        tag::MS_ADPCM => "MS ADPCM",
        tag::FLOAT => "IEEE float",
        tag::IMA_ADPCM => "IMA ADPCM",
        tag::MPEG_L3 => "MPEG Layer III",
        tag::EXTENSIBLE => "extensible",
        _ => "other",
    }
}

/// Decodes a whole WAV image (integer PCM 8/16/24/32 bit or float, also in the extensible wrapper) to f32.
/// Compressed tags (ADPCM, MPEG) are refused with [`DecodeError::Unsupported`] naming the tag: the ROM's 1,529
/// effects are all PCM16 (SPEC §10, re-counted by the census test), so no ADPCM decoder is carried.
pub fn decode_wav(bytes: &[u8]) -> Result<Pcm, DecodeError> {
    if let Some(t) = wav_format_tag(bytes) {
        if !matches!(t, tag::PCM | tag::FLOAT | tag::EXTENSIBLE) {
            return Err(DecodeError::Unsupported(format!(
                "WAVE format tag 0x{t:04x} ({})",
                tag_name(t)
            )));
        }
    }
    let mut r =
        hound::WavReader::new(Cursor::new(bytes)).map_err(|e| DecodeError::Wav(e.to_string()))?;
    let s = r.spec();
    let samples: Vec<f32> = match (s.sample_format, s.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => r
            .samples::<i16>()
            .map(|v| v.map(|x| f32::from(x) / 32768.0))
            .collect::<Result<_, _>>(),
        (hound::SampleFormat::Int, 8) => r
            .samples::<i8>()
            .map(|v| v.map(|x| f32::from(x) / 128.0))
            .collect::<Result<_, _>>(),
        (hound::SampleFormat::Int, 24 | 32) => {
            let scale = (1u64 << (s.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|v| v.map(|x| x as f32 / scale))
                .collect::<Result<_, _>>()
        }
        (hound::SampleFormat::Float, 32) => r.samples::<f32>().collect::<Result<_, _>>(),
        (f, b) => return Err(DecodeError::Unsupported(format!("{f:?} {b} bit"))),
    }
    .map_err(|e: hound::Error| DecodeError::Wav(e.to_string()))?;
    Ok(Pcm {
        rate: s.sample_rate,
        channels: s.channels,
        samples,
    })
}

/// Facts from the first MPEG audio frame header. // PUBLIC: ISO/IEC 11172-3 and 13818-3 frame header
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mp3Header {
    /// 1 = MPEG-1, 2 = MPEG-2, 25 = MPEG-2.5.
    pub mpeg: u8,
    pub layer: u8,
    pub rate: u32,
    pub channels: u16,
    pub bitrate_kbps: u32,
    pub had_id3v2: bool,
}

const BITRATE_V1_L3: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
const BITRATE_V2_L3: [u32; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];

/// Skips an ID3v2 tag if present and parses the first valid frame header after it.
pub fn mp3_header(bytes: &[u8]) -> Result<Mp3Header, DecodeError> {
    let mut pos = 0usize;
    let mut had = false;
    if bytes.len() >= 10 && &bytes[0..3] == b"ID3" {
        let size = bytes[6..10]
            .iter()
            .fold(0usize, |a, &b| (a << 7) | (b as usize & 0x7f));
        let footer = if bytes[5] & 0x10 != 0 { 10 } else { 0 };
        pos = 10 + size + footer;
        had = true;
    }
    let end = bytes.len().min(pos + 64 * 1024);
    let mut i = pos;
    while i + 4 <= end {
        let (b0, b1, b2, b3) = (bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]);
        if b0 == 0xff && (b1 & 0xe0) == 0xe0 {
            let ver_bits = (b1 >> 3) & 3;
            let layer_bits = (b1 >> 1) & 3;
            let br_idx = (b2 >> 4) as usize;
            let sr_idx = ((b2 >> 2) & 3) as usize;
            if ver_bits != 1 && layer_bits == 1 && br_idx != 0 && br_idx != 15 && sr_idx != 3 {
                let (mpeg, rate, kbps) = match ver_bits {
                    3 => (1u8, [44100, 48000, 32000][sr_idx], BITRATE_V1_L3[br_idx]),
                    2 => (2u8, [22050, 24000, 16000][sr_idx], BITRATE_V2_L3[br_idx]),
                    _ => (25u8, [11025, 12000, 8000][sr_idx], BITRATE_V2_L3[br_idx]),
                };
                let channels = if (b3 >> 6) == 3 { 1 } else { 2 };
                return Ok(Mp3Header {
                    mpeg,
                    layer: 3,
                    rate,
                    channels,
                    bitrate_kbps: kbps,
                    had_id3v2: had,
                });
            }
        }
        i += 1;
    }
    Err(DecodeError::Mp3("no Layer III frame header found".into()))
}

/// Decodes a whole MP3 image (MPEG-1/2/2.5 Layer III, ID3v2 skipped) to f32.
pub fn decode_mp3(bytes: &[u8]) -> Result<Pcm, DecodeError> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::Mp3(e.to_string()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| DecodeError::Mp3("no audio track".into()))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| DecodeError::Mp3(e.to_string()))?;

    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(0);
    let mut samples: Vec<f32> = Vec::new();
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => break,
            Err(e) => return Err(DecodeError::Mp3(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                channels = spec.channels.count() as u16;
                if buf
                    .as_ref()
                    .map(|b| b.capacity() < decoded.capacity() * channels as usize)
                    .unwrap_or(true)
                {
                    buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
                }
                let b = buf.as_mut().expect("buffer set above");
                b.copy_interleaved_ref(decoded);
                samples.extend_from_slice(b.samples());
            }
            // A corrupt frame is skipped, as a streaming decoder would.
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(DecodeError::Mp3(e.to_string())),
        }
    }
    if rate == 0 || channels == 0 {
        return Err(DecodeError::Mp3("no decodable frames".into()));
    }
    Ok(Pcm {
        rate,
        channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_wav(rate: u32, channels: u16, frames: usize) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let spec = hound::WavSpec {
                channels,
                sample_rate: rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::new(Cursor::new(&mut out), spec).unwrap();
            for i in 0..frames {
                for _ in 0..channels {
                    w.write_sample((i as i16).wrapping_mul(100)).unwrap();
                }
            }
            w.finalize().unwrap();
        }
        out
    }

    #[test]
    fn wav_roundtrip_mono_stereo() {
        let w = synth_wav(22050, 1, 100);
        let info = wav_info(&w[..64]).unwrap_or_else(|_| wav_info(&w).unwrap());
        assert_eq!(
            (info.channels, info.rate, info.bits, info.tag_pcm),
            (1, 22050, 16, true)
        );
        let p = decode_wav(&w).unwrap();
        assert_eq!(p.frames(), 100);
        assert!((p.samples[1] - 100.0 / 32768.0).abs() < 1e-7);
        let s = decode_wav(&synth_wav(44100, 2, 10)).unwrap();
        assert_eq!((s.channels, s.frames()), (2, 10));
        assert_eq!(s.frame_lr(3), (s.samples[6], s.samples[7]));
    }

    #[test]
    fn pcm8_is_offset_binary_and_decodes_to_signed() {
        let mut out = Vec::new();
        {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 11025,
                bits_per_sample: 8,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::new(Cursor::new(&mut out), spec).unwrap();
            for v in [-128i8, -64, 0, 64, 127] {
                w.write_sample(v).unwrap();
            }
            w.finalize().unwrap();
        }
        // on disk 8-bit PCM is unsigned: silence is 0x80
        let data = out.windows(4).position(|w| w == b"data").unwrap() + 8;
        assert_eq!(&out[data..data + 5], &[0x00, 0x40, 0x80, 0xc0, 0xff]);
        assert_eq!(wav_format_tag(&out), Some(tag::PCM));
        let p = decode_wav(&out).unwrap();
        assert_eq!((p.rate, p.channels), (11025, 1));
        assert_eq!(p.samples, vec![-1.0, -0.5, 0.0, 0.5, 127.0 / 128.0]);
    }

    #[test]
    fn adpcm_tags_are_found_and_refused_by_name() {
        // RIFF/WAVE with a LIST chunk (odd length, padded) before an IMA ADPCM `fmt ` chunk
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"LIST");
        v.extend_from_slice(&[3, 0, 0, 0]);
        v.extend_from_slice(b"abc");
        v.push(0);
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&[20, 0, 0, 0]);
        v.extend_from_slice(&tag::IMA_ADPCM.to_le_bytes());
        v.extend_from_slice(&[
            1, 0, 0x22, 0x56, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 2, 0, 0xf9, 1,
        ]);
        assert_eq!(wav_format_tag(&v), Some(tag::IMA_ADPCM));
        match decode_wav(&v) {
            Err(DecodeError::Unsupported(m)) => assert!(m.contains("0x0011 (IMA ADPCM)"), "{m}"),
            other => panic!("{other:?}"),
        }
        let mut avi = b"RIFF".to_vec();
        avi.extend_from_slice(&[0, 0, 0, 0]);
        avi.extend_from_slice(b"AVI ");
        assert_eq!(wav_format_tag(&avi), None);
        assert_eq!(tag_name(tag::MS_ADPCM), "MS ADPCM");
    }

    #[test]
    fn mp3_header_layouts() {
        // MPEG-1 L3, 128 kbit/s, 44.1 kHz, joint stereo
        assert_eq!(
            mp3_header(&[0xff, 0xfb, 0x90, 0x44]).unwrap(),
            Mp3Header {
                mpeg: 1,
                layer: 3,
                rate: 44100,
                channels: 2,
                bitrate_kbps: 128,
                had_id3v2: false
            }
        );
        // MPEG-2 L3, 16 kbit/s, 16 kHz, mono, behind a 20-byte ID3v2 tag
        let mut v = b"ID3\x04\x00\x00\x00\x00\x00\x14".to_vec();
        v.extend(std::iter::repeat_n(0u8, 20));
        v.extend_from_slice(&[0xff, 0xf3, 0x28, 0xc4]);
        let h = mp3_header(&v).unwrap();
        assert_eq!(
            (h.mpeg, h.rate, h.channels, h.bitrate_kbps, h.had_id3v2),
            (2, 16000, 1, 16, true)
        );
        assert!(mp3_header(&[0u8; 100]).is_err());
    }
}
