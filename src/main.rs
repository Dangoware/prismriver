use std::io::{self, Write as _};
use std::thread::sleep;

use chrono::Duration;

use fluent_uri::Uri;
use prismriver::utils::path_to_uri;
use prismriver::{Flag, Prismriver};
use prismriver::State;
use prismriver::Volume;

fn main() {
    colog::init();
    let mut player = Prismriver::new();
    player.set_volume(Volume::new(0.4));

    let mut paths = std::env::args().skip(1).peekable();

    if paths.len() == 0 {
        println!("Please input a filename or URL");
        return;
    }

    let mut path = paths.next().unwrap();
    let mut path_uri = make_uri(path.clone());
    loop {
        println!("Playing \"{}\"", path);
        player.load_new(&path_uri).unwrap();

        player.play();

        while player.state() == State::Playing || player.state() == State::Paused {
            sleep(std::time::Duration::from_millis(10));
            print_timer(player.position(), player.duration());

            if paths.peek().is_some() && player.flag() == Some(Flag::AboutToFinish) {
                break;
            }
        }

        if let State::Errored(e) = player.state() {
            log::error!("{e}");
        }

        if let Some(p) = paths.next() {
            path = p;
            path_uri = make_uri(path.clone());
        } else {
            break
        }
    }

    println!("It's so over")
}

fn make_uri(input: String) -> Uri<String> {
    if input.starts_with("http") {
        input.parse::<Uri<String>>().unwrap()
    } else {
        path_to_uri(&input).unwrap()
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
