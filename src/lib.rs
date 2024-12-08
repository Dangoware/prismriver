mod resampler;
mod audio_output;
mod decode;

use std::{fs::File, sync::{Arc, RwLock}, thread, time::Duration};

pub use audio_output::{AudioOutput, Volume};
use cpal::{traits::HostTrait as _, Device};
use crossbeam::channel::{self, Receiver, Sender};
use decode::{Decoder, SymphoniaDecoder};
use log::{info, warn};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions, ReadOnlySource};
use thiserror::Error;

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
    LoadNew(MediaSourceStream),

    LoadNext(MediaSourceStream),

    /// Give the thread a new output device
    NewOutputDevice(Device),

    /// Call once to kill the receiving thread
    Destroy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
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

pub struct PrismRiver {
    volume: Volume,

    state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,

    internal_send: channel::Sender<InternalMessage>,
    internal_recvback: channel::Receiver<Result<(), PrismError>>,

    uri_current: Option<String>,
    uri_next: Option<String>,
}

impl PrismRiver {
    pub fn new() -> PrismRiver {
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
    #[must_use]
    pub fn load_new(&mut self, uri: &str) -> Result<(), PrismError> {
        let mss = if uri.starts_with("h") {
            let stream = reqwest::blocking::get(uri).unwrap();
            MediaSourceStream::new(Box::new(ReadOnlySource::new(stream)), MediaSourceStreamOptions::default())
        } else {
            let file = File::open(uri).unwrap();
            MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default())
        };

        self.uri_current = Some(uri.to_string());

        self.send_recv(InternalMessage::LoadNew(mss))
    }

    /// Set a new stream to be played after the current one ends.
    ///
    /// This allows for gapless transitions.
    #[must_use]
    pub fn load_next(&mut self, uri: &str) -> Result<(), PrismError> {
        let mss = if uri.starts_with("h") {
            let stream = reqwest::blocking::get(uri).unwrap();
            MediaSourceStream::new(Box::new(ReadOnlySource::new(stream)), MediaSourceStreamOptions::default())
        } else {
            let file = File::open(uri).unwrap();
            MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default())
        };

        self.uri_next = Some(uri.to_string());

         self.send_recv(InternalMessage::LoadNext(mss))
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
        self.state.read().unwrap().clone()
    }

    pub fn set_state(&mut self, state: State) {
        *self.state.write().unwrap() = state
    }

    pub fn seek(&mut self, pos: Duration) -> Result<(), PrismError> {
        self.send_recv(InternalMessage::Seek(pos))
    }

    pub fn position(&mut self) -> Option<Duration> {
        self.position.read().unwrap().clone()
    }

    pub fn duration(&mut self) -> Option<Duration> {
        self.duration.read().unwrap().clone()
    }
}

impl Drop for PrismRiver {
    fn drop(&mut self) {
        let _ = self.internal_send.try_send(InternalMessage::Destroy);
    }
}

const LOOP_DELAY_US: Duration = Duration::from_micros(5000);
pub const BUFFER_MAX: u64 = 240_000 / 4;

fn player_loop(
    internal_recv: Receiver<InternalMessage>,
    internal_send: Sender<Result<(), PrismError>>,
    state: Arc<RwLock<State>>,
    position: Arc<RwLock<Option<Duration>>>,
    duration: Arc<RwLock<Option<Duration>>>,
) {
    let mut audio_output: Option<Box<dyn AudioOutput>> = None;
    let mut output_device = None;
    let mut decoder: Option<Box<dyn Decoder>> = None;
    let mut volume = Volume::default();
    let mut next_source = None;

    let mut spec = None;

    let mut output_buffer = [0f32; BUFFER_MAX as usize];

    'external: loop {
        // Check if there are any internal commands to process
        if let Ok(r) = internal_recv.try_recv() {
            match r {
                InternalMessage::NewOutputDevice(device) => {
                    output_device = Some(device);
                    let mut a_out = audio_output::open_output(&output_device.unwrap()).unwrap();
                    a_out.set_volume(volume);
                    audio_output = Some(a_out);
                },
                InternalMessage::LoadNew(f) => {
                    if audio_output.is_none() {
                        panic!("This shouldn't be possible!")
                    }

                    decoder = match SymphoniaDecoder::new(f) {
                        Ok(d) => Some(Box::new(d)),
                        Err(e) => {
                            let _ = internal_send.try_send(Err(PrismError::DecoderError(e)));
                            continue;
                        },
                    };

                    spec = Some(decoder.as_ref().unwrap().spec());
                    audio_output.as_mut().unwrap().update_signalspec(spec.unwrap());
                    internal_send.try_send(Ok(())).unwrap();
                },
                InternalMessage::LoadNext(f) => {
                    next_source = Some(f)
                },
                InternalMessage::Volume(v) => {
                    volume = v;
                    if let Some(a) = audio_output.as_mut() {
                        a.set_volume(volume)
                    }
                    info!("volume is now {:0.0}%", v.as_f32() * 100.0);
                }
                InternalMessage::Seek(p) => {
                    match if let Some(d) = decoder.as_mut() {
                        d.seek(p)
                    } else {
                        internal_send.send(Err(PrismError::NothingLoaded)).unwrap();
                        continue;
                    } {
                        Ok(_) => (),
                        Err(e) => {
                            internal_send.send(Err(PrismError::DecoderError(e))).unwrap();
                            continue;
                        },
                    }

                    internal_send.send(Ok(())).unwrap();
                },
                InternalMessage::Destroy => {
                    warn!("Destroying playback thread");
                    audio_output.unwrap().flush();
                    break
                },
            }
        }

        if *state.read().unwrap() == State::Playing {
            if decoder.is_none() {
                *state.write().unwrap() = State::Stopped;
                continue;
            }

            if audio_output.is_none() {
                *state.write().unwrap() = State::Stopped;
                continue;
            }

            // Only decode when buffer is below the healthy mark
            while audio_output.as_mut().unwrap().buffer_level().0 < audio_output.as_mut().unwrap().buffer_healthy() {
                let len = match decoder.as_mut().unwrap().next_packet_to_buf(&mut output_buffer) {
                    Ok(l) => l,
                    Err(decode::DecoderError::EndOfStream) => {
                        decoder = None;
                        audio_output.as_mut().unwrap().flush();
                        *state.write().unwrap() = State::Stopped;
                        *position.write().unwrap() = None;
                        continue 'external;
                    },
                    Err(de) => {
                        let _ = internal_send.send(Err(PrismError::DecoderError(de)));
                        decoder = None;
                        *state.write().unwrap() = State::Stopped;
                        *position.write().unwrap() = None;
                        continue 'external;
                    },
                };
                audio_output.as_mut().unwrap().write(&output_buffer[0..len]).unwrap();
            }
            *duration.write().unwrap() = Some(decoder.as_mut().unwrap().duration().unwrap());
            *position.write().unwrap() = Some(decoder.as_mut().unwrap().position().unwrap());
            //info!("buffer {:0.0}%", (audio_output.buffer_level().0 as f32 / audio_output.buffer_level().1 as f32) * 100.0);
        }

        // Prevent this from hogging a core
        thread::sleep(LOOP_DELAY_US);
    }
}
