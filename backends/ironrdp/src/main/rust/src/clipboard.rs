// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The clipboard channel, text only.
//!
//! RFB says "here is some Latin-1" in one message and is done. CLIPRDR is a
//! conversation with delayed rendering: whoever copies announces the *formats*
//! it could produce, and the other end asks for one of them if and when
//! somebody pastes. So both directions are two exchanges rather than one, and
//! the state between them lives here.
//!
//! Everything in this file runs on two threads. The [`CliprdrBackend`] half is
//! called from inside the protocol thread's decode; [`Clipboard::set_local`] is
//! called from wherever the app noticed a copy. Neither may encode anything —
//! the channel object that does the encoding is owned by the active stage — so
//! both sides leave a [`Want`] behind and the protocol loop picks it up.

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
};
use ironrdp_core::impl_as_any;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Longer than this is a file rather than a copy. The same number the Android
/// side stops reading its own clipboard at.
const MAX_CHARS: usize = 1 << 20;

/// Something only the channel can encode, left for the protocol thread.
#[derive(Debug)]
pub enum Want {
    /// Announce what we are holding.
    ///
    /// Also the channel's handshake: the client half stays in its
    /// initialisation state until a format list has been sent *and*
    /// acknowledged, and a paste before that is refused. So this goes out even
    /// when the list is empty, which is the honest way to say "nothing here".
    Advertise,
    /// Ask the remote for what it has copied, in a format it offered.
    Paste(ClipboardFormatId),
    /// Answer the remote's request for ours.
    Send(ClipboardFormatId),
}

/// The clipboard's half of a session.
#[derive(Debug, Default)]
pub struct Clipboard {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// What this phone last copied, or none if it is not being shared.
    ours: Option<String>,
    /// What we asked the remote for, so a response can be decoded as the thing
    /// it answers — a `FormatDataResponse` carries bytes and no format.
    pending: Option<ClipboardFormatId>,
    wants: VecDeque<Want>,
    incoming: VecDeque<String>,
}

impl Clipboard {
    /// Any thread: this phone copied something.
    pub fn set_local(&self, text: &str) {
        if text.is_empty() || text.chars().count() > MAX_CHARS {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.ours.as_deref() == Some(text) {
            return;
        }
        state.ours = Some(text.to_string());
        state.wants.push_back(Want::Advertise);
    }

    /// The formats we can produce. Two rather than one because a server that
    /// asks for `CF_TEXT` gets it: the remote decides which of the offered
    /// formats it wants, and offering only Unicode is a paste that fails on
    /// anything old.
    pub fn formats(&self) -> Vec<ClipboardFormat> {
        if self.state.lock().unwrap().ours.is_none() {
            return Vec::new();
        }
        vec![
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
            ClipboardFormat::new(ClipboardFormatId::CF_TEXT),
        ]
    }

    /// Our text in the format asked for, with the line endings a Windows
    /// application expects. `None` means we have nothing to give, which is an
    /// error response rather than empty data.
    pub fn data(&self, format: ClipboardFormatId) -> Option<Vec<u8>> {
        let text = crlf(self.state.lock().unwrap().ours.as_deref()?);
        match format {
            ClipboardFormatId::CF_UNICODETEXT => Some(utf16le(&text)),
            ClipboardFormatId::CF_TEXT => Some(latin1(&text)),
            _ => None,
        }
    }

    pub fn take_wants(&self) -> Vec<Want> {
        self.state.lock().unwrap().wants.drain(..).collect()
    }

    pub fn take_incoming(&self) -> Vec<String> {
        self.state.lock().unwrap().incoming.drain(..).collect()
    }

    /// The channel's view of this, for the connector to hold.
    pub fn backend(self: &Arc<Self>) -> Box<dyn CliprdrBackend> {
        Box::new(Channel {
            shared: Arc::clone(self),
        })
    }
}

/// What the channel calls, all of it on the protocol thread and none of it
/// allowed to encode anything.
#[derive(Debug)]
struct Channel {
    shared: Arc<Clipboard>,
}

impl_as_any!(Channel);

impl Channel {
    fn push(&self, want: Want) {
        self.shared.state.lock().unwrap().wants.push_back(want);
    }
}

impl CliprdrBackend for Channel {
    /// Where transferred files would go. Nothing here transfers a file, and the
    /// path is sent regardless during the channel's opening exchange.
    fn temporary_directory(&self) -> &str {
        "."
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        // The alternative is a fixed 32-byte ASCII name per format, which is
        // what CLIPRDR had before this flag existed. Nothing is lost by asking
        // for the newer one and a server that does not have it says so.
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {
        log::debug!("the clipboard channel is up");
    }

    fn on_request_format_list(&mut self) {
        self.push(Want::Advertise);
    }

    fn on_process_negotiated_capabilities(&mut self, capabilities: ClipboardGeneralCapabilityFlags) {
        log::debug!("clipboard capabilities: {capabilities:?}");
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        // Unicode first: `CF_TEXT` is the server's code page, which we would
        // have to guess at, and every server that has one has the other.
        let Some(format) = [
            ClipboardFormatId::CF_UNICODETEXT,
            ClipboardFormatId::CF_TEXT,
        ]
        .into_iter()
        .find(|id| available_formats.iter().any(|f| f.id == *id)) else {
            log::debug!("the remote copied something that is not text");
            return;
        };
        let mut state = self.shared.state.lock().unwrap();
        state.pending = Some(format);
        state.wants.push_back(Want::Paste(format));
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.push(Want::Send(request.format));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let format = self.shared.state.lock().unwrap().pending.take();
        if response.is_error() {
            log::debug!("the remote could not produce its clipboard");
            return;
        }
        let text = match format {
            Some(ClipboardFormatId::CF_UNICODETEXT) => from_utf16le(response.data()),
            Some(ClipboardFormatId::CF_TEXT) => from_latin1(response.data()),
            // A response to a request nobody remembers making.
            _ => return,
        };
        let text = lf(&text);
        if text.is_empty() || text.chars().count() > MAX_CHARS {
            return;
        }
        let mut state = self.shared.state.lock().unwrap();
        // Ours already, so that the copy this provokes on the phone is not
        // announced straight back to the remote.
        state.ours = Some(text.clone());
        state.incoming.push_back(text);
    }

    fn on_file_contents_request(&mut self, _request: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, _response: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _data_id: LockDataId) {}

    fn on_unlock(&mut self, _data_id: LockDataId) {}
}

// ---- the two encodings, and the line endings between them -------------------

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16()
        // Terminated, because a Windows application reads it as a C string and
        // pastes whatever follows in the buffer otherwise.
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn from_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// The server's code page, which we do not know; the low 256 code points are
/// what every candidate for it agrees on.
fn latin1(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' })
        .chain(std::iter::once(0))
        .collect()
}

fn from_latin1(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect()
}

/// What a Windows application puts on the clipboard.
fn crlf(text: &str) -> String {
    lf(text).replace('\n', "\r\n")
}

/// What everything above this expects.
fn lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round trips, including the terminator that is written and not read back.
    #[test]
    fn text_survives_both_encodings() {
        for text in ["hello", "héllo\nwörld", "", "tabs\tand\r\nnewlines"] {
            assert_eq!(from_utf16le(&utf16le(text)), text);
        }
        assert_eq!(from_latin1(&latin1("héllo")), "héllo");
        // Outside the low 256, which is where a code page we do not know would
        // start disagreeing with itself.
        assert_eq!(from_latin1(&latin1("日本")), "??");
    }

    /// One newline in, one newline out, whichever end started it.
    #[test]
    fn line_endings_are_normalised_once() {
        assert_eq!(crlf("a\nb"), "a\r\nb");
        assert_eq!(crlf("a\r\nb"), "a\r\nb");
        assert_eq!(lf("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }

    /// A copy is announced once; the same text again is not.
    #[test]
    fn only_a_change_is_announced() {
        let clip = Clipboard::default();
        clip.set_local("hello");
        clip.set_local("hello");
        assert_eq!(clip.take_wants().len(), 1);
        assert_eq!(clip.formats().len(), 2);
        assert_eq!(
            clip.data(ClipboardFormatId::CF_UNICODETEXT),
            Some(utf16le("hello"))
        );
    }

    /// With nothing to offer, the list is empty rather than absent — the
    /// channel does not open until one has been sent.
    #[test]
    fn an_empty_clipboard_still_has_a_format_list() {
        let clip = Arc::new(Clipboard::default());
        let mut channel = clip.backend();
        channel.on_request_format_list();
        assert_eq!(clip.take_wants().len(), 1);
        assert!(clip.formats().is_empty());
        assert_eq!(clip.data(ClipboardFormatId::CF_UNICODETEXT), None);
    }
}
