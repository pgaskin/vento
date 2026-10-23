// SPDX-FileCopyrightText: 2026 Nick Haghiri
// SPDX-License-Identifier: GPL-3.0-or-later

//! The remote machine's sound, played on this phone.
//!
//! The protocol side is `rdpsnd`: the server picks one of the formats this
//! handler advertises and sends wave blocks in it. Those blocks are handed to
//! the protocol thread through a channel rather than to Java directly, because
//! the processor that receives them has no access to the [`Handler`]; the
//! session loop drains the channel after each batch of frames.

use std::borrow::Cow;
use std::sync::mpsc::Sender;

use ironrdp_rdpsnd::client::RdpsndClientHandler;
use ironrdp_rdpsnd::pdu::{AudioFormat, AudioFormatFlags, PitchPdu, VolumePdu, WaveFormat};

/// One wave block, as the server sent it, in the format it was negotiated for.
#[derive(Debug)]
pub struct AudioBlock {
    pub sample_rate: u32,
    pub channels: u16,
    pub pcm: Vec<u8>,
}

/// Plays PCM only: 16-bit stereo at the two rates Android's mixer handles
/// without resampling. Compressed formats are left out on purpose; the server
/// falls back to PCM when they are not advertised.
#[derive(Debug)]
pub struct PhoneAudio {
    formats: Vec<AudioFormat>,
    blocks: Sender<AudioBlock>,
}

impl PhoneAudio {
    pub fn new(blocks: Sender<AudioBlock>) -> Self {
        let formats = [44100, 48000]
            .into_iter()
            .map(|rate| AudioFormat {
                format: WaveFormat::PCM,
                n_channels: 2,
                n_samples_per_sec: rate,
                n_avg_bytes_per_sec: rate * 4,
                n_block_align: 4,
                bits_per_sample: 16,
                data: None,
            })
            .collect();
        PhoneAudio { formats, blocks }
    }
}

impl RdpsndClientHandler for PhoneAudio {
    fn get_flags(&self) -> AudioFormatFlags {
        // Volume is not applied on this side, so it is not advertised.
        AudioFormatFlags::empty()
    }

    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn wave(&mut self, format: &AudioFormat, _ts: u32, data: Cow<'_, [u8]>) {
        // A send error means the session loop has gone, and the connection with
        // it, so there is nothing to report.
        let _ = self.blocks.send(AudioBlock {
            sample_rate: format.n_samples_per_sec,
            channels: format.n_channels,
            pcm: data.into_owned(),
        });
    }

    fn set_volume(&mut self, _volume: VolumePdu) {}

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    fn close(&mut self) {}
}
