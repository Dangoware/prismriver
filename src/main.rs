use cpal::traits::HostTrait as _;
use fluent_uri::Uri;
use log::info;
use prismriver::{audio_output::open_output, utils::{self, path_to_uri}, Volume};

fn main() {
    colog::init();

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .expect("no output device available");

    let mut audio_output = open_output(&device).unwrap();
    audio_output.set_volume(Volume::new(0.5));

    let mut decoder = utils::pick_format(&make_uri("./Kachigumi.flac")).unwrap();

    info!(
        "samplerate: {}; channels: {}; packet size: {}",
        decoder.params().rate,
        decoder.params().channels,
        decoder.params().packet_size
    );

    audio_output.update_input_params(decoder.params());

    let mut output_buffer = vec![0f32; decoder.params().packet_size as usize * 4];

    loop {
        if audio_output.buffer_level() > audio_output.buffer_healthy() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        } else if let Ok(bytes) = decoder.next_packet_to_buf(&mut output_buffer) {
            if bytes == 0 {
                break
            }

            audio_output.write(&output_buffer[..bytes]);
        } else if audio_output.buffer_level() == 0 {
            break
        }
    }
}

fn make_uri(input: &str) -> Uri<String> {
    if input.starts_with("http") {
        input.parse::<Uri<String>>().unwrap()
    } else {
        path_to_uri(&input).unwrap()
    }
}

/*
fn print_timer(pos: Option<chrono::Duration>, len: Option<chrono::Duration>) {
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
*/
