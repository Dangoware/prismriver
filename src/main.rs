use std::io::{self, Write};
use std::{thread::sleep, time::Duration};

use prismriver::State;
use prismriver::Volume;
use prismriver::PrismRiver;

fn main() {
    colog::init();
    let mut player = PrismRiver::new();
    player.set_volume(Volume::new(0.4));

    println!("Loading... short_test.wav");
    player.load_new("short_test.wav").unwrap();
    player.set_state(State::Playing);
    println!("Playing!");

    let mut duration = player.duration();
    while duration.is_none() {
        duration = player.duration()
    }

    while player.state() == State::Playing {
        sleep(Duration::from_millis(100));
        print!("{}/{}\r", player.position().unwrap_or_default().as_secs(), duration.unwrap().as_secs());
        io::stdout().flush().unwrap();
    }
    println!();

    player.load_new("short_test2.wav").unwrap();
    player.set_state(State::Playing);

    let mut duration = player.duration();
    while duration.is_none() {
        duration = player.duration()
    }

    while player.state() == State::Playing {
        sleep(Duration::from_millis(100));
        print!("{}/{}\r", player.position().unwrap_or_default().as_secs(), duration.unwrap().as_secs());
        io::stdout().flush().unwrap();
    }
    println!();

    println!("It's so over")
}

