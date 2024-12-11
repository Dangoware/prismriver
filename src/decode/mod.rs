use std::time::Duration;
use thiserror::Error;

// Mods and stuff -----------
// --------------------------
#[cfg(feature = "symphonia")]
mod rusty;
#[cfg(feature = "symphonia")]
pub use rusty::RustyDecoder;

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
    //fn new(input: MediaSourceStream) -> Result<Self, DecoderError> where Self: Sized;

    fn seek(&mut self, pos: Duration) -> Result<(), DecoderError>;

    fn position(&self) -> Option<Duration>;
    fn duration(&self) -> Option<Duration>;

    /// Write the decoder's audio bytes into the provided buffer, and return the
    /// number of bytes written
    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError>;

    fn params(&self) -> StreamParams;
}
