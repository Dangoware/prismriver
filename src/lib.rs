mod audio_output;
mod decode;
pub mod utils;

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    thread,
    time::Instant,
};

pub use audio_output::{AudioOutput, Volume};
use chrono::Duration;
use cpal::{traits::HostTrait as _, Device};
use crossbeam::channel::{self, Receiver, Sender};
use decode::Decoder;
use fluent_uri::Uri;
use log::{info, warn};

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

    /// Seek to a specified duration
    Seek(Duration, bool),

    /// Set up the thread with a new stream
    LoadNew(Uri<String>),

    LoadNext(Uri<String>),

    /// Give the thread a new output device
    NewOutputDevice(Device),

    /// Call once to kill the receiving thread
    Destroy,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum State {
    #[default]
    Stopped,
    Playing,
    Paused,
    Buffering(u8),
}

#[derive(thiserror::Error, Debug)]
pub enum PrismError {
    #[error("Internal decoder error {}", 0)]
    DecoderError(#[from] decode::DecoderError),

    #[error("There is no decoder capable of playing the selected format")]
    UnknownFormat,

    #[error("Nothing is loaded, the operation is invalid")]
    NothingLoaded,

    #[error("The internal thread panicked!")]
    InternalThreadPanic,
}

pub struct Prismriver {
    volume: Volume,

    state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,

    internal_send: channel::Sender<InternalMessage>,
    internal_recvback: channel::Receiver<Result<(), PrismError>>,

    _uri_current: Option<Uri<String>>,
    _uri_next: Option<Uri<String>>,

    metadata: Arc<RwLock<HashMap<String, String>>>,
}

impl Default for Prismriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Prismriver {
    pub fn new() -> Prismriver {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .expect("no output device available");

        let (internal_send, internal_recv) = channel::bounded(1);
        let (internal_sendback, internal_recvback) = channel::bounded(1);

        let state = Arc::new(RwLock::new(State::Stopped));
        let position = Arc::new(RwLock::new(None));
        let duration = Arc::new(RwLock::new(None));
        let metadata = Arc::new(RwLock::new(HashMap::new()));
        thread::Builder::new()
            .name("audio_player".to_string())
            .spawn({
                let state = Arc::clone(&state);
                let position = Arc::clone(&position);
                let duration = Arc::clone(&duration);
                let metadata = Arc::clone(&metadata);
                move || {
                    player_loop(
                        internal_recv,
                        internal_sendback,
                        state,
                        position,
                        duration,
                        metadata,
                    )
                }
            })
            .unwrap();

        internal_send
            .send(InternalMessage::NewOutputDevice(device))
            .unwrap();

        Self {
            position,
            duration,
            volume: Volume::default(),
            state,
            _uri_current: None,
            _uri_next: None,
            internal_send,
            internal_recvback,
            metadata,
        }
    }

    fn send_recv(&mut self, message: InternalMessage) -> Result<(), PrismError> {
        self.internal_send.send(message).unwrap();
        match self.internal_recvback.recv() {
            Ok(o) => o,
            Err(_) => Err(PrismError::InternalThreadPanic),
        }
    }

    /// Load a new stream.
    ///
    /// This immediately overrides the previous one, flushing the buffer.
    pub fn load_new(&mut self, uri: &Uri<String>) -> Result<(), PrismError> {
        self.send_recv(InternalMessage::LoadNew(uri.clone()))
    }

    /// Set a new stream to be played after the current one ends.
    ///
    /// This allows for gapless transitions.
    pub fn load_next(&mut self, _uri: &Uri<String>) -> Result<(), PrismError> {
        todo!()
    }

    /// Get the volume
    pub fn volume(&self) -> Volume {
        self.volume
    }

    /// Set the volume
    pub fn set_volume(&mut self, vol: Volume) {
        self.volume = vol;
        self.internal_send
            .send(InternalMessage::Volume(vol))
            .unwrap();
    }

    pub fn state(&mut self) -> State {
        *self.state.read().unwrap()
    }

    pub fn set_state(&mut self, state: State) {
        *self.state.write().unwrap() = state
    }

    /// Seek relative to the current position.
    ///
    /// The position is capped at the duration of the song, and zero.
    pub fn seek_to(&mut self, pos: chrono::Duration) -> Result<(), PrismError> {
        self.send_recv(InternalMessage::Seek(pos, false))
    }

    pub fn seek_by(&mut self, pos: chrono::Duration) -> Result<(), PrismError> {
        self.send_recv(InternalMessage::Seek(pos, true)).unwrap();
        todo!()
    }

    pub fn position(&mut self) -> Option<Duration> {
        *self.position.read().unwrap()
    }

    pub fn duration(&mut self) -> Option<Duration> {
        *self.duration.read().unwrap()
    }

    pub fn metadata(&mut self) -> HashMap<String, String> {
        self.metadata.read().unwrap().clone()
    }
}

impl Drop for Prismriver {
    fn drop(&mut self) {
        let _ = self.internal_send.try_send(InternalMessage::Destroy);
    }
}

const LOOP_DELAY_US: std::time::Duration = std::time::Duration::from_micros(5000);
pub const BUFFER_MAX: u64 = 240_000 / size_of::<f32>() as u64; // 240 KB

struct PlayerState {
    audio_output: Option<Box<dyn AudioOutput>>,
    audio_device: Option<Device>,

    decoder: Option<Box<dyn Decoder>>,
    volume: Volume,

    next_source: Option<Uri<String>>,
    stream_params: Option<decode::StreamParams>,

    internal_recv: Receiver<InternalMessage>,
    internal_send: Sender<Result<(), PrismError>>,
    state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,

    metadata: Arc<RwLock<HashMap<String, String>>>,
}

fn player_loop(
    internal_recv: Receiver<InternalMessage>,
    internal_send: Sender<Result<(), PrismError>>,
    state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,
    metadata: Arc<RwLock<HashMap<String, String>>>,
) {
    let mut p_state = PlayerState {
        internal_recv,
        internal_send,
        state,
        position,
        duration,

        audio_output: None,
        audio_device: None,
        decoder: None,
        volume: Volume::default(),
        next_source: None,
        stream_params: None,

        metadata,
    };

    // Set thread priority to avoid stutters
    #[cfg(target_os = "windows")]
    {
        use thread_priority::*;
        if set_current_thread_priority(ThreadPriority::Os(
            WinAPIThreadPriority::TimeCritical.into(),
        ))
        .is_err()
        {
            warn!("failed to set playback thread priority");
        };
    }

    let mut output_buffer = [0f32; BUFFER_MAX as usize];

    'external: loop {
        let timer = Instant::now();
        // Check if there are any internal commands to process
        if let Ok(r) = p_state.internal_recv.try_recv() {
            match r {
                InternalMessage::NewOutputDevice(device) => {
                    p_state.audio_device = Some(device);
                    let mut a_out =
                        audio_output::open_output(&p_state.audio_device.unwrap()).unwrap();
                    a_out.set_volume(p_state.volume);
                    p_state.audio_output = Some(a_out);
                }
                InternalMessage::LoadNew(f) => {
                    if p_state.audio_output.is_none() {
                        panic!("This shouldn't be possible!")
                    }

                    // Try to select a format decoder
                    p_state.decoder = utils::pick_format(&f);

                    if let Some(d) = p_state.decoder.as_ref() {
                        p_state.stream_params = Some(d.params());
                        p_state
                            .audio_output
                            .as_mut()
                            .unwrap()
                            .update_params(p_state.stream_params.unwrap());
                        p_state.internal_send.try_send(Ok(())).unwrap();
                    } else {
                        warn!("could not determine decoder to use for format");
                        p_state
                            .internal_send
                            .try_send(Err(PrismError::UnknownFormat))
                            .unwrap();
                    }
                }
                InternalMessage::LoadNext(f) => p_state.next_source = Some(f),
                InternalMessage::Volume(v) => {
                    p_state.volume = v;
                    if let Some(a) = p_state.audio_output.as_mut() {
                        a.set_volume(v)
                    }
                    info!("volume is now {:0.0}%", v.as_f32() * 100.0);
                }
                InternalMessage::Seek(p, relative) => {
                    match if let Some(d) = p_state.decoder.as_mut() {
                        match relative {
                            true => d.seek_relative(p),
                            false => d.seek_absolute(p),
                        }
                    } else {
                        p_state
                            .internal_send
                            .send(Err(PrismError::NothingLoaded))
                            .unwrap();
                        continue;
                    } {
                        Ok(_) => (),
                        Err(e) => {
                            p_state
                                .internal_send
                                .send(Err(PrismError::DecoderError(e)))
                                .unwrap();
                            continue;
                        }
                    }

                    p_state.audio_output.as_mut().unwrap().seek_flush();

                    p_state.internal_send.send(Ok(())).unwrap();
                }
                InternalMessage::Destroy => {
                    warn!("destroying playback thread");
                    p_state.audio_output.unwrap().flush();
                    break;
                }
            }
        }

        if *p_state.state.read().unwrap() == State::Playing {
            if p_state.decoder.is_none() {
                *p_state.state.write().unwrap() = State::Stopped;
                continue;
            }

            if p_state.audio_output.is_none() {
                *p_state.state.write().unwrap() = State::Stopped;
                continue;
            }

            // Only decode when buffer is below the healthy mark
            while p_state.audio_output.as_mut().unwrap().buffer_level().0
                < p_state.audio_output.as_mut().unwrap().buffer_healthy()
            {
                if timer.elapsed() > LOOP_DELAY_US {
                    // Never get stuck in here too long, but if this happens the
                    // decoding speed is too slow
                    break;
                }

                let len = match p_state
                    .decoder
                    .as_mut()
                    .unwrap()
                    .next_packet_to_buf(&mut output_buffer)
                {
                    Ok(l) => l,
                    Err(decode::DecoderError::EndOfStream) => {
                        // End of Stream reached, shut down everything and reset to
                        // stopped state
                        p_state.decoder = None;
                        p_state.audio_output.as_mut().unwrap().flush();
                        *p_state.state.write().unwrap() = State::Stopped;
                        *p_state.position.write().unwrap() = None;
                        continue 'external;
                    }
                    Err(de) => {
                        // Fatal decoder error
                        let _ = p_state
                            .internal_send
                            .send(Err(PrismError::DecoderError(de)));
                        p_state.decoder = None;
                        *p_state.state.write().unwrap() = State::Stopped;
                        *p_state.position.write().unwrap() = None;
                        continue 'external;
                    }
                };
                p_state
                    .audio_output
                    .as_mut()
                    .unwrap()
                    .write(&output_buffer[0..len]);
            }

            *p_state.duration.write().unwrap() = p_state.decoder.as_ref().unwrap().duration();
            *p_state.position.write().unwrap() = p_state.decoder.as_ref().unwrap().position();
            *p_state.metadata.write().unwrap() = p_state.decoder.as_ref().unwrap().metadata();
            //info!("buffer {:0.0}%", (p_state.audio_output.as_mut().unwrap().buffer_level().0 as f32 / p_state.audio_output.as_mut().unwrap().buffer_level().1 as f32) * 100.0);
        }

        // Prevent this from hogging a core
        thread::sleep(LOOP_DELAY_US);
    }
}
