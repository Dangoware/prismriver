use std::time::Duration;

use symphonia::core::{audio::{SampleBuffer, SignalSpec}, codecs::{CodecParameters, DecoderOptions, CODEC_TYPE_NULL}, formats::{FormatOptions, FormatReader, SeekMode, SeekTo}, io::MediaSourceStream, meta::MetadataOptions, probe::Hint, units::Time};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DecoderError {
    #[error("Internal decoder error {}", 0)]
    InternalError(String),

    #[error("Failed to decode packet")]
    DecodeFailed,

    #[error("End of stream")]
    EndOfStream,

    #[error("No timebase, cannot calculate time")]
    NoTimebase,
}

pub trait Decoder {
    fn new(input: MediaSourceStream) -> Result<Self, DecoderError> where Self: Sized;

    fn seek(&mut self, pos: Duration) -> Result<(), DecoderError>;

    fn position(&self) -> Result<Duration, DecoderError>;
    fn duration(&self) -> Result<Duration, DecoderError>;

    /// Write the decoder's audio bytes into the provided buffer, and return the
    /// number of bytes written
    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError>;

    fn spec(&self) -> SignalSpec;
    fn max_packet_size(&self) -> u64;
}

pub struct SymphoniaDecoder {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    sample_buf: Option<SampleBuffer<f32>>,
    track_id: u32,
    params: CodecParameters,
    timestamp: u64,
    spec: SignalSpec,
}

impl Decoder for SymphoniaDecoder {
    fn new(input: MediaSourceStream) -> Result<Self, DecoderError> {
        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };

        let probed = symphonia::default::get_probe()
            .format(&Hint::new(), input, &fmt_opts, &meta_opts)
            .map_err(|e| DecoderError::InternalError(e.to_string()))?;
        let format_reader = probed.format;

        let track = format_reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .expect("no supported audio tracks");

        let dec_opts: DecoderOptions = Default::default();
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &dec_opts)
            .expect("unsupported codec");

        let track_id = track.id;
        let params = track.codec_params.clone();

        Ok(Self {
            spec: SignalSpec::new(
                decoder.codec_params().sample_rate.unwrap(),
                decoder.codec_params().channels.unwrap()
            ),
            params,
            format_reader,
            decoder,
            track_id,
            timestamp: 0,
            sample_buf: None,
        })
    }

    fn seek(&mut self, pos: Duration) -> Result<(), DecoderError> {
        self.timestamp = match self.format_reader.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(pos),
                track_id: Some(self.track_id),
            }
        ) {
            Ok(ts) => ts.actual_ts,
            Err(e) => return Err(DecoderError::InternalError(e.to_string())),
        };

        self.decoder.reset();

        Ok(())
    }

    fn position(&self) -> Result<Duration, DecoderError> {
        if let Some(t) = self.params.time_base {
            Ok(t.calc_time(self.timestamp).into())
        } else {
            Err(DecoderError::NoTimebase)
        }
    }

    fn duration(&self) -> Result<Duration, DecoderError> {
        let dur = self.params.n_frames.map(|frames| self.params.start_ts + frames);
        if let Some(t) = self.params.time_base {
            if let Some(d) = dur {
                Ok(t.calc_time(d).into())
            } else {
                Err(DecoderError::NoTimebase)
            }
        } else {
            Err(DecoderError::NoTimebase)
        }
    }

    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError> {
        let mut offset = 0;
        loop {
            // Get the next packet from the media format.
            let packet = match self.format_reader.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::ResetRequired) => {
                    unimplemented!();
                }
                Err(symphonia::core::errors::Error::IoError(_)) => {
                    return Err(DecoderError::EndOfStream)
                }
                Err(err) => {
                    panic!("{}", err);
                }
            };

            // Consume any new metadata that has been read since the last packet.
            while !self.format_reader.metadata().is_latest() {
                // Pop the old head of the metadata queue.
                let metadata = self.format_reader.metadata().pop();
                dbg!(metadata);
            }

            // If the packet does not belong to the selected track, skip over it.
            if packet.track_id() != self.track_id {
                continue;
            }

            self.timestamp = packet.ts;

            // Decode the packet into audio samples.
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.spec = *decoded.spec();

                    if self.sample_buf.is_none() {
                        self.sample_buf = Some(SampleBuffer::new(decoded.capacity() as u64, *decoded.spec()));
                    }

                    // Copy the decoded samples to a buffer (why is it this convoluted?)
                    self.sample_buf.as_mut().unwrap().copy_planar_ref(decoded);
                    let len = self.sample_buf.as_mut().unwrap().len();
                    buf[offset..offset + len].copy_from_slice(self.sample_buf.as_mut().unwrap().samples());
                    offset += len;
                }
                Err(symphonia::core::errors::Error::IoError(_)) => {
                    // The packet failed to decode due to an IO error, skip the packet.
                    continue;
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => {
                    // The packet failed to decode due to invalid data, skip the packet.
                    continue;
                }
                Err(err) => {
                    // An unrecoverable error occurred, halt decoding.
                    panic!("{}", err);
                }
            }

            // No loop needed, packet was successfully decoded
            break
        }

        Ok(offset)
    }

    fn spec(&self) -> SignalSpec {
        self.spec
    }

    fn max_packet_size(&self) -> u64 {
        self.params.max_frames_per_packet.unwrap_or(4096)
    }
}
