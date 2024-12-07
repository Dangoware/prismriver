use cpal::{traits::{DeviceTrait, StreamTrait}, Sample as _};
use log::{error, info};
use rb::{RbConsumer as _, RbProducer as _, RB as _};
use crate::resampler::Resampler;
use symphonia::core::audio::{AudioBuffer, AudioBufferRef, SampleBuffer, Signal, SignalSpec};

#[allow(dead_code)]
#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum AudioOutputError {
    OpenStreamError,
    PlayStreamError,
    StreamClosedError,
}

pub fn open_output(device: &cpal::Device, spec: SignalSpec, duration: u64) -> Result<Box<dyn AudioOutput>, AudioOutputError> {
    let config = match device.default_output_config() {
        Ok(config) => config,
        Err(_err) => {
            return Err(AudioOutputError::OpenStreamError);
        }
    };

    // Select proper playback routine based on sample format.
    Ok(match config.sample_format() {
        cpal::SampleFormat::F32 => Box::new(AudioOutputInner::<f32>::new(&device, spec, duration)),
        cpal::SampleFormat::I16 => Box::new(AudioOutputInner::<i16>::new(&device, spec, duration)),
        cpal::SampleFormat::U16 => Box::new(AudioOutputInner::<u16>::new(&device, spec, duration)),
        cpal::SampleFormat::U8 => Box::new(AudioOutputInner::<u8>::new(&device, spec, duration)),
        e => panic!("{} is an unsupported sample format", e),
    })
}

pub trait AudioOutput {
    /// Write some samples into the buffer to be played
    fn write(&mut self, decoded: AudioBuffer<f32>) -> Result<(), ()>;

    /// Flush the remaining samples from the resampler
    fn flush(&mut self);

    /// Set the volume (amplitude) of the output
    fn set_volume(&mut self, vol: Volume);

    /// Get the volume (amplitude) of the output
    fn volume(&self) -> Volume;

    /// Get the current [`SignalSpec`]
    fn spec(&self) -> &SignalSpec;
}

pub struct AudioOutputInner<T: AudioOutputSample> {
    ring_buf: rb::SpscRb<T>,
    ring_buf_producer: rb::Producer<T>,
    sample_buf: SampleBuffer<T>,
    stream: cpal::Stream,
    resampler: Option<crate::resampler::Resampler<T>>,
    volume: Volume,
    spec: SignalSpec,
}

impl<T: AudioOutputSample> AudioOutputInner<T> {
    fn new(
        device: &cpal::Device,
        spec: SignalSpec,
        duration: u64,
    ) -> Self {
        //let num_channels = spec.channels.count();

        // Set up CPAL stuff
        let config = device
            .default_output_config()
            .expect("Failed to get the default output config.")
            .config();

        // Create a ring buffer with a capacity for up-to 200ms of audio.
        let ring_len = ((200 * config.sample_rate.0 as usize) / 1000) * spec.channels.count();

        let ring_buf = rb::SpscRb::new(ring_len);
        let (ring_buf_producer, ring_buf_consumer) = (ring_buf.producer(), ring_buf.consumer());

        let stream_result = device.build_output_stream(
            &config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Write out as many samples as possible from the ring buffer to the audio
                // output.
                let written = ring_buf_consumer.read(data).unwrap_or(0);

                // Mute any remaining samples.
                data[written..].iter_mut().for_each(|s| *s = T::MID);
            },
            move |err| error!("audio output error: {}", err), None,
        ).unwrap();

        let sample_buf = SampleBuffer::<T>::new(duration, spec);

        let resampler = if spec.rate != config.sample_rate.0 {
            info!("resampling {} Hz to {} Hz", spec.rate, config.sample_rate.0);
            Some(Resampler::new(spec, config.sample_rate.0 as usize, duration))
        } else {
            None
        };

        Self {
            ring_buf,
            ring_buf_producer,
            sample_buf,
            stream: stream_result,
            resampler,
            volume: Volume::default(),
            spec,
        }
    }
}

impl<T: AudioOutputSample> AudioOutput for AudioOutputInner<T> {
    fn write(&mut self, decoded: AudioBuffer<f32>) -> Result<(), ()> {
        // Do nothing if there are no audio frames.
        if decoded.frames() == 0 {
            return Ok(());
        }

        let samples = if let Some(resampler) = &mut self.resampler {
            // Resampling is required. The resampler will return interleaved samples in the
            // correct sample format.
            match resampler.resample(decoded) {
                Some(resampled) => resampled,
                None => return Ok(()),
            }
        } else {
            // Resampling is not required. Interleave the sample for cpal using a sample buffer.
            self.sample_buf.copy_interleaved_typed(&decoded);

            self.sample_buf.samples()
        };

        // Set the sample amplitude (volume) for every sample
        let mut test: Vec<T> = samples.iter().map(|s| s.mul_amp(self.volume.as_f32().to_sample())).collect();

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
            let mut remaining_samples = resampler.flush().unwrap_or_default();

            while let Some(written) = self.ring_buf_producer.write_blocking(remaining_samples) {
                remaining_samples = &remaining_samples[written..];
            }
        }

        // Flush is best-effort, ignore the returned result.
        let _ = self.stream.pause();
    }

    fn set_volume(&mut self, vol: Volume) {
        self.volume = vol
    }

    fn volume(&self) -> Volume {
        self.volume
    }

    fn spec(&self) -> &SignalSpec {
        &self.spec
    }
}

pub trait AudioOutputSample:
    cpal::Sample + symphonia::core::conv::ConvertibleSample + symphonia::core::conv::IntoSample<f32> + symphonia::core::audio::RawSample + std::marker::Send + 'static
    + cpal::SizedSample + std::fmt::Debug
{
}

impl AudioOutputSample for f32 {}
impl AudioOutputSample for i16 {}
impl AudioOutputSample for u16 {}
impl AudioOutputSample for u8 {}

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
