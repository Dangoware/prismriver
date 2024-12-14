use std::{path::PathBuf, sync::{Arc, RwLock}, thread, time::Duration};
use crossbeam::channel::{Receiver, Sender};
use ffmpeg_next::{codec::{self, Context}, decoder::Audio, filter, format::{context::Input, sample::{self, Type}}, frame, media, rescale, Rational, Rescale as _};
use fluent_uri::Uri;
use log::info;

use crate::uri_to_path;

use super::{Decoder, DecoderError, StreamParams};

pub struct FfmpegDecoder {
    seek_send: Sender<i64>,
    data_recv: Receiver<Arc<[f32]>>,
    stream_params: StreamParams,

    duration: Option<Duration>,
    position: Arc<RwLock<Option<Duration>>>,
}

impl FfmpegDecoder {
    pub fn new(input: Uri<String>) -> Result<Self, DecoderError> {
        ffmpeg_next::init()
            .map_err(|e| DecoderError::InternalError(e.to_string()))?;
        ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Quiet);

        let mut ictx = if input.scheme().as_str().starts_with("http") {
            info!("playing back from network source");
            ffmpeg_next::format::input::<PathBuf>(&input.to_string().into())
                .map_err(|e| DecoderError::InternalError(e.to_string()))?
        } else {
            let path = uri_to_path(&input).unwrap();
            ffmpeg_next::format::input(&path)
                .map_err(|e| DecoderError::InternalError(e.to_string()))?
        };


        let stream = ictx
            .streams()
            .best(media::Type::Audio)
            .ok_or(DecoderError::InternalError("Could not find audio stream".to_string()))?;

        // Duration in ms
        let duration = Some(Duration::from_millis(ictx.duration().rescale(rescale::TIME_BASE, (1, 1000)) as u64));
        let position = Arc::new(RwLock::new(None));

        let context = Context::from_parameters(stream.parameters()).unwrap();
        let mut decoder = context.decoder().audio().unwrap();
        decoder.set_parameters(stream.parameters()).unwrap();
        decoder.set_time_base(rescale::TIME_BASE);

        let rate = match decoder.rate() {
            r if r <= 96000 => decoder.rate(),
            _ => 48000,
        };

        let stream_params = StreamParams {
            rate,
            channels: decoder.channels(),
            packet_size: if decoder.frame_size() != 0 {
                decoder.frame_size() as u64
            } else {
                4096
            },
        };

        // Set up the filter for resampling and sample format
        let filter = filter(&decoder, stream_params).unwrap();

        let (seek_send, seek_recv) = crossbeam::channel::bounded::<i64>(1);
        let (data_send, data_recv) = crossbeam::channel::bounded(0);
        thread::spawn({
            let position = Arc::clone(&position);
            let input_time_base = stream.time_base();
            move || {
                decode_loop(
                    ictx,
                    decoder,
                    filter,
                    input_time_base,
                    position,
                    data_send,
                    seek_recv,
                );
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

    fn seek_absolute(&mut self, pos: Duration) -> Result<(), DecoderError> {
        self.seek_send.send(pos.as_millis() as i64).unwrap();
        Ok(())
    }

    fn seek_relative(&mut self, pos: Duration) -> Result<(), DecoderError> {
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

fn filter(
    decoder: &codec::decoder::Audio,
    params: StreamParams,
) -> Result<filter::Graph, ffmpeg_next::Error> {
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
        out.set_channel_layout(ffmpeg_next::ChannelLayout::HEXADECAGONAL);
        out.set_sample_rate(params.rate);
    }

    filter.output("in", 0)?.input("out", 0)?.parse("anull")?;
    filter.validate()?;

    Ok(filter)
}

fn decode_loop(
    mut ictx: Input,
    mut decoder: Audio,
    mut filter: filter::Graph,
    input_time_base: Rational,
    position: Arc<RwLock<Option<Duration>>>,
    data_send: Sender<Arc<[f32]>>,
    seek_recv: Receiver<i64>,
) {
    while let Some((_stream, packet)) = ictx.packets().next() {
        // Decode the frame
        let mut decoded = frame::Audio::empty();
        decoder.send_packet(&packet).unwrap_or_default();
        while decoder.receive_frame(&mut decoded).is_ok() {
            // Filter the frame to the proper format
            let mut filtered = frame::Audio::empty();
            filter.get("in").unwrap().source().add(&decoded).unwrap();

            while filter.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
                if filtered.timestamp().is_some() {
                    let pos = Some(Duration::from_millis(
                        filtered.timestamp()
                            .unwrap()
                            .rescale(input_time_base, (1, 1000)) as u64
                    ));
                    *position.write().unwrap() = pos;
                }

                let output: Vec<f32> = (0..filtered.planes()).flat_map(|p| filtered.plane::<f32>(p)).copied().collect();

                data_send.send(output.as_slice().into()).unwrap();
                drop(output);
            }
        }

        // Check for seek events and seek on them
        if let Some(s) = seek_recv.try_recv().ok() {
            let position = s.rescale((1, 1000), rescale::TIME_BASE);
            ictx.seek(position, ..position).unwrap();
            decoder.flush();
        }
    }
    decoder.send_eof().unwrap();
}
