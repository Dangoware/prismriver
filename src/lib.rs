mod audio_output;
mod decode;

use std::{path::PathBuf, sync::{Arc, RwLock}, thread, time::{Duration, Instant}};

pub use audio_output::{AudioOutput, Volume};
use cpal::{traits::HostTrait as _, Device};
use crossbeam::channel::{self, Receiver, Sender};
use decode::{Decoder, MidiDecoder};
use log::{info, warn};
use thiserror::Error;

#[cfg(feature = "symphonia")]
use decode::RustyDecoder;
#[cfg(feature = "ffmpeg")]
use decode::FfmpegDecoder;

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
    Seek(Duration),

    /// Set up the thread with a new stream
    LoadNew(PathBuf),

    LoadNext(PathBuf),

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

#[derive(Error, Debug)]
pub enum PrismError {
    #[error("Internal decoder error {}", 0)]
    DecoderError(#[from] decode::DecoderError),

    #[error("Nothing is loaded, the operation is invalid")]
    NothingLoaded,
}

pub struct Prismriver {
    volume: Volume,

    state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,

    internal_send: channel::Sender<InternalMessage>,
    internal_recvback: channel::Receiver<Result<(), PrismError>>,

    uri_current: Option<String>,
    uri_next: Option<String>,
}

impl Default for Prismriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Prismriver {
    pub fn new() -> Prismriver {
        let host = cpal::default_host();
        let device = host.default_output_device().expect("no output device available");

        let (internal_send, internal_recv) = channel::bounded(1);
        let (internal_sendback, internal_recvback) = channel::bounded(1);

        let state = Arc::new(RwLock::new(State::Stopped));
        let position = Arc::new(RwLock::new(None));
        let duration = Arc::new(RwLock::new(None));
        thread::Builder::new().name("audio_player".to_string()).spawn({
            let state = Arc::clone(&state);
            let position = Arc::clone(&position);
            let duration = Arc::clone(&duration);
            move || player_loop(
                internal_recv,
                internal_sendback,
                state,
                position,
                duration
            )
        }).unwrap();

        internal_send.send(InternalMessage::NewOutputDevice(device)).unwrap();

        Self {
            position,
            duration,
            volume: Volume::default(),
            state,
            uri_current: None,
            uri_next: None,
            internal_send,
            internal_recvback
        }
    }

    fn send_recv(&mut self, message: InternalMessage) -> Result<(), PrismError> {
        self.internal_send.send(message).unwrap();
        self.internal_recvback.recv().unwrap()
    }

    /// Load a new stream.
    ///
    /// This immediately overrides the previous one, flushing the buffer.
    pub fn load_new(&mut self, uri: &str) -> Result<(), PrismError> {
        let path = PathBuf::from(uri);
        path.canonicalize().unwrap();

        self.send_recv(InternalMessage::LoadNew(path))
    }

    /// Set a new stream to be played after the current one ends.
    ///
    /// This allows for gapless transitions.
    pub fn load_next(&mut self, _uri: &str) -> Result<(), PrismError> {
        todo!()
    }

    /// Get the volume
    pub fn volume(&self) -> Volume {
        self.volume
    }

    /// Set the volume
    pub fn set_volume(&mut self, vol: Volume) {
        self.volume = vol;
        self.internal_send.send(InternalMessage::Volume(vol)).unwrap();
    }

    pub fn state(&mut self) -> State {
        *self.state.read().unwrap()
    }

    pub fn set_state(&mut self, state: State) {
        *self.state.write().unwrap() = state
    }

    pub fn seek(&mut self, pos: Duration) -> Result<(), PrismError> {
        self.send_recv(InternalMessage::Seek(pos))
    }

    pub fn position(&mut self) -> Option<Duration> {
        *self.position.read().unwrap()
    }

    pub fn duration(&mut self) -> Option<Duration> {
        *self.duration.read().unwrap()
    }
}

impl Drop for Prismriver {
    fn drop(&mut self) {
        let _ = self.internal_send.try_send(InternalMessage::Destroy);
    }
}

const LOOP_DELAY_US: Duration = Duration::from_micros(5000);
pub const BUFFER_MAX: u64 = 240_000;

struct PlayerState {
    audio_output: Option<Box<dyn AudioOutput>>,
    audio_device: Option<Device>,

    decoder: Option<Box<dyn Decoder>>,
    volume: Volume,

    next_source: Option<PathBuf>,
    stream_params: Option<decode::StreamParams>,

    internal_recv: Receiver<InternalMessage>,
    internal_send: Sender<Result<(), PrismError>>,
    state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,
}

fn player_loop(
    internal_recv: Receiver<InternalMessage>,
    internal_send: Sender<Result<(), PrismError>>,
    state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,
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
    };

    // Set thread priority to avoid stutters
    #[cfg(target_os = "windows")]
    {
        use thread_priority::*;
        if set_current_thread_priority(ThreadPriority::Os(WinAPIThreadPriority::TimeCritical.into())).is_err() {
            warn!("failed to set playback thread priority");
        };
    }

    let mut output_buffer = vec![0f32; BUFFER_MAX as usize];

    'external: loop {
        let timer = Instant::now();
        // Check if there are any internal commands to process
        if let Ok(r) = p_state.internal_recv.try_recv() {
            match r {
                InternalMessage::NewOutputDevice(device) => {
                    p_state.audio_device = Some(device);
                    let mut a_out = audio_output::open_output(&p_state.audio_device.unwrap()).unwrap();
                    a_out.set_volume(p_state.volume);
                    p_state.audio_output = Some(a_out);
                },
                InternalMessage::LoadNew(f) => {
                    if p_state.audio_output.is_none() {
                        panic!("This shouldn't be possible!")
                    }

                    // TODO: Make this detect format and use the appropriate
                    // decoder
                    p_state.decoder = {
                        #[cfg(feature = "ffmpeg")]
                        match FfmpegDecoder::new(f) {
                            Ok(d) => Some(Box::new(d)),
                            Err(e) => {
                                let _ = p_state.internal_send.try_send(Err(PrismError::DecoderError(e)));
                                continue;
                            },
                        }

                        #[cfg(feature = "symphonia")]
                        #[cfg(not(feature = "ffmpeg"))]
                        match MidiDecoder::new() {
                            Ok(d) => Some(Box::new(d)),
                            Err(e) => {
                                let _ = p_state.internal_send.try_send(Err(PrismError::NothingLoaded));
                                continue;
                            },
                        }

                        #[cfg(not(any(feature = "ffmpeg", feature = "symphonia")))]
                        {
                            log::error!("using dummmy decoder, there will be no decoding and no output");
                            Some(Box::new(decode::DummyDecoder::new()))
                        }
                    };

                    p_state.stream_params = Some(p_state.decoder.as_ref().unwrap().params());
                    p_state.audio_output.as_mut().unwrap().update_params(p_state.stream_params.unwrap());
                    p_state.internal_send.try_send(Ok(())).unwrap();
                },
                InternalMessage::LoadNext(f) => {
                    p_state.next_source = Some(f)
                },
                InternalMessage::Volume(v) => {
                    p_state.volume = v;
                    if let Some(a) = p_state.audio_output.as_mut() {
                        a.set_volume(v)
                    }
                    info!("volume is now {:0.0}%", v.as_f32() * 100.0);
                }
                InternalMessage::Seek(p) => {
                    match if let Some(d) = p_state.decoder.as_mut() {
                        d.seek(p)
                    } else {
                        p_state.internal_send.send(Err(PrismError::NothingLoaded)).unwrap();
                        continue;
                    } {
                        Ok(_) => (),
                        Err(e) => {
                            p_state.internal_send.send(Err(PrismError::DecoderError(e))).unwrap();
                            continue;
                        },
                    }

                    p_state.audio_output.as_mut().unwrap().seek_flush();

                    p_state.internal_send.send(Ok(())).unwrap();
                },
                InternalMessage::Destroy => {
                    warn!("destroying playback thread");
                    p_state.audio_output.unwrap().flush();
                    break
                },
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
            while p_state.audio_output.as_mut().unwrap().buffer_level().0 < p_state.audio_output.as_mut().unwrap().buffer_healthy() {
                if timer.elapsed() > LOOP_DELAY_US {
                    // Never get stuck in here too long, but if this happens the
                    // decoding speed is too slow
                    break;
                }

                let len = match p_state.decoder.as_mut().unwrap().next_packet_to_buf(&mut output_buffer) {
                    Ok(l) => l,
                    Err(decode::DecoderError::EndOfStream) => {
                        // End of Stream reached, shut down everything and reset to
                        // stopped state
                        p_state.decoder = None;
                        p_state.audio_output.as_mut().unwrap().flush();
                        *p_state.state.write().unwrap() = State::Stopped;
                        *p_state.position.write().unwrap() = None;
                        continue 'external;
                    },
                    Err(de) => {
                        // Fatal decoder error! TODO: Communicate this somehow
                        let _ = p_state.internal_send.send(Err(PrismError::DecoderError(de)));
                        p_state.decoder = None;
                        *p_state.state.write().unwrap() = State::Stopped;
                        *p_state.position.write().unwrap() = None;
                        continue 'external;
                    },
                };
                p_state.audio_output.as_mut().unwrap().write(&output_buffer[0..len]).unwrap();
            }

            *p_state.duration.write().unwrap() = p_state.decoder.as_mut().unwrap().duration();
            *p_state.position.write().unwrap() = p_state.decoder.as_mut().unwrap().position();

            /*
            print_buffer_state(
                p_state.audio_output.as_mut().unwrap().buffer_level().0,
                p_state.audio_output.as_mut().unwrap().buffer_level().1,
                p_state.audio_output.as_mut().unwrap().buffer_healthy(),
            );
            */
        }

        // Prevent this from hogging a core
        thread::sleep(LOOP_DELAY_US);
    }
}

fn print_buffer_state(
    buffer_current: usize,
    buffer_max: usize,
    buffer_healthy: usize,
) {
    info!(
        "buffer {:0.0}%, healthy is {:0.0}%",
        (buffer_current as f32 / buffer_max as f32) * 100.0,
        (buffer_healthy as f32 / buffer_max as f32) * 100.0,
    );
}
