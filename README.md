# Prismriver
Prismriver is an audio library intended for music players which allows for 
simple, cross platform, high quality audio output.

## Goals
The overall goal of Prismriver is not to create an all-rust playback 
library, those already exist. Instead, it's supposed to function similarly
to Gstreamer, where one frontend can be used to build multimedia applications
easily. Because of that, being 100% Rust is not a goal of this project,
except where that would improve cross compilation or ease development.

## Functionality/Roadmap
- [x] Cross platform output with CPAL
- [x] Multiple decoder backends
    - [x] [symphonia](https://github.com/pdeljanov/Symphonia)
    - [x] ffmpeg
    - [ ] Audio CD
- [ ] Gapless

## Acknowledgements
The functionality of this library is inspired by the PHAzOR audio playback
module used in [Tauon](https://tauonmusicbox.rocks).

The name is based on the 
[Prismriver Sisters](https://en.touhouwiki.net/wiki/Prismriver_Sisters) from the
[Touhou Project](https://en.touhouwiki.net/wiki/Touhou_Project), a trio
of poltergeists who play wonderful music.
