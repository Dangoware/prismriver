//! An audio playback library intended for music players.
//!
//! This crate is being developed alongside Dango Music Player as the audio
//! playback backend. The overall goal of Prismriver is not to create an
//! all-rust playback library; those already exist. Instead, it's supposed to
//! function similarly to Gstreamer, where one frontend can be used to build
//! multimedia applications easily. Because of that, being 100% Rust is not a
//! goal of this project, except where that would improve cross compilation or
//! ease development.
//!
//! Playback is easy! Just create a new [`Prismriver`] instance, and then give
//! it a [`Uri<String>`] to play using [`Prismriver::load_new`]. A `URI` can be
//! easily created from a path using [`utils::path_to_uri`]. Then, set the state
//! to [`State::Playing`] and the file will play. Then just wait for the
//! playback state to be [`State::Stopped`]. If you want to transition between
//! tracks gaplessly, wait for the [`Flag::AboutToFinish`] flag to be set, and
//! feed it a new uri again using [`Prismriver::load_new`].
//!
//! ## Basic Example
//! This will play all paths passed to it in order.
//! ```
//! use prismriver::{
//!     Prismriver, State, Volume,
//!     utils::path_to_uri,
//! };
//! use std::thread::sleep;
//!
//! let mut player = Prismriver::new();
//! player.set_volume(Volume::new(0.4));
//!
//! let paths: Vec<String> = std::env::args().skip(1).collect();
//!
//! for path in paths {
//!     println!("Playing... {}", path);
//!
//!     let path_uri = path_to_uri(&path).unwrap();
//!
//!     player.load_new(&path_uri).unwrap();
//!     player.set_state(State::Playing);
//!
//!     while player.state() == State::Playing || player.state() == State::Paused {
//!         sleep(std::time::Duration::from_millis(100));
//!     }
//! }
//! ```

//#![warn(missing_docs)]

pub mod audio_output;
pub mod decode;
pub mod utils;

pub use audio_output::Volume;
use chrono::Duration;
use cpal::Device;
use fluent_uri::Uri;

enum InternalMessage {
    /// Set a volume level
    Volume(Volume),

    /// Seek to a specified duration
    Seek(Duration, bool),

    /// Set up the thread with a new stream.
    ///
    /// The bool determines whether to immediately end the currently playing
    /// stream and flush all buffers.
    LoadNew(Uri<String>, bool),

    /// Remove the current stream and flush all buffers immediately, stopping playback.
    RemoveCurrent,

    /// Give the thread a new output device
    NewOutputDevice(Device),

    /// Call once to kill the receiving thread
    Destroy,
}

/// The current playback state of a [`Prismriver`] instance.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum State {
    /// The playback is stopped, and nothing is loaded.
    #[default]
    Stopped,
    /// The player has a loaded stream, but it is not being actively played.
    Paused,
    /// The player has a loaded stream that is actively being played.
    Playing,
    /// This state will be set if there was a fatal decoder error, otherwise it
    /// should never be set.
    Errored(Error),
    /// Unused for right now, this should be set while the player is buffering
    /// network data.
    Buffering(u8),
}

/// A flag indicating various things about a stream's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// The stream is very close to finishing.
    ///
    /// To ensure gapless playback, swap in a new URI now.
    AboutToFinish,

    /// The stream has ended and a new one can be set.
    EndOfStream,
}

/// An error that a [`Prismriver`] instance can throw.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Internal decoder error.
    #[error("Internal error: {0}")]
    DecoderError(#[from] decode::DecoderError),

    /// The file given is a format incapable of being played.
    #[error("There is no decoder capable of playing the selected format")]
    UnknownFormat,

    /// This operation is invalid with nothing loaded.
    #[error("Nothing is loaded, the operation is invalid")]
    NothingLoaded,

    /// There was a serious issue and the internal thread panicked!
    #[error("The internal thread panicked!")]
    InternalThreadPanic,

    #[error("This stream is not seekable")]
    NotSeekable,
}

/// A player for audio.
///
/// Create a new one using the [`Prismriver::new()`] function, then you can load
/// a stream and perform various actions on it.
pub struct Prismriver {
    volume: Volume,
}

impl Default for Prismriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Prismriver {
    /// Create a new Prismriver instance. This also automatically selects the
    /// host's default output device.
    pub fn new() -> Prismriver {
        todo!()
    }

    /// Load a new stream.
    ///
    /// This flushes all buffers and immediately plays the new stream.
    pub fn load_new(&mut self, uri: &Uri<String>) -> Result<(), Error> {
        todo!()
    }

    /// Load a new stream.
    ///
    /// This does not flush any buffers and will continue playing the previous stream
    /// until the buffer runs out, ensuring gapless transitions.
    pub fn load_gapless(&mut self, uri: &Uri<String>) -> Result<(), Error> {
        todo!()
    }

    /// Get the currently loaded URI.
    pub fn current_uri(&self) -> &Option<Uri<String>> {
        todo!()
    }

    /// Get the volume.
    pub fn volume(&self) -> Volume {
        todo!()
    }

    /// Set the volume.
    pub fn set_volume(&mut self, vol: Volume) {
        todo!()
    }

    /// Get the current playback [`State`].
    pub fn state(&self) -> State {
        todo!()
    }

    /// Set the player's mode to [`State::Playing`].
    ///
    /// This will start any loaded streams.
    pub fn play(&mut self) {
        todo!()
    }

    /// Set the player's mode to [`State::Stopped`].
    ///
    /// This will discard any loaded streams and reset the player to the initial
    /// state.
    pub fn stop(&mut self) {
        todo!()
    }

    /// Set the player's mode to [`State::Paused`].
    ///
    /// This will pause playback of any loaded streams, but keep them loaded.
    pub fn pause(&mut self) {
        todo!()
    }

    /// Get the current [`Flag`] status.
    pub fn flag(&self) -> Option<Flag> {
        todo!()
    }

    /// Seek to an absolute position in the stream.
    ///
    /// The position is capped at the duration of the song, and zero.
    ///
    /// ## Errors:
    /// This will error if the stream is an unseekable network stream.
    pub fn seek_to(&mut self, pos: chrono::Duration) -> Result<(), Error> {
        todo!()
    }

    /// Seek relative to the current position in the stream.
    ///
    /// ## Errors:
    /// This will error if the stream is an unseekable network stream.
    pub fn seek_by(&mut self, pos: chrono::Duration) -> Result<(), Error> {
        todo!()
    }

    /// Get the current playback position.
    ///
    /// This will return [`None`] if nothing is loaded and playing, and also if
    /// the stream decoder does not report a position.
    pub fn position(&self) -> Option<Duration> {
        todo!()
    }

    /// Get the stream's duration.
    ///
    /// This will return [`None`] if nothing is loaded and playing, and also if
    /// the stream decoder does not report a duration.
    pub fn duration(&self) -> Option<Duration> {
        todo!()
    }
}

const LOOP_DELAY: std::time::Duration = std::time::Duration::from_micros(5_000);
const BUFFER_MAX: u64 = 240_000 / size_of::<f32>() as u64; // 240 KB

fn player_loop() {
}
