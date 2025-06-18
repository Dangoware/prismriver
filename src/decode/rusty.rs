use std::{collections::HashMap, fs::File, io};

use chrono::Duration;
use fluent_uri::Uri;
use log::{info, warn};
use symphonia::core::{
    audio::{SampleBuffer, SignalSpec},
    codecs::{CodecParameters, DecoderOptions, CODEC_TYPE_NULL},
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSourceStream, MediaSourceStreamOptions, ReadOnlySource},
    meta::MetadataOptions,
    probe::Hint,
    units::Time,
};

use crate::utils::uri_to_path;

use super::{Decoder, DecoderError, StreamParams};

pub struct RustyDecoder {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    sample_buf: Option<SampleBuffer<f32>>,
    track_id: u32,
    params: CodecParameters,
    timestamp: u64,
    spec: SignalSpec,
    metadata: HashMap<String, String>,
}

impl RustyDecoder {
    pub fn new(input: &Uri<String>) -> Result<Self, DecoderError> {
        if input.scheme().as_str() != "file" {
            return Err(DecoderError::InternalError("Invalid URI".to_string()));
        }

        if uri_to_path(input).is_err() {
            return Err(DecoderError::InternalError(
                "URI was not a valid path".to_string(),
            ));
        }

        let file = File::open(uri_to_path(input).unwrap()).unwrap();
        let mss = MediaSourceStream::new(
            Box::new(ReadOnlySource::new(file)),
            MediaSourceStreamOptions::default(),
        );

        let meta_opts: MetadataOptions = MetadataOptions::default();
        let fmt_opts: FormatOptions = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };

        let mut probed = symphonia::default::get_probe()
            .format(&Hint::new(), mss, &fmt_opts, &meta_opts)
            .map_err(|e| DecoderError::InternalError(e.to_string()))?;

        let mut metadata = HashMap::new();
        if let Some(mut m) = probed.metadata.get() {
            let mut new = true;
            while new {
                if let Some(rev) = m.current() {
                    for tag in rev.tags() {
                        metadata.insert(tag.key.clone(), tag.value.to_string());
                    }
                }
                new = m.pop().is_some();
            }
        }
        info!("found {} metadata tags", metadata.len());

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

        if decoder.codec_params().channels.unwrap_or_default().count() < 2 {
            warn!("mono audio will be sent in stereo to both left and right");
        }

        let track_id = track.id;
        let params = track.codec_params.clone();

        Ok(Self {
            spec: SignalSpec::new(
                decoder.codec_params().sample_rate.unwrap(),
                decoder.codec_params().channels.unwrap(),
            ),
            params,
            format_reader,
            decoder,
            track_id,
            timestamp: 0,
            sample_buf: None,
            metadata,
        })
    }
}

impl Decoder for RustyDecoder {
    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError> {
        let mut offset = 0;
        loop {
            // Get the next packet from the media format.
            let packet = match self.format_reader.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::ResetRequired) => {
                    unimplemented!();
                }
                Err(symphonia::core::errors::Error::IoError(e)) => match e.kind() {
                    io::ErrorKind::UnexpectedEof => return Err(DecoderError::EndOfStream),
                    _ => unimplemented!(),
                }
                Err(err) => {
                    panic!("{}", err);
                }
            };

            // Consume any new metadata that has been read since the last packet.
            let mut meta_count = 0;
            while !self.format_reader.metadata().is_latest() {
                // Pop the old head of the metadata queue.
                self.format_reader.metadata().pop();

                if let Some(rev) = self.format_reader.metadata().current() {
                    for tag in rev.tags() {
                        self.metadata.insert(tag.key.clone(), tag.value.to_string());
                        meta_count += 1;
                    }
                }
            }
            if meta_count > 0 {
                info!("found {meta_count} new metadata tags");
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
                        self.sample_buf = Some(SampleBuffer::new(
                            decoded.capacity() as u64,
                            *decoded.spec(),
                        ));
                    }

                    // Copy the decoded samples to a buffer (why is it this convoluted?)
                    // TODO: This is actually copying the planar samples twice, which is wrong!
                    self.sample_buf.as_mut().unwrap().copy_planar_ref(decoded);
                    let len = self.sample_buf.as_mut().unwrap().len();
                    buf[offset..offset + len]
                        .copy_from_slice(self.sample_buf.as_mut().unwrap().samples());
                    offset += len;
                }
                Err(symphonia::core::errors::Error::IoError(_)) => {
                    warn!("io error");
                    // The packet failed to decode due to an IO error, skip the packet.
                    continue;
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => {
                    warn!("invalid packet skipped");
                    // The packet failed to decode due to invalid data, skip the packet.
                    continue;
                }
                Err(err) => {
                    // An unrecoverable error occurred, halt decoding.
                    panic!("{}", err);
                }
            }

            // No loop needed, packet was successfully decoded
            break;
        }

        Ok(offset)
    }

    fn seek_absolute(&mut self, pos: Duration) -> Result<(), DecoderError> {
        self.timestamp = match self.format_reader.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(pos.to_std().unwrap()),
                track_id: Some(self.track_id),
            },
        ) {
            Ok(ts) => ts.actual_ts,
            Err(e) => return Err(DecoderError::InternalError(e.to_string())),
        };

        self.decoder.reset();

        Ok(())
    }

    fn seek_relative(&mut self, _pos: Duration) -> Result<(), DecoderError> {
        todo!()
    }

    fn position(&self) -> Option<Duration> {
        self.params
            .time_base
            .map(|t| Duration::from_std(t.calc_time(self.timestamp).into()).unwrap())
    }

    fn duration(&self) -> Option<Duration> {
        let dur = self
            .params
            .n_frames
            .map(|frames| self.params.start_ts + frames);
        if let Some(t) = self.params.time_base {
            dur.map(|d| Duration::from_std(t.calc_time(d).into()).unwrap())
        } else {
            None
        }
    }

    fn params(&self) -> StreamParams {
        StreamParams {
            rate: self.spec.rate,
            channels: self.spec.channels.count() as u16,
            packet_size: self.params.max_frames_per_packet.unwrap_or(4096),
        }
    }

    fn metadata(&self) -> HashMap<String, String> {
        self.metadata.clone()
    }
}
