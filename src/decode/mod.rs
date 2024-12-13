use std::time::{Duration, Instant};
use thiserror::Error;

// Mods and stuff -----------
// --------------------------
#[cfg(feature = "symphonia")]
mod rusty;
#[cfg(feature = "symphonia")]
pub use rusty::RustyDecoder;

mod midi;
pub use midi::MidiDecoder;

#[cfg(feature = "ffmpeg")]
mod ffmpeg;
#[cfg(feature = "ffmpeg")]
pub use ffmpeg::FfmpegDecoder;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamParams {
    pub rate: u32,
    pub channels: u16,
    pub packet_size: u64,
}

pub trait Decoder {
    /// Write the decoder's audio bytes into the provided buffer, and return the
    /// number of bytes written
    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError>;

    fn seek(&mut self, pos: Duration) -> Result<(), DecoderError>;

    fn position(&self) -> Option<Duration>;
    fn duration(&self) -> Option<Duration>;

    fn params(&self) -> StreamParams;
}

pub struct DummyDecoder {
    instant: std::time::Instant,
}

impl DummyDecoder {
    pub fn new() -> Self {
        Self {
            instant: Instant::now()
        }
    }
}

impl Decoder for DummyDecoder {
    fn seek(&mut self, _pos: Duration) -> Result<(), DecoderError> {
        Ok(())
    }

    fn position(&self) -> Option<Duration> {
        Some(self.instant.elapsed())
    }

    fn duration(&self) -> Option<Duration> {
        Some(Duration::MAX)
    }

    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError> {
        buf[..4096].copy_from_slice(&[0f32; 4096]);
        Ok(4096)
    }

    fn params(&self) -> StreamParams {
        StreamParams {
            rate: 44100,
            channels: 2,
            packet_size: 4096
        }
    }
}
