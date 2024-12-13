use std::{fs::{self, File}, thread, time::Duration};

use super::{Decoder, DecoderError, StreamParams};

use midly::{Smf, Timing};
use oxisynth::{MidiEvent, SoundFont, Synth};
use rb::{Consumer, RbConsumer, RbInspector, RbProducer, RB as _};

pub struct MidiDecoderInternal {
    synth: Synth,
    midi_parsed: Smf<'static>,
    selected_track: usize,

    track_pos: usize,
    tick_pos: usize,
    timing: MidiTiming,
}

pub struct MidiDecoder {
    output_buffer: (Consumer<f32>, Consumer<f32>),
}

struct MidiTiming {
    timing: Timing,
    tempo: Option<u32>,
}

impl MidiTiming {
    fn ticks_to_micros(&self, ticks: usize) -> Option<Duration> {
        match self.timing {
            Timing::Metrical(b) => {
                if let Some(t) = self.tempo {
                    let us_per_tick = t as u64 / b.as_int() as u64;
                    return Some(Duration::from_micros(ticks as u64 * us_per_tick))
                }
            }
            Timing::Timecode(fps, subframe) => (),
        }
        None
    }
}

const SAMPLE_RATE: u32 = 44_100;
const PACKET_LEN: usize = SAMPLE_RATE as usize / 10;
const CHANNEL_LEN: usize = PACKET_LEN / 2;

impl MidiDecoder {
    pub fn new() -> Result<Self, ()> {
        let midi_file = fs::read("/media/g2/Storage4/Media-Files/Music/MIDI/touhou-midi-collection/1 - Shooter Games/6 - The Embodiment of Scarlet Devil/5. Tomboyish Girl in Love (ZUN).mid").unwrap();
        let midi_parsed = Smf::parse(&midi_file).unwrap().make_static();

        let settings = oxisynth::SynthDescriptor {
            sample_rate: SAMPLE_RATE as f32,
            gain: 1.0,
            drums_channel_active: true,
            reverb_active: true,
            chorus_active: true,
            polyphony: 256,
            ..Default::default()
        };

        let mut synth = Synth::new(settings).unwrap();

        let mut file = File::open("/usr/share/soundfonts/Touhou.sf2").unwrap();
        let sound_font = SoundFont::load(&mut file).unwrap();
        synth.add_font(sound_font, true);

        let rb1 = rb::SpscRb::new(PACKET_LEN * 2);
        let rb2 = rb::SpscRb::new(PACKET_LEN * 2);
        let (ring_buf_1, ring_buf_consumer_1) = (rb1.producer(), rb1.consumer());
        let (ring_buf_2, ring_buf_consumer_2) = (rb2.producer(), rb2.consumer());

        let mut midi_thing = MidiDecoderInternal {
            timing: MidiTiming { timing: midi_parsed.header.timing, tempo: None },
            synth,
            midi_parsed,
            selected_track: 0,
            tick_pos: 0,
            track_pos: 0,
        };

        // Get timing information
        if midi_thing.timing.ticks_to_micros(midi_thing.tick_pos).is_none() {
            while midi_thing.timing.ticks_to_micros(midi_thing.tick_pos).is_none() {
                midi_thing.tick_pos += midi_thing.midi_parsed.tracks[0][midi_thing.track_pos].delta.as_int() as usize;
                match midi_thing.midi_parsed.tracks[0][midi_thing.track_pos].kind {
                    midly::TrackEventKind::Meta(meta_message) => {
                        match meta_message {
                            midly::MetaMessage::Tempo(t) => midi_thing.timing.tempo = Some(t.as_int()),
                            _ => (),
                        }
                    },
                    _ => (),
                }
                midi_thing.track_pos += 1;
            }
        }

        thread::spawn({
            move || {
                let nanos_per_sample = 1_000_000_000 / SAMPLE_RATE as usize;
                loop {
                    let this_chunk_time = midi_thing.timing.ticks_to_micros(midi_thing.midi_parsed.tracks[midi_thing.selected_track][midi_thing.track_pos].delta.as_int() as usize).unwrap();
                    let this_chunk_increment = this_chunk_time.as_nanos() as usize / nanos_per_sample;

                    for _ in 0..this_chunk_increment {
                        let (l, r) = midi_thing.synth.read_next();

                        ring_buf_1.write_blocking(&[l]);
                        ring_buf_2.write_blocking(&[r]);
                    }

                    match midi_thing.midi_parsed.tracks[midi_thing.selected_track][midi_thing.track_pos].kind {
                        midly::TrackEventKind::Midi { channel, message } => {
                            let _res = match message {
                                midly::MidiMessage::NoteOff { key, .. } => midi_thing.synth.send_event(MidiEvent::NoteOff {
                                    channel: channel.as_int(),
                                    key: key.as_int()
                                }),
                                midly::MidiMessage::NoteOn { key, vel } => midi_thing.synth.send_event(MidiEvent::NoteOn {
                                    channel: channel.as_int(),
                                    key: key.as_int(),
                                    vel: vel.as_int()
                                }),
                                midly::MidiMessage::Aftertouch { key, vel } => midi_thing.synth.send_event(MidiEvent::PolyphonicKeyPressure {
                                    channel: channel.as_int(),
                                    key: key.as_int(),
                                    value: vel.as_int()
                                }),
                                midly::MidiMessage::Controller { controller, value } => midi_thing.synth.send_event(MidiEvent::ControlChange {
                                    channel: channel.as_int(),
                                    ctrl: controller.as_int(),
                                    value: value.as_int()
                                }),
                                midly::MidiMessage::ProgramChange { program } => midi_thing.synth.send_event(MidiEvent::ProgramChange {
                                    channel: channel.as_int(),
                                    program_id: program.as_int()
                                }),
                                midly::MidiMessage::ChannelAftertouch { vel } => midi_thing.synth.send_event(MidiEvent::ChannelPressure {
                                    channel: channel.as_int(),
                                    value: vel.as_int()
                                }),
                                midly::MidiMessage::PitchBend { bend } => midi_thing.synth.send_event(MidiEvent::PitchBend {
                                    channel: channel.as_int(),
                                    value: bend.0.as_int()
                                }),
                            };
                        },
                        midly::TrackEventKind::Meta(meta_message) => {
                            dbg!(meta_message);
                            match meta_message {
                                midly::MetaMessage::Tempo(t) => midi_thing.timing.tempo = Some(t.as_int()),
                                midly::MetaMessage::EndOfTrack => break,
                                _ => (),
                            }
                        },
                        _ => (),
                    }

                    dbg!(midi_thing.track_pos);
                    midi_thing.track_pos += 1;
                }

                for _ in 0..SAMPLE_RATE * 2 {
                    let (l, r) = midi_thing.synth.read_next();

                    ring_buf_1.write_blocking(&[l]);
                    ring_buf_2.write_blocking(&[r]);
                }
            }
        });

        Ok(Self { output_buffer: (ring_buf_consumer_1, ring_buf_consumer_2), })
    }
}

impl Decoder for MidiDecoder {
    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError> {
        self.output_buffer.0.read_blocking_timeout(
            &mut buf[..CHANNEL_LEN],
            Duration::from_millis(5)
        ).unwrap();
        self.output_buffer.1.read_blocking_timeout(
            &mut buf[CHANNEL_LEN..PACKET_LEN],
            Duration::from_millis(5)
        ).unwrap();

        Ok(PACKET_LEN)
    }

    fn seek(&mut self, _pos: std::time::Duration) -> Result<(), DecoderError> {
        todo!()
    }

    fn position(&self) -> Option<std::time::Duration> {
        None
    }

    fn duration(&self) -> Option<std::time::Duration> {
        // Cannot get the duration with this library
        None
    }

    fn params(&self) -> StreamParams {
        StreamParams {
            rate: SAMPLE_RATE as u32,
            channels: 2,
            packet_size: PACKET_LEN as u64,
        }
    }
}
