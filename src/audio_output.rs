use chrono::Duration;
use cpal::{
    traits::{DeviceTrait as _, StreamTrait},
    Stream, StreamConfig,
};
use log::{error, info, warn};
use rb::{RbConsumer as _, RbInspector, RbProducer as _, RB as _};
use samplerate::{ConverterType, Samplerate};
use std::ops::Mul;

use crate::decode::StreamParams;

#[allow(dead_code)]
#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum AudioOutputError {
    OpenStreamError,
    PlayStreamError,
    StreamClosedError,
}

pub fn open_output(device: &cpal::Device) -> Result<Box<dyn AudioOutput>, AudioOutputError> {
    // Select proper playback routine based on sample format.
    // In theory this could be different per platform, with other outputs like
    // pipewire or something else
    Ok(Box::new(AudioOutputInner::new(device)))
}

pub fn interleave(planar: &[f32], channels: usize, output_channels: usize) -> Vec<f32> {
    let channel_len = planar.len() / channels;
    let mut interleaved = vec![0.; channel_len * output_channels];
    for (i, frame) in interleaved.chunks_exact_mut(output_channels).enumerate() {
        for (ch, s) in frame.iter_mut().enumerate() {
            match ch {
                c if c < 2 && channels == 1 => {
                    *s = match planar.get((ch * channel_len) + i) {
                        Some(v) => *v,
                        None => planar[i]
                    }
                },
                c if c < output_channels && c < channels => {
                    *s = planar[(ch * channel_len) + i];
                },
                _ => *s = 0.0
            }
        }
    }
    interleaved
}

pub trait AudioOutput {
    /// Write some samples into the buffer to be played.
    fn write(&mut self, decoded: &[f32]);

    /// Flush the remaining samples from the resampler.
    fn flush(&mut self);

    /// Call on a seek to prevent audio weirdness.
    fn seek_flush(&mut self);

    /// Set the volume (amplitude) of the output.
    fn set_volume(&mut self, vol: Volume);

    /// Get the volume (amplitude) of the output.
    fn volume(&self) -> Volume;

    fn params(&self) -> StreamConfig;

    /// Get the input stream parameters.
    fn input_params(&self) -> Option<StreamParams>;

    /// Update input stream parameters, used for internal calculations.
    fn update_input_params(&mut self, params: StreamParams);

    /// Gets the current level of the buffer in bytes.
    fn buffer_level(&self) -> usize;

    /// Gets the current level of the buffer in bytes.
    fn buffer_capacity(&self) -> usize;

    /// Prints the size in bytes of the "healthy" level of the buffer.
    fn buffer_healthy(&self) -> usize;

    /// Calculates the delay until the buffer is empty and the last sample plays.
    fn buffer_delay(&self) -> Duration {
        let samples_per_second = self.params().channels as i64 * self.params().sample_rate.0 as i64;
        let delay_microseconds = (self.buffer_level() as i64 * 1_000_000) / samples_per_second;

        Duration::microseconds(delay_microseconds)
    }

    /// Get the buffer's fullness as a percentage.
    fn buffer_percent(&self) -> f32 {
        (self.buffer_level() as f32 / self.buffer_capacity() as f32) * 100.0
    }
}

pub struct AudioOutputInner {
    ring_buf: rb::SpscRb<f32>,
    ring_buf_producer: rb::Producer<f32>,
    resampler: Option<samplerate::Samplerate>,
    volume: Volume,
    channels: u16,
    state: bool,

    input_params: Option<StreamParams>,
    output_stream: Stream,
    output_params: StreamConfig,
}

const OUTPUT_RATE_HZ: u32 = 44_100;
const CHANNELS_OUT: u16 = 2;
/// Ringbuffer buffer time in milliseconds
const RINGBUFFER_DURATION: usize = 200;

impl AudioOutputInner {
    fn new(device: &cpal::Device) -> Self {
        // Ensure that the stream has a valid output sample rate.
        // Always prefer OUTPUT_RATE_HZ, but adapt as needed.
        let mut out_hz = OUTPUT_RATE_HZ;
        let mut min = 0;
        let mut max = 0;
        for config in device.supported_output_configs().unwrap() {
            (min, max) = (config.min_sample_rate().0, config.max_sample_rate().0);
        }
        out_hz = out_hz.clamp(min, max);
        if out_hz != OUTPUT_RATE_HZ {
            warn!("output rate can't be set to {OUTPUT_RATE_HZ}, using {out_hz}")
        }

        // Ensure the stream has a valid output channel count
        let mut out_ch = CHANNELS_OUT;
        if let Ok(c) = device.default_output_config() {
            out_ch = c.channels();
        }
        if out_ch != CHANNELS_OUT {
            warn!("output channel count can't be set to {CHANNELS_OUT}, using {out_ch}")
        }

        let output_params = cpal::StreamConfig {
            channels: out_ch,
            sample_rate: cpal::SampleRate(out_hz),
            buffer_size: cpal::BufferSize::Default,
        };

        // Create a ring buffer with a capacity for up-to 200ms of audio.
        let ring_len =
            ((RINGBUFFER_DURATION * output_params.sample_rate.0 as usize) / 1000) * output_params.channels as usize;

        let ring_buf = rb::SpscRb::new(ring_len);
        let (ring_buf_producer, ring_buf_consumer) = (ring_buf.producer(), ring_buf.consumer());

        let output_stream = device
            .build_output_stream(
                &output_params,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // Write out as many samples as possible from the ring buffer to the audio
                    // output.
                    let written = ring_buf_consumer.read(data).unwrap_or(0);

                    // Mute any remaining samples.
                    data[written..].iter_mut().for_each(|s| *s = 0f32);
                },
                move |err| error!("audio output error: {}", err),
                None,
            )
            .unwrap();

        Self {
            ring_buf,
            ring_buf_producer,
            resampler: None,
            volume: Volume::default(),
            channels: out_ch,
            state: false,

            input_params: None,
            output_stream,
            output_params,
        }
    }
}

impl AudioOutput for AudioOutputInner {
    fn write(&mut self, decoded: &[f32]) {
        // Do nothing if there are no audio frames.
        if decoded.is_empty() {
            warn!("decoded length was 0");
            return;
        }

        if !self.state {
            // If the stream isn't playing, do that
            self.output_stream.play().unwrap();
            self.state = true
        }

        // Interleave samples
        let decoded = interleave(decoded, self.channels as usize, self.output_params.channels as usize);

        // Resample if resampler exists
        let mut processed_samples = if let Some(resampler) = &mut self.resampler {
            match resampler.process(&decoded) {
                Ok(resampled) => resampled,
                Err(_) => return,
            }
        } else {
            decoded
        };

        // Set the sample amplitude (volume) for every sample
        // This is obviously not necessary if the volume is not changed
        if self.volume != 1.0 {
            processed_samples
                .iter_mut()
                .for_each(|s| *s = s.mul(self.volume.as_f32()));
        }

        // Write all samples to the ring buffer.
        let mut offset = 0;
        while let Some(written) = self
            .ring_buf_producer
            .write_blocking(&processed_samples[offset..])
        {
            offset += written;
        }
    }

    fn seek_flush(&mut self) {
        if let Some(resampler) = &mut self.resampler {
            resampler.reset().unwrap();
        }

        self.ring_buf.clear();
    }

    fn flush(&mut self) {
        // If there is a resampler, then it may need to be flushed
        // depending on the number of samples it has.
        if let Some(resampler) = &mut self.resampler {
            let mut remaining_samples = resampler.process_last(&[]).unwrap_or_default();

            while let Some(written) = self.ring_buf_producer.write_blocking(&remaining_samples) {
                remaining_samples = remaining_samples[written..].to_vec();
            }
        }

        // Flush is best-effort, ignore the returned result.
        self.state = false;
    }

    fn set_volume(&mut self, vol: Volume) {
        self.volume = vol
    }

    fn volume(&self) -> Volume {
        self.volume
    }

    fn params(&self) -> StreamConfig {
        self.output_params.clone()
    }

    fn input_params(&self) -> Option<StreamParams> {
        self.input_params
    }

    fn update_input_params(&mut self, params: StreamParams) {
        // If the sample rate is not equal to the output sample rate,
        // create a resampler to correct it
        if params.rate != self.output_params.sample_rate.0 {
            let resample_ratio = params.rate as f64 / self.output_params.sample_rate.0 as f64;
            info!(
                "resampling {} Hz to {} Hz ({:0.4}), buffer size of {} bytes",
                params.rate, self.output_params.sample_rate.0, resample_ratio, params.packet_size,
            );

            // Chose samplerate conversion based on how extreme the sample ratio is
            // TODO: Make this settable manually
            let converter = match resample_ratio {
                r if r <= 2.0 => ConverterType::SincBestQuality,
                r if r <= 3.0 => ConverterType::SincMediumQuality,
                r if r <= 4.0 => ConverterType::SincFastest,
                _ => ConverterType::Linear,
            };
            info!("chose {} for sample rate conversion", converter.name());

            self.resampler = Some(
                Samplerate::new(converter, params.rate, self.output_params.sample_rate.0, 2)
                    .unwrap(),
            );
        } else {
            self.resampler = None;
        };

        self.input_params = Some(params);
        self.channels = params.channels;
    }

    fn buffer_level(&self) -> usize {
        self.ring_buf.count()
    }

    fn buffer_capacity(&self) -> usize {
        self.ring_buf.capacity()
    }

    fn buffer_healthy(&self) -> usize {
        self.ring_buf.capacity() - (self.ring_buf.capacity() / 5)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume(f32);

impl Default for Volume {
    fn default() -> Self {
        Self(1.0)
    }
}

impl Volume {
    pub fn new(vol: f32) -> Self {
        let vol = vol.clamp(0.0, 1.0);

        Volume(vol)
    }

    pub fn set(&mut self, vol: f32) {
        let vol = vol.clamp(0.0, 1.0);

        self.0 = vol
    }

    pub fn as_f32(&self) -> f32 {
        self.0
    }
}

impl std::ops::Mul<f32> for Volume {
    type Output = f32;

    fn mul(self, rhs: f32) -> Self::Output {
        self.0 * rhs
    }
}

impl PartialEq<f32> for Volume {
    fn eq(&self, other: &f32) -> bool {
        self.0 == *other
    }
}
