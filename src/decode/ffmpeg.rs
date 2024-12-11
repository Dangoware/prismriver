use std::{io::Read, path::Path, sync::{Arc, Mutex, RwLock}, thread, time::Duration};
use crossbeam::channel::{Receiver, Sender};
use ffmpeg_next::{codec::{self, Context}, filter, format::sample::{self, Type}, frame, media, rescale, Rescale as _};
use rb::{Consumer, RbConsumer, RbProducer, RB as _};

use super::{Decoder, DecoderError, StreamParams};

pub struct FfmpegDecoder {
    seek_send: Sender<i64>,
    data_recv: Receiver<Vec<f32>>,
    stream_params: StreamParams,

    duration: Option<Duration>,
    position: Arc<RwLock<Option<Duration>>>,
}

impl FfmpegDecoder {
    pub fn new<P: AsRef<Path>>(input: P) -> Result<Self, DecoderError> {
        ffmpeg_next::init().unwrap();

        let mut ictx = ffmpeg_next::format::input(&input).unwrap();
        let input = ictx
            .streams()
            .best(media::Type::Audio)
            .expect("could not find best audio stream");

        // Duration in ms
        let duration = Some(Duration::from_millis(ictx.duration().rescale(rescale::TIME_BASE, (1, 1000)) as u64));
        let position = Arc::new(RwLock::new(None));

        let context = Context::from_parameters(input.parameters()).unwrap();
        let mut decoder = context.decoder().audio().unwrap();
        decoder.set_parameters(input.parameters()).unwrap();
        let stream_params = StreamParams {
            rate: 44100,
            channels: decoder.channels(),
            packet_size: if decoder.frame_size() != 0 {
                decoder.frame_size() as u64 * 10
            } else {
                4096
            },
        };

        let mut filter = filter(&decoder).unwrap();

        let (seek_send, seek_recv) = crossbeam::channel::bounded(0);
        let (data_send, data_recv) = crossbeam::channel::bounded(0);
        thread::spawn({
            let position = Arc::clone(&position);
            move || {
                let mut decoded = frame::Audio::empty();
                let mut filtered = frame::Audio::empty();

                for (_stream, packet) in ictx.packets() {
                    // Decode the frame
                    decoder.send_packet(&packet).unwrap_or_default();
                    while decoder.receive_frame(&mut decoded).is_ok() {
                        // Filter the frame to the proper format
                        filter.get("in").unwrap().source().add(&decoded).unwrap();
                        while filter.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
                            let pos = Some(Duration::from_millis(
                                filtered.timestamp().unwrap().rescale(decoder.time_base(), (1, 1000)) as u64)
                            );
                            *position.write().unwrap() = pos;

                            let mut output = vec![];
                            for plane in 0..filtered.planes() {
                                output.extend(filtered.plane::<f32>(plane));
                            }

                            data_send.send(output).unwrap();
                        }
                    }
                }
                decoder.send_eof().unwrap();
            }
        });

        Ok(Self {
            seek_send,
            data_recv,
            stream_params,

            duration,
            position,
        })
    }
}

impl Decoder for FfmpegDecoder {
    fn next_packet_to_buf(&mut self, buf: &mut [f32]) -> Result<usize, DecoderError> {
        let data = match self.data_recv.recv() {
            Ok(l) => l,
            Err(_) => return Err(DecoderError::EndOfStream)
        };
        buf[..data.len()].copy_from_slice(&data);

        Ok(data.len())
    }

    fn seek(&mut self, pos: Duration) -> Result<(), DecoderError> {
        todo!()
    }

    fn position(&self) -> Option<Duration> {
        self.position.read().unwrap().clone()
    }

    fn duration(&self) -> Option<Duration> {
        self.duration
    }

    fn params(&self) -> StreamParams {
        self.stream_params
    }
}

fn filter(decoder: &codec::decoder::Audio,) -> Result<filter::Graph, ffmpeg_next::Error> {
    let mut filter = filter::Graph::new();

    let args = format!(
        "time_base={}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
        decoder.time_base(),
        decoder.rate(),
        decoder.format().name(),
        if decoder.channel_layout().bits() != 0 {
            decoder.channel_layout().bits()
        } else {
            ffmpeg_next::ChannelLayout::STEREO.bits()
        }
    );

    filter.add(&filter::find("abuffer").unwrap(), "in", &args)?;
    filter.add(&filter::find("abuffersink").unwrap(), "out", "")?;

    {
        let mut out = filter.get("out").unwrap();

        out.set_sample_format(sample::Sample::F32(Type::Planar));
        out.set_channel_layout(ffmpeg_next::ChannelLayout::STEREO);
        out.set_sample_rate(44100);
    }

    filter.output("in", 0)?.input("out", 0)?.parse("anull")?;
    filter.validate()?;

    Ok(filter)
}
