mod resampler;
mod audio_output;
mod decode;

use std::{thread::sleep, time::Duration};

use audio_output::Volume;
use decode::Prismriver;

fn main() {
    colog::init();
    let mut player = Prismriver::new();
    player.set_volume(Volume::new(0.4));

    println!("Playing bless_you_girl.mp3");
    player.load_new("bless_you_girl.mp3");

    sleep(Duration::from_secs(100));
}

