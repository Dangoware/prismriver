use chrono::Duration;
use std::collections::HashMap;
use thiserror::Error;

// Mods and stuff -----------
// --------------------------
#[cfg(feature = "symphonia")]
pub mod rusty;

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;
// End of mods and stuff ----
// --------------------------

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum DecoderError {
    #[error("{0}")]
    InternalError(String),

    #[error("Decode timed out")]
    DecodeTimeout,

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

    /// Seek to the desired absolute position in the stream.
    fn seek_absolute(&mut self, pos: Duration) -> Result<(), DecoderError>;

    /// Seek to a position relative to the current playback position.
    fn seek_relative(&mut self, pos: Duration) -> Result<(), DecoderError>;

    /// Get the current playback position, if it is known.
    fn position(&self) -> Option<Duration>;

    /// Get the current duration, if it is known.
    fn duration(&self) -> Option<Duration>;

    /// Get some useful parameters about the stream.
    fn params(&self) -> StreamParams;

    /// Get metadata from the stream.
    ///
    /// If there is none, the map will be empty.
    fn metadata(&self) -> HashMap<String, String>;
}

pub mod dummy {
    use std::collections::HashMap;
    use std::time::Instant;

    use chrono::Duration;

    use super::{Decoder, DecoderError, StreamParams};

    pub struct DummyDecoder {
        instant: std::time::Instant,
    }

    impl DummyDecoder {
        const PACKET_SIZE: u64 = 4096;

        pub fn new() -> Self {
            Self {
                instant: Instant::now(),
            }
        }
    }

    impl Decoder for DummyDecoder {
        fn seek_absolute(&mut self, _pos: Duration) -> Result<(), DecoderError> {
            Ok(())
        }

        fn seek_relative(&mut self, _pos: Duration) -> Result<(), DecoderError> {
            Ok(())
        }

        fn position(&self) -> Option<Duration> {
            Some(Duration::from_std(self.instant.elapsed()).unwrap())
        }

        fn duration(&self) -> Option<Duration> {
            Some(Duration::MAX)
        }

        fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError> {
            buf[..Self::PACKET_SIZE as usize].copy_from_slice(&[0f32; Self::PACKET_SIZE as usize]);
            Ok(Self::PACKET_SIZE as usize)
        }

        fn params(&self) -> StreamParams {
            StreamParams {
                rate: 44100,
                channels: 2,
                packet_size: Self::PACKET_SIZE,
            }
        }

        fn metadata(&self) -> HashMap<String, String> {
            HashMap::new()
        }
    }
}
