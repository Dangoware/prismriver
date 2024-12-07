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

    println!("Playing 01 1ミリ Symphony.flac");
    player.load_new("01 1ミリ Symphony.flac");

    sleep(Duration::from_secs(4));

    println!("Playing bless_you_girl.mp3");
    player.load_new("bless_you_girl.mp3");

    sleep(Duration::from_secs(4));

    println!("Playing short_test.wav");
    player.load_new("short_test.wav");

    sleep(Duration::from_secs(10));

    println!("Playing short_test2.wav");
    player.load_new("short_test2.wav");

    sleep(Duration::from_secs(100));
}

