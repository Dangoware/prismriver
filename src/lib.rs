mod audio_output;
mod decode;
pub mod utils;

use std::{
    collections::HashMap, ops::Deref, sync::{Arc, RwLock}, thread, time::Instant
};

pub use audio_output::{AudioOutput, Volume};
use chrono::{Duration, TimeDelta};
use cpal::{traits::{DeviceTrait, HostTrait as _}, Device};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    AboutToFinish,
    EndOfStream,
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
    flag: Arc<RwLock<Option<Flag>>>,

    internal_send: channel::Sender<InternalMessage>,
    internal_recvback: channel::Receiver<Result<(), PrismError>>,

    uri_current: Option<Uri<String>>,
    uri_next: Option<Uri<String>>,

    metadata: Arc<RwLock<HashMap<String, String>>>,
}

impl Default for Prismriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Prismriver {
    pub fn new() -> Prismriver {
        let (internal_send, internal_recv) = channel::bounded(1);
        let (internal_sendback, internal_recvback) = channel::bounded(1);

        let state = Arc::new(RwLock::new(State::Stopped));
        let position = Arc::new(RwLock::new(None));
        let duration = Arc::new(RwLock::new(None));
        let metadata = Arc::new(RwLock::new(HashMap::new()));
        let flag = Arc::new(RwLock::new(None));
        thread::Builder::new()
            .name("audio_player".to_string())
            .spawn({
                let state = Arc::clone(&state);
                let position = Arc::clone(&position);
                let duration = Arc::clone(&duration);
                let metadata = Arc::clone(&metadata);
                let flag = Arc::clone(&flag);
                move || {
                    player_loop(
                        internal_recv,
                        internal_sendback,
                        state,
                        position,
                        duration,
                        metadata,
                        flag,
                    )
                }
            })
            .unwrap();

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .expect("no output device available");

        internal_send
            .send(InternalMessage::NewOutputDevice(device))
            .unwrap();

        Self {
            position,
            duration,
            volume: Volume::default(),
            state,
            flag,
            uri_current: None,
            uri_next: None,
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
    pub fn load_new(&mut self, uri: &Uri<String>) -> Result<(), PrismError> {
        self.uri_current = Some(uri.clone());
        self.send_recv(InternalMessage::LoadNew(uri.clone()))
    }

    /// Get the volume.
    pub fn volume(&self) -> Volume {
        self.volume
    }

    /// Set the volume.
    pub fn set_volume(&mut self, vol: Volume) {
        self.volume = vol;
        self.internal_send
            .send(InternalMessage::Volume(vol))
            .unwrap();
    }

    pub fn state(&self) -> State {
        *self.state.read().unwrap()
    }

    pub fn set_state(&mut self, state: State) {
        *self.state.write().unwrap() = state
    }

    pub fn flag(&self) -> Option<Flag> {
        *self.flag.read().unwrap()
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

    pub fn position(&self) -> Option<Duration> {
        *self.position.read().unwrap()
    }

    pub fn duration(&self) -> Option<Duration> {
        *self.duration.read().unwrap()
    }

    pub fn metadata(&self) -> HashMap<String, String> {
        self.metadata.read().unwrap().clone()
    }
}

impl Drop for Prismriver {
    fn drop(&mut self) {
        let _ = self.internal_send.try_send(InternalMessage::Destroy);
    }
}

const LOOP_DELAY: std::time::Duration = std::time::Duration::from_micros(5_000);
pub const BUFFER_MAX: u64 = 240_000 / size_of::<f32>() as u64; // 240 KB

fn player_loop(
    internal_recv: Receiver<InternalMessage>,
    internal_send: Sender<Result<(), PrismError>>,
    playback_state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,
    metadata: Arc<RwLock<HashMap<String, String>>>,
    flag: Arc<RwLock<Option<Flag>>>,
) {
    let mut audio_output = None;
    let mut audio_device;

    let mut decoder = None;
    let mut volume = Volume::default();

    let mut next_source;
    let mut stream_params;

    let mut player_state = PlayerState {
        playback_state,
        position,
        duration,
        metadata,
        stream_ending: false,
    };

    let mut decoded_bytes = 0;

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
                            .update_input_params(stream_params.unwrap());
                        decoded_bytes = 0;
                        player_state.load_new();
                        *flag.write().unwrap() = None;
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

        let state = *player_state.playback_state.read().unwrap();
        if state == State::Playing {
            let Some(aud_out) = audio_output.as_mut() else {
                player_state.set_state(State::Stopped);
                continue;
            };

            // Here is the actual EndOfStream handler
            if player_state.stream_ending && aud_out.buffer_level() == 0 {
                aud_out.flush();
                *flag.write().unwrap() = None;
                player_state.set_state(State::Stopped);
                continue;
            }

            //info!("buffer delay: {}ms, decoder {}", aud_out.buffer_delay().num_milliseconds(), decoder.is_some());

            // Only decode when buffer is below the healthy mark
            while decoder.is_some()
                && !player_state.stream_ending
                && aud_out.buffer_level() < aud_out.buffer_healthy()
            {
                if timer.elapsed() > LOOP_DELAY {
                    // Never get stuck in here too long, but if this happens the
                    // decoding speed is too slow
                    warn!("decoding took more than {}ms, buffer level {:0.2}%", LOOP_DELAY.as_millis(), aud_out.buffer_percent());
                    break;
                }

                let len = match decoder.as_mut().unwrap().next_packet_to_buf(&mut output_buffer) {
                    Ok(l) => l,
                    Err(decode::DecoderError::EndOfStream) => {
                        // End of Stream reached, set the EOF flag and wait for
                        // buffer to finish
                        decoder = None;
                        *flag.write().unwrap() = Some(Flag::AboutToFinish);
                        player_state.stream_ending = true;
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

                player_state.set_times(
                    decoder.as_ref().unwrap().duration(),
                    decoder.as_ref().unwrap().position().map(|d|
                        (d - aud_out.buffer_delay()).clamp(TimeDelta::zero(), TimeDelta::MAX)
                    )
                );

                // Write the decoded data to the output
                aud_out.write(&output_buffer[..len]);
                decoded_bytes += len;

                *player_state.metadata.write().unwrap() = decoder.as_ref().unwrap().metadata();
            }

            //info!("buffer {:0.0}%", (p_state.audio_output.as_mut().unwrap().buffer_level().0 as f32 / p_state.audio_output.as_mut().unwrap().buffer_level().1 as f32) * 100.0);
        } else if state == State::Stopped {
            // This would happen when the user manually stops playback
            decoder = None;
            player_state.set_state(State::Stopped);
            continue 'external;
        }

        // Prevent this from hogging a core
        thread::sleep(LOOP_DELAY.saturating_sub(timer.elapsed()));
    }
}

#[derive(Debug, Clone)]
struct PlayerState {
    playback_state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,
    metadata: Arc<RwLock<HashMap<String, String>>>,
    stream_ending: bool,
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
                self.stream_ending = false;
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

    fn load_new(&mut self) {
        *self.playback_state.write().unwrap() = State::Paused;
        *self.duration.write().unwrap() = None;
        *self.position.write().unwrap() = None;
        self.stream_ending = false;
        self.metadata.write().unwrap().clear();
    }
}
