mod resampler;
mod audio_output;
mod decode;

use audio_output::{open_output, AudioOutput};
use cpal::traits::HostTrait as _;
use decode::RustPlayer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

fn main() {
    let src = std::fs::File::open("./bless_you_girl.mp3").expect("failed to open media");
    //let src = reqwest::blocking::get("https://g2games.dev/assets/hosted_files/misc/sounds1/07.%20Reboot%20Tactics.flac").unwrap();

    let player = RustPlayer::new(src);
}

