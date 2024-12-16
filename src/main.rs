use std::io::{self, Write};
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

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut paths = args.iter().filter(|a| !a.starts_with('-')).cloned().peekable();
    let flags: Vec<String> = args.iter().filter(|a| a.starts_with('-')).cloned().collect();

    for flag in flags {
        if flag.starts_with("-v") {
            let vol = flag.as_str()[2..].parse().unwrap();
            player.set_volume(Volume::new(vol));
        }
    }

    while let Some(path) = paths.next() {
        println!("Loading... {}", path);

        let uri = if path.starts_with("http") {
            path.parse::<Uri<String>>().unwrap()
        } else {
            path_to_uri(&path).unwrap()
        };

        //let path = PathBuf::from(path);
        player.load_new(&uri).unwrap();
        player.set_state(State::Playing);
        println!("Playing!");

        while player.state() == State::Playing {
            sleep(std::time::Duration::from_millis(50));
            print_timer(player.position(), player.duration());
            if paths.peek().is_some() && player.flag() == Some(Flag::AboutToFinish) {
                break;
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
        "--:--".to_string()
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
        "--:--".to_string()
    };

    print!("{} / {}\r", pos_string, len_string,);
    io::stdout().flush().unwrap();
}
