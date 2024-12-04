use std::fs::File;

use cpal::traits::HostTrait as _;
use cpal::Device;
use symphonia::core::{codecs::{DecoderOptions, CODEC_TYPE_NULL}, formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint};
use crate::audio_output::{open_output, AudioOutput};

pub struct RustPlayer {
    uri: Option<String>,
    uri_next: Option<String>,
    audio_output: Option<Box<dyn AudioOutput>>,
    output_device: Option<Device>,
}

impl RustPlayer {
    pub fn new() -> RustPlayer {
        let host = cpal::default_host();
        let device = host.default_output_device().expect("no output device available");

        Self {
            uri: None,
            uri_next: None,
            audio_output: None,
            output_device: Some(device),
        }
    }

    pub fn set_uri(&mut self, uri: &str) {
        self.uri = Some(uri.to_string());
        let file = File::open(self.uri.as_ref().unwrap()).unwrap();

        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };

        let probed = symphonia::default::get_probe()
            .format(&Hint::new(), mss, &fmt_opts, &meta_opts)
            .expect("unsupported format");

        let mut format = probed.format;

        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .expect("no supported audio tracks");

        let dec_opts: DecoderOptions = Default::default();
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &dec_opts)
            .expect("unsupported codec");

        let track_id = track.id;

        loop {
            // Get the next packet from the media format.
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::ResetRequired) => {
                    unimplemented!();
                }
                Err(symphonia::core::errors::Error::IoError(_)) => {
                    println!("End of stream");
                    break
                }
                Err(err) => {
                    panic!("{}", err);
                }
            };

            // Consume any new metadata that has been read since the last packet.
            while !format.metadata().is_latest() {
                // Pop the old head of the metadata queue.
                format.metadata().pop();
            }

            // If the packet does not belong to the selected track, skip over it.
            if packet.track_id() != track_id {
                continue;
            }


            // Decode the packet into audio samples.
            match decoder.decode(&packet) {
                Ok(decoded) => {
                    if self.audio_output.is_none() {
                        let spec = *decoded.spec();
                        let duration = decoded.capacity() as u64;
                        self.audio_output.replace(open_output(self.output_device.as_ref().unwrap(), spec, duration).unwrap());
                    } else {
                        // Check stuff
                    }

                    if let Some(audio_output) = self.audio_output.as_mut() {
                        audio_output.write(decoded).unwrap()
                    }
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
        }

        if let Some(out) = self.audio_output.as_mut() {
            out.flush();
        }
    }
}
