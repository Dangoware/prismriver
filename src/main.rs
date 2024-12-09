use std::io::{self, Write};
use std::{thread::sleep, time::Duration};

use prismriver::State;
use prismriver::Volume;
use prismriver::PrismRiver;

fn main() {
    colog::init();
    let mut player = PrismRiver::new();
    player.set_volume(Volume::new(0.4));

    let args: Vec<String> = std::env::args().skip(1).collect();

    for path in args {
        println!("Loading... {}", path);
        player.load_new(&path).unwrap();
        player.set_state(State::Playing);
        println!("Playing!");
        while player.state() == State::Playing {
            sleep(Duration::from_millis(100));
            print_timer(player.position().unwrap_or_default(), player.duration());
        }
        println!();
    }

    println!("It's so over")
}

fn print_timer(pos: Duration, len: Option<Duration>) {
    let len_string = if let Some(l) = len {
        format!("{:02}:{:02}", l.as_secs() / 60, l.as_secs() % 60)
    } else {
        format!("--:--")
    };

    print!(
        "{:02}:{:02}/{}\r",
        pos.as_secs() / 60,
        pos.as_secs() % 60,
        len_string,
    );
    io::stdout().flush().unwrap();
}
