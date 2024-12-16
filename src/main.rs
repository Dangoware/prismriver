use std::io::{self, Write};
use std::thread::sleep;

use chrono::{Duration, TimeDelta};

use fluent_uri::Uri;
use prismriver::utils::path_to_uri;
use prismriver::{Flag, Prismriver};
use prismriver::State;
use prismriver::Volume;

fn main() {
    colog::init();
    let mut player = Prismriver::new();
    player.set_volume(Volume::new(0.4));

    let mut paths = std::env::args().skip(1).enumerate().peekable();

    'main: while let Some((i, path)) = paths.next() {
        println!("Loading... {}", path);

        let uri = if path.starts_with("http") {
            path.parse::<Uri<String>>().unwrap()
        } else {
            path_to_uri(&path).unwrap()
        };

        player.load_new(&uri).unwrap();
        player.set_state(State::Playing);
        println!("Playing!");

        while player.state() == State::Playing || player.state() == State::Paused {
            sleep(std::time::Duration::from_millis(100));
            print_timer(player.position(), player.duration());

            if paths.peek().is_some() && player.flag() == Some(Flag::AboutToFinish) {
                continue 'main;
            }
        }
        println!();
    }

    println!("It's so over")
}

fn print_timer(pos: Option<Duration>, len: Option<Duration>) {
    let len_string = if let Some(p) = len {
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            p.num_seconds() / 3600,
            (p.num_seconds() / 60) % 60,
            p.num_seconds() % 60,
            p.num_milliseconds() % 1000
        )
    } else {
        "--:--:--.---".to_string()
    };

    let pos_string = if let Some(p) = pos {
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            p.num_seconds() / 3600,
            (p.num_seconds() / 60) % 60,
            p.num_seconds() % 60,
            p.num_milliseconds() % 1000
        )
    } else {
        "--:--:--.---".to_string()
    };

    print!("{} / {}\r", pos_string, len_string,);
    io::stdout().flush().unwrap();
}
