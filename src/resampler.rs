// Symphonia
// Copyright (c) 2019-2022 The Project Symphonia Developers.
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use symphonia::core::audio::SignalSpec;
use symphonia::core::conv::IntoSample;
use symphonia::core::sample::Sample;

pub struct Resampler {
    resampler: rubato::FftFixedIn<f32>,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    interleaved: Vec<f32>,
    duration: usize,
    channels: usize,
    in_rate: usize,
}

impl Resampler {
    fn resample_inner(&mut self) -> &[f32] {
        {
            let mut input: arrayvec::ArrayVec<&[f32], 32> = Default::default();

            for channel in self.input.iter() {
                input.push(&channel[..self.duration]);
            }

            // Resample.
            rubato::Resampler::process_into_buffer(
                &mut self.resampler,
                &input,
                &mut self.output,
                None,
            ).unwrap();
        }

        // Remove consumed samples from the input buffer.
        for channel in self.input.iter_mut() {
            channel.drain(0..self.duration);
        }

        self.interleaved.clear();
        self.interleaved.extend(self.output.iter().flatten().cloned());

        &self.interleaved
    }
}

impl Resampler {
    pub fn new(spec: SignalSpec, to_sample_rate: usize, duration: u64) -> Self {
        let duration = duration as usize;
        let channels = spec.channels.count();

        let resampler = rubato::FftFixedIn::<f32>::new(
            spec.rate as usize,
            to_sample_rate,
            duration,
            2,
            channels,
        ).unwrap();

        /*
        let ratio = to_sample_rate as f64 / spec.rate as f64;
        let resampler = rubato::FastFixedIn::<f32>::new(
            ratio,
            1.0,
            PolynomialDegree::Septic,
            duration,
            channels,
        ).unwrap();
        */

        let output = rubato::Resampler::output_buffer_allocate(&resampler, true);
        dbg!(output[0].len());

        let input = vec![Vec::with_capacity(duration); channels];

        Self {
            resampler,
            input,
            output,
            duration,
            interleaved: Vec::new(),
            channels,
            in_rate: spec.rate as usize,
        }
    }

    /// Resamples a planar/non-interleaved input.
    ///
    /// Returns the resampled samples in an interleaved format.
    pub fn resample(&mut self, input: &[f32]) -> Option<&[f32]> {
        // Copy and convert samples into input buffer.
        let mut offset = 0;
        for (_, dst) in self.input.iter_mut().enumerate() {
            let n = offset + (input.len() / self.channels);
            dst.extend(input[offset..n].iter().map(|&s| <f32 as IntoSample<f32>>::into_sample(s)));
            offset += input.len() / self.channels;
        }

        // Check if more samples are required.
        if self.input[0].len() < self.duration {
            return None;
        }

        Some(self.resample_inner())
    }

    /// Resample any remaining samples in the resample buffer.
    pub fn flush(&mut self) -> Option<&[f32]> {
        let len = self.input[0].len();

        if len == 0 {
            return None;
        }

        let partial_len = len % self.duration;

        if partial_len != 0 {
            // Fill each input channel buffer with silence to the next multiple of the resampler
            // duration.
            for channel in self.input.iter_mut() {
                channel.resize(len + (self.duration - partial_len), f32::MID);
            }
        }

        Some(self.resample_inner())
    }

    pub fn in_rate(&self) -> usize {
        self.in_rate
    }
}
