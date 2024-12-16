use std::io::{self, Write};
use std::thread::sleep;

use chrono::Duration;

use fluent_uri::Uri;
use prismriver::utils::path_to_uri;
use prismriver::{Flag, Prismriver};
use prismriver::State;
use prismriver::Volume;

fn main() {
    let mut player = Prismriver::new();
    player.set_volume(Volume::new(0.4));

    let paths: Vec<String> = std::env::args().skip(1).collect();

    for path in paths {
        println!("Playing... {}", path);

        let path_uri = path_to_uri(&path).unwrap();

        player.load_new(&path_uri).unwrap();
        player.set_state(State::Playing);

        while player.state() == State::Playing || player.state() == State::Paused {
            sleep(std::time::Duration::from_millis(100));
        }
    }
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
