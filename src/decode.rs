use std::{fs::File, thread, time::{Duration, Instant}};

use cpal::traits::HostTrait as _;
use cpal::Device;
use crossbeam::channel::{self, Receiver};
use log::{info, warn};
use symphonia::core::{audio::{SampleBuffer, SignalSpec}, codecs::{DecoderOptions, CODEC_TYPE_NULL}, formats::{FormatOptions, FormatReader}, io::{MediaSourceStream, MediaSourceStreamOptions}, meta::MetadataOptions, probe::Hint};
use thiserror::Error;
use crate::audio_output::{open_output, AudioOutput, Volume};

#[derive(Debug, Clone, Copy)]
pub enum Command {
    Play,
    Pause,
    Stop,
    Seek(Duration),
}

pub enum InternalMessage {
    /// Set a volume level
    Volume(Volume),

    /// Set up the thread with a new stream
    LoadNew(MediaSourceStream),

    /// Give the thread a new output device
    NewOutputDevice(Device),

    /// Call once to kill the receiving thread
    Destroy,
}

pub struct Prismriver {
    volume: Volume,

    command_send: channel::Sender<Command>,
    internal_send: channel::Sender<InternalMessage>,

    uri_current: Option<String>,
    uri_next: Option<String>,
}

impl Prismriver {
    pub fn new() -> Prismriver {
        let host = cpal::default_host();
        let device = host.default_output_device().expect("no output device available");

        let (internal_send, internal_recv) = channel::bounded(1);
        let (command_send, command_recv) = channel::unbounded();
        thread::Builder::new().name("audio_player".to_string()).spawn(move || player_loop(internal_recv, command_recv)).unwrap();

        internal_send.send(InternalMessage::NewOutputDevice(device)).unwrap();

        Self {
            volume: Volume::default(),
            uri_current: None,
            uri_next: None,
            internal_send,
            command_send,
        }
    }

    /// Load a new stream from a file
    pub fn load_new(&mut self, uri: &str) {
        self.uri_current = Some(uri.to_string());
        let file = File::open(self.uri_current.as_ref().unwrap()).unwrap();

        self.internal_send.send(InternalMessage::LoadNew(
            MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default()))
        ).unwrap();
    }

    pub fn volume(&self) -> Volume {
        self.volume
    }

    pub fn set_volume(&mut self, vol: Volume) {
        self.volume = vol;
        self.internal_send.send(InternalMessage::Volume(vol)).unwrap();
    }
}

impl Drop for Prismriver {
    fn drop(&mut self) {
        let _ = self.internal_send.try_send(InternalMessage::Destroy);
    }
}

const LOOP_DELAY_US: Duration = Duration::from_micros(5000);
pub const BUFFER_MAX: u64 = 240000 / 4;

fn player_loop(internal_recv: Receiver<InternalMessage>, command_recv: Receiver<Command>) {
    let mut audio_output: Option<Box<dyn AudioOutput>> = None;
    let mut output_device = None;
    let mut decoder: Option<Box<dyn Decoder>> = None;
    let mut volume = Volume::default();

    let mut spec = None;

    let mut output_buffer = [0f32; BUFFER_MAX as usize];

    loop {
        let timer = Instant::now();
        // Check if there are any internal commands to process
        if let Ok(r) = internal_recv.try_recv() {
            match r {
                InternalMessage::LoadNew(f) => {
                    if audio_output.is_none() {
                        panic!("This shouldn't be possible!")
                    }

                    decoder = Some(Box::new(SymphoniaDecoder::new(f)));

                    spec = Some(decoder.as_ref().unwrap().spec());
                    audio_output.as_mut().unwrap().update_signalspec(spec.unwrap());
                },
                InternalMessage::Volume(v) => {
                    volume = v;
                    if let Some(a) = audio_output.as_mut() {
                        a.set_volume(volume)
                    }
                    info!("volume is now {:0.0}%", v.as_f32() * 100.0);
                }
                InternalMessage::NewOutputDevice(device) => {
                    output_device = Some(device);
                    let mut a_out = open_output(&output_device.unwrap()).unwrap();
                    a_out.set_volume(volume);
                    audio_output = Some(a_out);
                },
                InternalMessage::Destroy => {
                    warn!("Destroying playback thread");
                    audio_output.unwrap().flush();
                    break
                },
            }
        }

        let decoder = if let Some(d) = decoder.as_mut() {
            d
        } else {
            continue;
        };

        let audio_output = if let Some(a) = audio_output.as_mut() {
            a
        } else {
            continue;
        };

        // Only decode when buffer is below 50% full
        while audio_output.buffer_level().0 < audio_output.buffer_healthy() {
            let len = decoder.next_packet_to_buf(&mut output_buffer).unwrap();
            audio_output.write(&output_buffer[0..len]).unwrap();
        }

        thread::sleep(LOOP_DELAY_US.saturating_sub(timer.elapsed()));
    }
}

fn get_new_samples() {

}

#[derive(Error, Debug)]
pub enum DecoderError {
    #[error("Failed to decode packet")]
    DecodeFailed,

    #[error("End of stream")]
    EndOfStream,
}

pub trait Decoder {
    fn new(input: MediaSourceStream) -> Self where Self: Sized;

    /// Write the decoder's audio bytes into the provided buffer, and return the
    /// number of bytes written
    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError>;

    fn spec(&self) -> SignalSpec;
}

struct SymphoniaDecoder {
    format_reader: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    sample_buf: Option<SampleBuffer<f32>>,
    track_id: u32,
    spec: SignalSpec,
}

impl Decoder for SymphoniaDecoder {
    fn new(input: MediaSourceStream) -> Self {
        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };

        let probed = symphonia::default::get_probe()
            .format(&Hint::new(), input, &fmt_opts, &meta_opts)
            .expect("unsupported format");
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

        Self {
            spec: SignalSpec::new(
                decoder.codec_params().sample_rate.unwrap(),
                decoder.codec_params().channels.unwrap()
            ),
            format_reader,
            decoder,
            track_id,
            sample_buf: None,
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
                Err(symphonia::core::errors::Error::IoError(e)) => {
                    println!("End of stream: {}", e);
                    return Err(DecoderError::EndOfStream)
                }
                Err(err) => {
                    panic!("{}", err);
                }
            };

            // Consume any new metadata that has been read since the last packet.
            while !self.format_reader.metadata().is_latest() {
                // Pop the old head of the metadata queue.
                self.format_reader.metadata().pop();
            }

            // If the packet does not belong to the selected track, skip over it.
            if packet.track_id() != self.track_id {
                continue;
            }

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
}
