//! Output device through cpal (feature `device`; Windows WASAPI shared mode, macOS CoreAudio, Linux ALSA).
//!
//! The mixer is shared behind a mutex and rendered from the device callback (pull model: the voice starts the game
//! thread asks for are heard at the next callback, one device period later). The device runs at 44.1 kHz when it
//! offers that rate for its own channel count and sample format, otherwise at its own rate (WASAPI shared mode:
//! the system mix rate, often 48 kHz); the mixer then converts every voice from its file rate straight to the
//! device rate ([`Mixer::set_output_rate`]). f32, i16 and u16 device formats are written; the stereo mix goes to
//! the OS speaker layout ([`spread_stereo`]). Each callback feeds [`OutputStats`] (period, latency, late callbacks
//! = probable underruns, lock waits, clipping, errors) for the log line.
// SPEC: AUDIO_2009.md §2 (44.1 kHz, 16-bit, OS speaker layout), §12 item 1

use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::mixer::Mixer;
use crate::output::{f32_to_i16, f32_to_u16, spread_stereo, OutputStats};
use crate::OUTPUT_RATE;

/// How the device is opened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceOptions {
    /// Linear gain after the mixer (1.0: the 2009 levels unchanged). Device smoke tests use a very low gain.
    pub gain: f32,
    /// Rate asked for first (the 2009 drivers' 44.1 kHz).
    pub prefer_rate: u32,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            gain: 1.0,
            prefer_rate: OUTPUT_RATE,
        }
    }
}

/// An open output stream that renders a shared mixer.
pub struct AudioDevice {
    pub mixer: Arc<Mutex<Mixer>>,
    stats: Arc<Mutex<OutputStats>>,
    stream: Option<cpal::Stream>,
    /// Device name, rate, channels and sample format, for the log.
    pub name: String,
    pub rate: u32,
    pub channels: u16,
    pub format: String,
    pub opened: Instant,
}

fn lock_stats(s: &Mutex<OutputStats>) -> std::sync::MutexGuard<'_, OutputStats> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

impl AudioDevice {
    /// Opens the default output device and starts rendering `mixer` into it.
    pub fn open(mixer: Arc<Mutex<Mixer>>, opts: &DeviceOptions) -> Result<Self, String> {
        let host = cpal::default_host();
        let dev = host
            .default_output_device()
            .ok_or("no default output device")?;
        let name = dev.name().unwrap_or_else(|_| "?".into());
        let default = dev.default_output_config().map_err(|e| e.to_string())?;
        // Prefer 44.1 kHz in the device's own channel count and sample format; else the device's default.
        // macOS keeps the device's default rate: there cpal sets the device's nominal rate system-wide (Mac outputs
        // default to 48 kHz), which changes other applications' audio and fails while another holds the device; the
        // mixer resamples to any output rate (`set_output_rate`).
        // DC-DEFAULT: AUDIO-4 (proposed)
        let cfg = if cfg!(target_os = "macos") {
            default
        } else {
            dev.supported_output_configs()
                .ok()
                .and_then(|mut it| {
                    it.find(|c| {
                        c.channels() == default.channels()
                            && c.sample_format() == default.sample_format()
                            && c.min_sample_rate().0 <= opts.prefer_rate
                            && opts.prefer_rate <= c.max_sample_rate().0
                    })
                })
                .map(|c| c.with_sample_rate(cpal::SampleRate(opts.prefer_rate)))
                .unwrap_or(default)
        };
        let rate = cfg.sample_rate().0;
        let channels = cfg.channels();
        let format = cfg.sample_format();
        {
            let mut m = dc_hangdump::crumb_wait!("audio: lock the mixer (open)", mixer.lock())
                .unwrap_or_else(|e| e.into_inner());
            m.set_output_rate(rate);
        }
        let stats = Arc::new(Mutex::new(OutputStats::new(rate)));
        let opened = Instant::now();
        let config: cpal::StreamConfig = cfg.config();
        let stream = match format {
            cpal::SampleFormat::F32 => {
                build::<f32>(&dev, &config, &mixer, &stats, opened, opts.gain, |s| s)
            }
            cpal::SampleFormat::I16 => {
                build::<i16>(&dev, &config, &mixer, &stats, opened, opts.gain, f32_to_i16)
            }
            cpal::SampleFormat::U16 => {
                build::<u16>(&dev, &config, &mixer, &stats, opened, opts.gain, f32_to_u16)
            }
            other => return Err(format!("device sample format {other:?} not written")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            mixer,
            stats,
            stream: Some(stream),
            name,
            rate,
            channels,
            format: format!("{format:?}"),
            opened,
        })
    }

    /// `"<name>" <rate> Hz <channels> ch <format>`.
    pub fn describe(&self) -> String {
        format!(
            "\"{}\" {} Hz {} ch {}",
            self.name, self.rate, self.channels, self.format
        )
    }

    /// A copy of the callback statistics.
    pub fn stats(&self) -> OutputStats {
        lock_stats(&self.stats).clone()
    }

    /// The statistics as one log line.
    pub fn line(&self) -> String {
        self.stats().line(&self.describe())
    }

    /// Stops the stream (no callback runs after this returns) and gives the final log line.
    pub fn close(mut self) -> String {
        self.stop();
        self.line()
    }

    fn stop(&mut self) {
        if let Some(s) = self.stream.take() {
            let _ = s.pause();
            drop(s);
        }
    }
}

impl Drop for AudioDevice {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build<T>(
    dev: &cpal::Device,
    config: &cpal::StreamConfig,
    mixer: &Arc<Mutex<Mixer>>,
    stats: &Arc<Mutex<OutputStats>>,
    opened: Instant,
    gain: f32,
    conv: fn(f32) -> T,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + Send + 'static,
{
    let channels = usize::from(config.channels.max(1));
    let shared = mixer.clone();
    let st = stats.clone();
    let st_err = stats.clone();
    let mut lr: Vec<f32> = Vec::new();
    dev.build_output_stream(
        config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let t = opened.elapsed().as_secs_f64();
            let frames = data.len() / channels;
            if lr.len() < 2 * frames {
                lr.resize(2 * frames, 0.0);
            }
            let block = &mut lr[..2 * frames];
            let before = Instant::now();
            let silent =
                match dc_hangdump::crumb_wait!("audio callback: lock the mixer", shared.lock()) {
                    Ok(mut m) => {
                        let wait = before.elapsed().as_secs_f64();
                        m.render(block, frames);
                        Some(wait)
                    }
                    Err(_) => None,
                };
            if silent.is_none() {
                block.fill(0.0);
            }
            spread_stereo(block, data, channels, gain, conv);
            let ts = info.timestamp();
            let latency = ts
                .playback
                .duration_since(&ts.callback)
                .map(|d| d.as_secs_f64());
            let mut s = lock_stats(&st);
            s.on_callback(t, frames as u32, latency, silent.unwrap_or(0.0));
            if silent.is_none() {
                s.silent_callbacks += 1;
            }
            s.on_block(block);
        },
        move |e| lock_stats(&st_err).on_error(e.to_string()),
        None,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Pcm;
    use crate::mixer::PlayRequest;

    /// Opens the real default device: only with `DC_AUDIO_DEVICE_TEST=1` (off in CI-style runs). Plays a 440 Hz tone
    /// for 0.3 s at a gain of 0.02 (inaudible to barely audible), then checks the callbacks ran and closes.
    #[test]
    fn device_smoke_low_gain_when_enabled() {
        if std::env::var("DC_AUDIO_DEVICE_TEST").ok().as_deref() != Some("1") {
            eprintln!("device smoke skipped (set DC_AUDIO_DEVICE_TEST=1)");
            return;
        }
        let mixer = Arc::new(Mutex::new(Mixer::new()));
        let dev = match AudioDevice::open(
            mixer.clone(),
            &DeviceOptions {
                gain: 0.02,
                ..DeviceOptions::default()
            },
        ) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("device smoke: no device ({e})");
                return;
            }
        };
        let rate = 44_100u32;
        let tone: Vec<f32> = (0..rate as usize / 2)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5)
            .collect();
        mixer
            .lock()
            .unwrap()
            .play(PlayRequest::local(Arc::new(Pcm {
                rate,
                channels: 1,
                samples: tone,
            })))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let line = dev.close();
        eprintln!("{line}");
        assert!(line.contains("callbacks"));
    }
}
