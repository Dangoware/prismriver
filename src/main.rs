use std::collections::HashMap;
use std::io::{self, Write};
use std::thread::sleep;

use chrono::Duration;

use fluent_uri::Uri;
use prismriver::utils::path_to_uri;
use prismriver::Prismriver;
use prismriver::State;
use prismriver::Volume;

fn main() {
    colog::init();
    let mut player = Prismriver::new();
    player.set_volume(Volume::new(0.3));

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut metadata = HashMap::new();

    for path in args {
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
            sleep(std::time::Duration::from_millis(100));
            print_timer(player.position(), player.duration());
            if player.metadata() != metadata {
                metadata = player.metadata();
                println!("{}", metadata.get("title").cloned().unwrap_or_default());
            }
        }
        println!();
    }

    println!("It's so over")
}

fn print_timer(pos: Option<Duration>, len: Option<Duration>) {
    let len_string = if let Some(l) = len {
        format!("{:02}:{:02}", l.num_seconds() / 60, l.num_seconds() % 60)
    } else {
        "--:--".to_string()
    };

    let pos_string = if let Some(p) = pos {
        format!("{:02}:{:02}", p.num_seconds() / 60, p.num_seconds() % 60)
    } else {
        "--:--".to_string()
    };

    print!("{}/{}\r", pos_string, len_string,);
    io::stdout().flush().unwrap();
}
