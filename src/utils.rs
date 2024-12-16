use std::path::{Path, PathBuf};

use fluent_uri::{
    component::Scheme,
    encoding::{encoder, EString},
    Uri,
};

use crate::decode::{self, Decoder};

#[must_use]
pub(super) fn pick_format(uri: &Uri<String>) -> Option<Box<dyn Decoder>> {
    // If it's a network stream, use ffmpeg
    #[cfg(feature = "ffmpeg")]
    if uri.scheme().as_str().starts_with("http") {
        if let Ok(d) = decode::ffmpeg::FfmpegDecoder::new(uri) {
            return Some(Box::new(d));
        }
    }

    // If it's not a network stream, try symphonia
    #[cfg(feature = "symphonia")]
    if uri.scheme().as_str() == "file" {
        if let Ok(d) = decode::rusty::RustyDecoder::new(uri) {
            return Some(Box::new(d));
        }
    }

    // If symphonia can't parse it, try ffmpeg
    #[cfg(feature = "ffmpeg")]
    {
        if let Ok(d) = decode::ffmpeg::FfmpegDecoder::new(uri) {
            return Some(Box::new(d));
        }
    }

    if cfg!(not(any(feature = "symphonia", feature = "ffmpeg"))) {
        log::error!("using dummmy decoder for {}", uri);
        Some(Box::new(decode::dummy::DummyDecoder::new()))
    } else {
        None
    }
}

/// Turn a path into a [`Uri<Path>`] by finding the canonical path for the given
/// path and prepending it with the `file://` scheme.
///
/// This function errors if the path cannot be canonicalized.
pub fn path_to_uri<P: AsRef<Path>>(path: &P) -> Result<Uri<String>, Box<dyn std::error::Error>> {
    let canonicalized = path.as_ref().canonicalize()?;
    let path_string = canonicalized.to_string_lossy();

    let mut percent_path: EString<encoder::Path> = EString::new();
    percent_path.encode::<encoder::Path>(&path_string.to_string());
    let uri = Uri::<String>::builder()
        .scheme(Scheme::new_or_panic("file"))
        .path(&percent_path)
        .build()?;

    Ok(uri)
}

/// Turn a [`Uri<String>`] with the `file://` scheme into an absolute system
/// path.
pub fn uri_to_path(uri: &Uri<String>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let estr = uri.path();
    let decoded = estr.decode().into_string()?;
    let path = PathBuf::from(decoded.to_string());

    Ok(path)
}
