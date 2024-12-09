use std::ops::Mul;

use cpal::{traits::{DeviceTrait as _, StreamTrait}, Stream, StreamConfig};
use log::{error, info, warn};
use rb::{RbConsumer as _, RbInspector, RbProducer as _, RB as _};
use samplerate::Samplerate;
use crate::BUFFER_MAX;
use symphonia::core::{audio::SignalSpec, sample::Sample};

#[allow(dead_code)]
#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum AudioOutputError {
    OpenStreamError,
    PlayStreamError,
    StreamClosedError,
}

pub fn open_output(device: &cpal::Device) -> Result<Box<dyn AudioOutput>, AudioOutputError> {
    /* This is performed inside the trait struct now

    let config = match device.default_output_config() {
        Ok(config) => config,
        Err(_err) => {
            return Err(AudioOutputError::OpenStreamError);
        }
    };
    */

    // Select proper playback routine based on sample format.
    Ok(Box::new(AudioOutputInner::new(&device)))
}

pub trait AudioOutput {
    /// Write some samples into the buffer to be played
    fn write(&mut self, decoded: &[f32]) -> Result<(), ()>;

    /// Flush the remaining samples from the resampler
    fn flush(&mut self);

    /// Set the volume (amplitude) of the output
    fn set_volume(&mut self, vol: Volume);

    /// Get the volume (amplitude) of the output
    fn volume(&self) -> Volume;

    fn update_signalspec(&mut self, spec: SignalSpec, duration: u64);

    fn buffer_level(&self) -> (usize, usize);
    fn buffer_healthy(&self) -> usize;
}

pub struct AudioOutputInner {
    ring_buf: rb::SpscRb<f32>,
    ring_buf_producer: rb::Producer<f32>,
    resampler: Option<samplerate::Samplerate>,
    volume: Volume,
    channels: usize,
    state: bool,

    output_stream: Stream,
    output_params: StreamConfig,

    scratch_buffer: Vec<f32>,
}

const OUTPUT_RATE_HZ: usize = 44100;
const OUTPUT_COUNT: usize = OUTPUT_RATE_HZ / 100;

impl AudioOutputInner {
    fn new(
        device: &cpal::Device,
    ) -> Self {
        let output_params = if cfg!(not(target_os = "windows")) {
            cpal::StreamConfig {
                channels: 2,
                sample_rate: cpal::SampleRate(44100),
                buffer_size: cpal::BufferSize::Default,
            }
        }
        else {
            // Use the default config for Windows.
            device
                .default_output_config()
                .expect("Failed to get the default output config.")
                .config()
        };

        // Create a ring buffer with a capacity for up-to 200ms of audio.
        let ring_len = ((200 * output_params.sample_rate.0 as usize) / 1000) * output_params.channels as usize;

        let ring_buf = rb::SpscRb::new(ring_len);
        let (ring_buf_producer, ring_buf_consumer) = (ring_buf.producer(), ring_buf.consumer());

        let output_stream = device.build_output_stream(
            &output_params,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                // Write out as many samples as possible from the ring buffer to the audio
                // output.
                let written = ring_buf_consumer.read(data).unwrap_or(0);

                // Mute any remaining samples.
                data[written..].iter_mut().for_each(|s| *s = f32::MID);
            },
            move |err| error!("audio output error: {}", err),
            None,
        ).unwrap();

        Self {
            ring_buf,
            ring_buf_producer,
            resampler: None,
            volume: Volume::default(),
            channels: 2,
            state: false,

            output_stream,
            output_params,

            scratch_buffer: vec![0f32; BUFFER_MAX as usize],
        }
    }
}

impl AudioOutput for AudioOutputInner {
    fn write(&mut self, decoded: &[f32]) -> Result<(), ()> {
        // Do nothing if there are no audio frames.
        if decoded.len() == 0 {
            warn!("decoded length was 0");
            return Ok(());
        }

        if !self.state {
            self.output_stream.play().unwrap();
            self.state = true
        }

        // Interleave samples
        let mut interleaved = vec![0.; (decoded.len() / self.channels) * 2];
        for (i, frame) in interleaved.chunks_exact_mut(2).enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                *s = match decoded.get((ch * decoded.len() / self.channels) + i) {
                    Some(s) => *s,
                    None => decoded[i],
                }
            }
        }

        let samples = if let Some(resampler) = &mut self.resampler {
            // Resampling is required. The resampler will return interleaved samples in the
            // correct sample format.
            match resampler.process(&interleaved) {
                Ok(resampled) => resampled,
                Err(_) => return Ok(()),
            }
        } else {
            interleaved.to_vec()
        };

        // Set the sample amplitude (volume) for every sample
        let mut test: Vec<f32> = samples.iter().map(|s| s.mul(self.volume.as_f32())).collect();

        //info!("{} samples to a buffer with a capacity for {}", samples.len(), self.ring_buf.capacity());

        // Write all samples to the ring buffer.
        while let Some(written) = self.ring_buf_producer.write_blocking(&test) {
            test = test[written..].to_vec();
        }

        Ok(())
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

    fn update_signalspec(&mut self, spec: SignalSpec, duration: u64) {
        // If the sample rate is not equal to the output sample rate,
        // create a resampler to correct it
        if spec.rate != self.output_params.sample_rate.0 {
            info!(
                "resampling {} Hz to {} Hz ({:0.4}), buffer size of {} bytes",
                spec.rate,
                self.output_params.sample_rate.0,
                spec.rate as f64 / self.output_params.sample_rate.0 as f64,
                duration,
            );

            self.resampler = Some(Samplerate::new(
                samplerate::ConverterType::SincMediumQuality,
                spec.rate,
                self.output_params.sample_rate.0,
                2,
            ).unwrap());
        } else {
            self.resampler = None;
        };

        self.channels = spec.channels.count()
    }

    fn buffer_level(&self) -> (usize, usize) {
        (self.ring_buf.count(), self.ring_buf.capacity())
    }

    fn buffer_healthy(&self) -> usize {
        self.ring_buf.capacity() - (self.ring_buf.capacity() / 3)
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
