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
    /// This state will be set if there was a fatal decoder error
    Errored,
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
        // If there was already an error queued up, stuff is errored and this
        // can't continue
        if let Ok(Err(e)) = self.internal_recvback.try_recv() {
            return Err(e)
        }

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

const LOOP_DELAY: std::time::Duration = std::time::Duration::from_micros(5000);
pub const BUFFER_MAX: u64 = 240_000 / size_of::<f32>() as u64; // 240 KB

fn player_loop(
    internal_recv: Receiver<InternalMessage>,
    internal_send: Sender<Result<(), PrismError>>,
    playback_state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,
    metadata: Arc<RwLock<HashMap<String, String>>>,
) {
    let mut audio_output = None;
    let mut audio_device = None;

    let mut decoder = None;
    let mut volume = Volume::default();

    let mut next_source = None;
    let mut stream_params = None;

    let mut player_state = PlayerState {
        playback_state,
        position,
        duration,
        metadata,
    };

    // Set thread priority on Windows to avoid stutters
    // TODO: Do we have any other problems on other platforms?
    // Linux seems to be fine
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
        if let Ok(r) = internal_recv.try_recv() {
            match r {
                InternalMessage::NewOutputDevice(device) => {
                    audio_device = Some(device);
                    let mut a_out =
                        audio_output::open_output(&audio_device.unwrap()).unwrap();
                    a_out.set_volume(volume);
                    audio_output = Some(a_out);
                }
                InternalMessage::LoadNew(f) => {
                    if audio_output.is_none() {
                        panic!("This shouldn't be possible!")
                    }

                    // Try to select a format decoder
                    decoder = utils::pick_format(&f);

                    if let Some(d) = decoder.as_ref() {
                        stream_params = Some(d.params());
                        audio_output
                            .as_mut()
                            .unwrap()
                            .update_params(stream_params.unwrap());
                        internal_send.try_send(Ok(())).unwrap();
                    } else {
                        warn!("could not determine decoder to use for format");
                        internal_send
                            .try_send(Err(PrismError::UnknownFormat))
                            .unwrap();
                    }
                }
                InternalMessage::LoadNext(f) => next_source = Some(f),
                InternalMessage::Volume(v) => {
                    volume = v;
                    if let Some(a) = audio_output.as_mut() {
                        a.set_volume(v)
                    }
                    info!("volume is now {:0.0}%", v.as_f32() * 100.0);
                }
                InternalMessage::Seek(p, relative) => {
                    match if let Some(d) = decoder.as_mut() {
                        match relative {
                            true => d.seek_relative(p),
                            false => d.seek_absolute(p),
                        }
                    } else {
                        internal_send
                            .send(Err(PrismError::NothingLoaded))
                            .unwrap();
                        continue;
                    } {
                        Ok(_) => (),
                        Err(e) => {
                            internal_send
                                .send(Err(PrismError::DecoderError(e)))
                                .unwrap();
                            continue;
                        }
                    }

                    audio_output.as_mut().unwrap().seek_flush();

                    internal_send.send(Ok(())).unwrap();
                }
                InternalMessage::Destroy => {
                    warn!("destroying playback thread");
                    audio_output.unwrap().flush();
                    break;
                }
            }
        }

        if *player_state.playback_state.read().unwrap() == State::Playing {
            let Some(dec) = decoder.as_mut() else {
                *player_state.playback_state.write().unwrap() = State::Stopped;
                continue;
            };

            let Some(aud_out) = audio_output.as_mut() else {
                *player_state.playback_state.write().unwrap() = State::Stopped;
                continue;
            };

            // Only decode when buffer is below the healthy mark
            while aud_out.buffer_level().0 < aud_out.buffer_healthy() {
                if timer.elapsed() > LOOP_DELAY {
                    // Never get stuck in here too long, but if this happens the
                    // decoding speed is too slow
                    warn!("decoding taking more than {}ms", LOOP_DELAY.as_millis());
                    break;
                }

                let len = match dec.next_packet_to_buf(&mut output_buffer) {
                    Ok(l) => l,
                    Err(decode::DecoderError::EndOfStream) => {
                        // End of Stream reached, shut down everything and reset to
                        // stopped state
                        decoder = None;
                        aud_out.flush();
                        player_state.set_state(State::Stopped);
                        continue 'external;
                    }
                    Err(de) => {
                        // Fatal decoder error
                        let _ = internal_send.send(Err(PrismError::DecoderError(de)));
                        decoder = None;
                        player_state.set_state(State::Stopped);
                        continue 'external;
                    }
                };

                // Write the decoded data to the output
                aud_out.write(&output_buffer[..len]);
            }

            let delay = aud_out.buffer_delay();
            //dbg!(delay);

            player_state.set_times(
                dec.duration(),
                dec.position().map(|d| d - delay)
            );

            *player_state.metadata.write().unwrap() = dec.metadata();
            //info!("buffer {:0.0}%", (p_state.audio_output.as_mut().unwrap().buffer_level().0 as f32 / p_state.audio_output.as_mut().unwrap().buffer_level().1 as f32) * 100.0);
        }

        // Prevent this from hogging a core
        thread::sleep(LOOP_DELAY);
    }
}

struct PlayerState {
    playback_state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,
    metadata: Arc<RwLock<HashMap<String, String>>>,
}

impl PlayerState {
    /// Sets the shared variables to the "stopped" state
    fn set_state(&mut self, state: State) {
        *self.playback_state.write().unwrap() = state;

        match state {
            State::Stopped | State::Errored => {
                *self.position.write().unwrap() = None;
                *self.duration.write().unwrap() = None;
                self.metadata.write().unwrap().clear();
            },
            State::Playing => (),
            State::Paused => (),
            State::Buffering(_) => (),
        }
    }

    fn set_times(
        &mut self,
        duration: Option<Duration>,
        position: Option<Duration>,
    ) {
        *self.duration.write().unwrap() = duration;
        *self.position.write().unwrap() = position;
    }
}
