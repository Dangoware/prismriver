mod resampler;
mod audio_output;
mod decode;

use decode::RustPlayer;

fn main() {
    let mut player = RustPlayer::new();

    player.set_uri("./bless_you_girl.mp3");
}

